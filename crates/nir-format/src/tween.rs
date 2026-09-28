//! Pure scalar animation math shared by story, UI and audio owners.
//! Ownership and clocks stay with the caller; this module performs no scheduling.
use crate::{CancelPolicy, Easing, FinishPolicy, Micros};

/// Author-controlled appearance; user visibility is a separate mask.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DialogueAppearance {
    pub opacity: f32,
    pub background_opacity: f32,
    pub text_opacity: f32,
}
impl Default for DialogueAppearance {
    fn default() -> Self {
        Self {
            opacity: 1.,
            background_opacity: 1.,
            text_opacity: 1.,
        }
    }
}
impl DialogueAppearance {
    pub fn get(&self, property: crate::DialogueProperty) -> f32 {
        match property {
            crate::DialogueProperty::Opacity => self.opacity,
            crate::DialogueProperty::BackgroundOpacity => self.background_opacity,
            crate::DialogueProperty::TextOpacity => self.text_opacity,
        }
    }
    pub fn set(&mut self, property: crate::DialogueProperty, value: f32) {
        match property {
            crate::DialogueProperty::Opacity => self.opacity = value,
            crate::DialogueProperty::BackgroundOpacity => self.background_opacity = value,
            crate::DialogueProperty::TextOpacity => self.text_opacity = value,
        }
    }
    pub fn valid(&self) -> bool {
        [self.opacity, self.background_opacity, self.text_opacity]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    }
}

/// Interpolate finite endpoints without overflowing `to - from` in f32.
/// Endpoints are exact, including zero-duration completion.
pub fn interpolate(from: f32, to: f32, progress: f32, easing: Easing) -> f32 {
    let p = progress.clamp(0., 1.) as f64;
    let p = match easing {
        Easing::Linear => p,
        Easing::Smooth => p * p * (3. - 2. * p),
    };
    if p <= 0. {
        from
    } else if p >= 1. {
        to
    } else {
        ((1. - p) * from as f64 + p * to as f64) as f32
    }
}

/// Captured values and policies, independent of the target's type or owner.
#[derive(Debug, Clone, Copy)]
pub struct ScalarTween {
    pub from: f32,
    pub base: f32,
    pub to: f32,
    pub duration_us: Micros,
    pub easing: Easing,
    pub finish: FinishPolicy,
    pub cancel: CancelPolicy,
}
impl ScalarTween {
    pub fn sample(&self, elapsed_us: Micros) -> f32 {
        let progress = if self.duration_us.0 == 0 {
            1.
        } else {
            (elapsed_us.0 as f64 / self.duration_us.0 as f64).min(1.) as f32
        };
        interpolate(self.from, self.to, progress, self.easing)
    }
    pub fn settle(&self, elapsed_us: Micros, completed: bool) -> f32 {
        if completed {
            match self.finish {
                FinishPolicy::CommitEnd => self.to,
                FinishPolicy::RemoveEffect => self.base,
            }
        } else {
            match self.cancel {
                CancelPolicy::CommitCurrent => self.sample(elapsed_us),
                CancelPolicy::SettleEnd => self.to,
                CancelPolicy::RestoreBase => self.base,
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> ScalarTween {
        ScalarTween {
            from: 0.2,
            base: 0.1,
            to: 0.8,
            duration_us: Micros(1000),
            easing: Easing::Linear,
            finish: FinishPolicy::CommitEnd,
            cancel: CancelPolicy::CommitCurrent,
        }
    }
    #[test]
    fn endpoint_midpoint_and_all_settlement_policies() {
        let mut t = track();
        assert_eq!(t.sample(Micros(0)), 0.2);
        assert_eq!(t.sample(Micros(500)), 0.5);
        assert_eq!(t.sample(Micros(2000)), 0.8);
        assert_eq!(t.settle(Micros(500), false), 0.5);
        assert_eq!(t.settle(Micros(500), true), 0.8);
        t.cancel = CancelPolicy::RestoreBase;
        assert_eq!(t.settle(Micros(500), false), 0.1);
        t.cancel = CancelPolicy::SettleEnd;
        assert_eq!(t.settle(Micros(500), false), 0.8);
        t.finish = FinishPolicy::RemoveEffect;
        assert_eq!(t.settle(Micros(500), true), 0.1);
        t.duration_us = Micros(0);
        assert_eq!(t.sample(Micros(0)), 0.8);
    }
    #[test]
    fn opposite_finite_extremes_stay_finite() {
        assert_eq!(interpolate(f32::MAX, -f32::MAX, 0.5, Easing::Linear), 0.);
        for easing in [Easing::Linear, Easing::Smooth] {
            for n in 0..=100 {
                let value = interpolate(f32::MAX, -f32::MAX, n as f32 / 100., easing);
                assert!(value.is_finite());
            }
        }
    }
}
