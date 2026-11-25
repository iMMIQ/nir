//! Platform-agnostic touch gesture recognizer shared across Android Native and Mobile Web.
//!
//! Uses monotonic `delta_us` ticks (never `std::time::Instant`) so behavior is 100% deterministic
//! across `wasm32-unknown-unknown`, Android, desktop, and unit tests.

use nir_ir::UiAction;
use nir_ui::Screen;

const TAP_SLOP_PX: f32 = 14.0;
const TAP_MAX_TRAVEL_PX: f32 = 16.0;
const SWIPE_THRESHOLD_PX: f32 = 56.0;
const TWO_FINGER_SLOP_PX: f32 = 28.0;
const HISTORY_SCROLL_STEP_PX: f32 = 36.0;
const LEFT_EDGE_GUARD_PX: f32 = 24.0;
const LONG_PRESS_SKIP_US: u64 = 420_000;
const TWO_FINGER_WINDOW_US: u64 = 350_000;
const DOUBLE_TAP_WINDOW_US: u64 = 300_000;
const DOUBLE_TAP_MAX_DIST_PX: f32 = 32.0;

#[derive(Debug, Clone, Copy)]
struct TouchPoint {
    id: u64,
    start: [f32; 2],
    cur: [f32; 2],
    prev: [f32; 2],
    last_scroll_y: f32,
    max_dist_sq: f32,
}

/// High-level semantic action recognized from raw multi-touch events.
#[derive(Debug, Clone, PartialEq)]
pub enum TouchAction {
    /// Finger pressed down on first contact (highlight pressed control or capture slider).
    Down(f32, f32),
    /// Clean single-finger tap released at `(x, y)`.
    Tap(f32, f32),
    /// Cancel any active button press highlight (e.g. finger dragged off or 2nd finger landed).
    CancelPress,
    /// Two-finger tap -> open menu / close overlay / cancel choice.
    CancelOrMenu,
    /// Long-press skip started (`true`) or released (`false`).
    HoldSkip(bool),
    /// Recognized swipe or continuous scroll UI action.
    Dispatch(UiAction),
    /// Continuous single-finger slider drag position `(x, y)`.
    SliderDrag(f32, f32),
    /// CG lightbox swipe to previous (`-1`) or next (`+1`) variant.
    CgSwipe(i32),
    /// CG lightbox two-finger pinch zoom multiplier (`> 1.0` zoom in, `< 1.0` zoom out).
    CgPinchZoom { factor: f32, center: [f32; 2] },
    /// CG lightbox single-finger pan delta `(dx, dy)` while zoomed in.
    CgPan { dx: f32, dy: f32 },
    /// CG lightbox double-tap toggle (1x <-> 2x zoom).
    CgDoubleTap([f32; 2]),
}

#[derive(Debug, Default)]
pub struct TouchGestureEngine {
    active: Vec<TouchPoint>,
    max_fingers: usize,
    single_hold_us: u64,
    gesture_consumed: bool,
    long_press_active: bool,
    two_finger_candidate: bool,
    press_cancelled: bool,
    pinch_prev_dist: f32,
    last_cg_tap: Option<([f32; 2], u64)>,
    clock_us: u64,
}

impl TouchGestureEngine {
    /// Returns true while a stationary single finger is awaiting the long-press threshold
    /// so idle-pausing hosts keep ticking until the threshold fires or the finger lifts.
    pub fn needs_clock(&self) -> bool {
        self.active.len() == 1
            && self.max_fingers == 1
            && !self.gesture_consumed
            && !self.long_press_active
            && self.active[0].max_dist_sq <= TAP_SLOP_PX * TAP_SLOP_PX
    }

    pub fn is_long_press_active(&self) -> bool {
        self.long_press_active
    }

    pub fn reset(&mut self) -> Option<TouchAction> {
        let was_hold = self.long_press_active;
        self.active.clear();
        self.max_fingers = 0;
        self.single_hold_us = 0;
        self.gesture_consumed = false;
        self.long_press_active = false;
        self.two_finger_candidate = false;
        self.press_cancelled = false;
        self.pinch_prev_dist = 0.0;
        if was_hold {
            Some(TouchAction::HoldSkip(false))
        } else {
            None
        }
    }

    fn two_finger_dist_and_center(&self) -> Option<(f32, [f32; 2])> {
        if self.active.len() < 2 {
            return None;
        }
        let a = self.active[0].cur;
        let b = self.active[1].cur;
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let dist = (dx * dx + dy * dy).sqrt();
        Some((dist, [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]))
    }

    pub fn touch_start(&mut self, id: u64, x: f32, y: f32) -> Option<TouchAction> {
        // Deduplicate stale touch IDs if platform dropped an end event.
        self.active.retain(|p| p.id != id);

        if self.active.is_empty() {
            self.max_fingers = 1;
            self.single_hold_us = 0;
            self.gesture_consumed = false;
            self.long_press_active = false;
            self.two_finger_candidate = false;
            self.press_cancelled = false;
            self.pinch_prev_dist = 0.0;
            self.active.push(TouchPoint {
                id,
                start: [x, y],
                cur: [x, y],
                prev: [x, y],
                last_scroll_y: y,
                max_dist_sq: 0.0,
            });
            return Some(TouchAction::Down(x, y));
        }

        self.active.push(TouchPoint {
            id,
            start: [x, y],
            cur: [x, y],
            prev: [x, y],
            last_scroll_y: y,
            max_dist_sq: 0.0,
        });
        self.max_fingers = self.max_fingers.max(self.active.len());

        if self.long_press_active {
            self.long_press_active = false;
            self.gesture_consumed = true;
            self.two_finger_candidate = false;
            return Some(TouchAction::HoldSkip(false));
        }

        if self.active.len() == 2 {
            if let Some((d, _)) = self.two_finger_dist_and_center() {
                self.pinch_prev_dist = d;
            }
        }

        if self.active.len() == 2
            && self.max_fingers == 2
            && !self.gesture_consumed
            && self.single_hold_us <= TWO_FINGER_WINDOW_US
        {
            self.two_finger_candidate = true;
        } else {
            self.two_finger_candidate = false;
            self.gesture_consumed = true;
        }
        self.press_cancelled = true;
        Some(TouchAction::CancelPress)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn touch_move(
        &mut self,
        id: u64,
        x: f32,
        y: f32,
        screen: Screen,
        slider_active: bool,
        viewing_cg: bool,
        cg_zoomed: bool,
    ) -> Option<TouchAction> {
        let active_len = self.active.len();
        let idx = self.active.iter().position(|p| p.id == id)?;
        let (step_dx, step_dy, dist_sq, max_dist_sq) = {
            let pt = &mut self.active[idx];
            let step_dx = x - pt.cur[0];
            let step_dy = y - pt.cur[1];
            pt.prev = pt.cur;
            pt.cur = [x, y];
            let dx = x - pt.start[0];
            let dy = y - pt.start[1];
            let dist_sq = dx * dx + dy * dy;
            if dist_sq > pt.max_dist_sq {
                pt.max_dist_sq = dist_sq;
            }
            (step_dx, step_dy, dist_sq, pt.max_dist_sq)
        };

        // Two-finger pinch zoom inside CG Lightbox!
        if viewing_cg && active_len == 2 {
            if let Some((dist, center)) = self.two_finger_dist_and_center() {
                if self.pinch_prev_dist > 12.0 && dist > 12.0 {
                    let ratio = dist / self.pinch_prev_dist;
                    if (ratio - 1.0).abs() > 0.015 {
                        self.pinch_prev_dist = dist;
                        self.two_finger_candidate = false;
                        self.gesture_consumed = true;
                        return Some(TouchAction::CgPinchZoom {
                            factor: ratio.clamp(0.75, 1.35),
                            center,
                        });
                    }
                } else {
                    self.pinch_prev_dist = dist;
                }
            }
        }

        if self.two_finger_candidate && dist_sq > TWO_FINGER_SLOP_PX * TWO_FINGER_SLOP_PX {
            self.two_finger_candidate = false;
        }

        if active_len == 1 && self.max_fingers == 1 && !self.long_press_active {
            // Continuous slider scrubbing in Settings!
            if slider_active {
                self.gesture_consumed = true;
                return Some(TouchAction::SliderDrag(x, y));
            }
            // Single-finger panning inside zoomed CG Lightbox!
            if viewing_cg && cg_zoomed && max_dist_sq > (TAP_SLOP_PX * 0.5) * (TAP_SLOP_PX * 0.5) {
                self.gesture_consumed = true;
                self.press_cancelled = true;
                return Some(TouchAction::CgPan {
                    dx: step_dx,
                    dy: step_dy,
                });
            }
            if screen == Screen::History {
                let delta_y = y - self.active[idx].last_scroll_y;
                if delta_y.abs() >= HISTORY_SCROLL_STEP_PX {
                    let steps = (delta_y / HISTORY_SCROLL_STEP_PX).trunc() as i32;
                    if steps != 0 {
                        self.active[idx].last_scroll_y += steps as f32 * HISTORY_SCROLL_STEP_PX;
                        self.gesture_consumed = true;
                        self.press_cancelled = true;
                        return Some(TouchAction::Dispatch(UiAction::ScrollHistory(steps)));
                    }
                }
            }
            if !self.press_cancelled && max_dist_sq > TAP_SLOP_PX * TAP_SLOP_PX {
                self.press_cancelled = true;
                return Some(TouchAction::CancelPress);
            }
        }
        None
    }

    pub fn tick(&mut self, delta_us: u64, can_long_press_skip: bool) -> Option<TouchAction> {
        self.clock_us = self.clock_us.saturating_add(delta_us);
        if self.active.len() == 1
            && self.max_fingers == 1
            && !self.gesture_consumed
            && !self.long_press_active
            && self.active[0].max_dist_sq <= TAP_SLOP_PX * TAP_SLOP_PX
        {
            self.single_hold_us = self.single_hold_us.saturating_add(delta_us);
            if can_long_press_skip && self.single_hold_us >= LONG_PRESS_SKIP_US {
                self.long_press_active = true;
                self.gesture_consumed = true;
                return Some(TouchAction::HoldSkip(true));
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub fn touch_end(
        &mut self,
        id: u64,
        x: f32,
        y: f32,
        screen: Screen,
        allow_story_swipes: bool,
        viewing_cg: bool,
        cg_zoomed: bool,
    ) -> Option<TouchAction> {
        let pos = self.active.iter().position(|p| p.id == id)?;
        let mut pt = self.active.remove(pos);
        pt.cur = [x, y];
        let dx = x - pt.start[0];
        let dy = y - pt.start[1];
        let dist_sq = dx * dx + dy * dy;
        if dist_sq > pt.max_dist_sq {
            pt.max_dist_sq = dist_sq;
        }
        if self.active.len() < 2 {
            self.pinch_prev_dist = 0.0;
        }

        if self.long_press_active {
            if self.active.is_empty() {
                self.long_press_active = false;
                return Some(TouchAction::HoldSkip(false));
            }
            return None;
        }

        if self.two_finger_candidate {
            if pt.max_dist_sq > TWO_FINGER_SLOP_PX * TWO_FINGER_SLOP_PX {
                self.two_finger_candidate = false;
            }
            if self.active.is_empty() {
                let fire = self.two_finger_candidate && !self.gesture_consumed;
                self.two_finger_candidate = false;
                if fire {
                    self.gesture_consumed = true;
                    return Some(TouchAction::CancelOrMenu);
                }
            }
            return None;
        }

        if self.max_fingers > 1 || self.gesture_consumed {
            return None;
        }

        let dist = dist_sq.sqrt();
        if dist < TAP_SLOP_PX && pt.max_dist_sq < TAP_MAX_TRAVEL_PX * TAP_MAX_TRAVEL_PX {
            // Quick double-tap inside CG viewer toggles 1x <-> 2x zoom!
            if viewing_cg {
                if let Some((prev_pos, prev_us)) = self.last_cg_tap {
                    let dt = self.clock_us.saturating_sub(prev_us);
                    let ddx = x - prev_pos[0];
                    let ddy = y - prev_pos[1];
                    if dt <= DOUBLE_TAP_WINDOW_US
                        && ddx * ddx + ddy * ddy <= DOUBLE_TAP_MAX_DIST_PX * DOUBLE_TAP_MAX_DIST_PX
                    {
                        self.last_cg_tap = None;
                        return Some(TouchAction::CgDoubleTap([x, y]));
                    }
                }
                self.last_cg_tap = Some(([x, y], self.clock_us));
            }
            return Some(TouchAction::Tap(x, y));
        }

        if dist >= SWIPE_THRESHOLD_PX {
            if viewing_cg && !cg_zoomed && dx.abs() > dy.abs() * 1.25 {
                return Some(TouchAction::CgSwipe(if dx < 0.0 { 1 } else { -1 }));
            }
            if matches!(screen, Screen::Saves { .. }) && dx.abs() > dy.abs() * 1.25 {
                let delta = if dx < 0.0 { 1 } else { -1 };
                return Some(TouchAction::Dispatch(UiAction::SavesPage(delta)));
            }
            if screen == Screen::Gallery && dx.abs() > dy.abs() * 1.25 {
                let delta = if dx < 0.0 { 1 } else { -1 };
                return Some(TouchAction::Dispatch(UiAction::GalleryPage(delta)));
            }
            if screen == Screen::Story && allow_story_swipes {
                if dy.abs() > dx.abs() * 1.25 {
                    let act = if dy < 0.0 {
                        UiAction::OpenHistory
                    } else {
                        UiAction::ToggleHideUi
                    };
                    return Some(TouchAction::Dispatch(act));
                }
                if dx.abs() > dy.abs() * 1.25 {
                    // Guard left 24px browser edge so OS edge-back swipe doesn't also rollback!
                    if dx > 0.0 && pt.start[0] < LEFT_EDGE_GUARD_PX {
                        return Some(TouchAction::CancelPress);
                    }
                    let act = if dx > 0.0 {
                        UiAction::Rollback
                    } else {
                        UiAction::Advance
                    };
                    return Some(TouchAction::Dispatch(act));
                }
            }
        }

        Some(TouchAction::CancelPress)
    }

    pub fn touch_cancel(&mut self, id: u64) -> Option<TouchAction> {
        self.active.retain(|p| p.id != id);
        if self.active.len() < 2 {
            self.pinch_prev_dist = 0.0;
        }
        if self.long_press_active && self.active.is_empty() {
            self.long_press_active = false;
            self.gesture_consumed = false;
            return Some(TouchAction::HoldSkip(false));
        }
        if self.active.is_empty() {
            self.two_finger_candidate = false;
            self.gesture_consumed = false;
            self.press_cancelled = false;
        }
        Some(TouchAction::CancelPress)
    }
}
