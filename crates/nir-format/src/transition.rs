use serde::{Deserialize, Serialize};
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StageTransition {
    #[default]
    Dissolve,
    Mask {
        asset: String,
        channel: MaskChannel,
        #[serde(default)]
        invert: bool,
        #[serde(default)]
        softness: f32,
    },
    Wipe {
        direction: WipeDirection,
        #[serde(default)]
        softness: f32,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskChannel {
    Alpha,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WipeDirection {
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
}
impl StageTransition {
    pub fn asset(&self) -> Option<&str> {
        match self {
            Self::Mask { asset, .. } => Some(asset),
            _ => None,
        }
    }
    pub fn capability(&self) -> Option<&'static str> {
        match self {
            Self::Dissolve => None,
            Self::Wipe { .. } => Some("stage.wipe.v1"),
            Self::Mask { .. } => Some("stage.mask.v1"),
        }
    }
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Dissolve)
    }
    pub fn valid(&self) -> bool {
        match self {
            Self::Dissolve => true,
            Self::Mask {
                asset, softness, ..
            } => !asset.is_empty() && softness.is_finite() && (0. ..=1.).contains(softness),
            Self::Wipe { softness, .. } => softness.is_finite() && (0. ..=1.).contains(softness),
        }
    }
    /// Packed parameters for the existing two-input compositor, not a color.
    pub fn parameters(&self, progress: f32) -> [f32; 4] {
        match self {
            Self::Dissolve => [0., 0., 0., progress],
            Self::Mask {
                softness, invert, ..
            } => [5., *softness, if *invert { 1. } else { 0. }, progress],
            Self::Wipe {
                direction,
                softness,
            } => [
                match direction {
                    WipeDirection::LeftToRight => 1.,
                    WipeDirection::RightToLeft => 2.,
                    WipeDirection::TopToBottom => 3.,
                    WipeDirection::BottomToTop => 4.,
                },
                *softness,
                0.,
                progress,
            ],
        }
    }
    /// Reference coverage, matching the GPU's normalized frozen-stage mask.
    pub fn coverage(&self, progress: f32, uv: [f32; 2], mask_sample: f32) -> f32 {
        if progress <= 0. {
            return 0.;
        }
        if progress >= 1. {
            return 1.;
        }
        let (coordinate, softness) = match self {
            Self::Dissolve => return progress,
            Self::Mask {
                invert, softness, ..
            } => (
                if *invert {
                    1. - mask_sample
                } else {
                    mask_sample
                },
                *softness,
            ),
            Self::Wipe {
                direction,
                softness,
            } => (
                match direction {
                    WipeDirection::LeftToRight => uv[0],
                    WipeDirection::RightToLeft => 1. - uv[0],
                    WipeDirection::TopToBottom => uv[1],
                    WipeDirection::BottomToTop => 1. - uv[1],
                },
                *softness,
            ),
        };
        if softness == 0. {
            return if coordinate <= progress { 1. } else { 0. };
        }
        let threshold = progress * (1. + softness) - softness / 2.;
        let t = ((coordinate - (threshold - softness / 2.)) / softness).clamp(0., 1.);
        1. - t * t * (3. - 2. * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alpha_masks_obey_polarity_and_exact_endpoints() {
        for invert in [false, true] {
            let mask = StageTransition::Mask {
                asset: "pattern".into(),
                channel: MaskChannel::Alpha,
                invert,
                softness: 0.2,
            };
            assert_eq!(mask.coverage(0., [0., 0.], 0.), 0.);
            assert_eq!(mask.coverage(1., [0., 0.], 1.), 1.);
            assert_eq!(
                mask.coverage(0.5, [0., 0.], 0.),
                if invert { 0. } else { 1. }
            );
            assert_eq!(
                mask.coverage(0.5, [0., 0.], 1.),
                if invert { 1. } else { 0. }
            );
            assert!((mask.coverage(0.5, [0., 0.], 0.5) - 0.5).abs() < 0.00001);
        }
    }
    #[test]
    fn wipe_endpoints_directions_and_soft_edge_are_bounded() {
        for direction in [
            WipeDirection::LeftToRight,
            WipeDirection::RightToLeft,
            WipeDirection::TopToBottom,
            WipeDirection::BottomToTop,
        ] {
            for softness in [0., 0.2, 1.] {
                let wipe = StageTransition::Wipe {
                    direction,
                    softness,
                };
                for uv in [[0., 0.], [0.5, 0.5], [1., 1.]] {
                    assert_eq!(wipe.coverage(0., uv, 0.), 0.);
                    assert_eq!(wipe.coverage(1., uv, 0.), 1.);
                    let mut previous = 0.;
                    for i in 0..=100 {
                        let v = wipe.coverage(i as f32 / 100., uv, 0.);
                        assert!(v >= previous && (0. ..=1.).contains(&v));
                        previous = v;
                    }
                }
            }
        }
        let wipe = StageTransition::Wipe {
            direction: WipeDirection::LeftToRight,
            softness: 0.2,
        };
        assert_eq!(wipe.coverage(0.5, [0.2, 0.5], 0.), 1.);
        assert_eq!(wipe.coverage(0.5, [0.8, 0.5], 0.), 0.);
        assert!((wipe.coverage(0.5, [0.5, 0.5], 0.) - 0.5).abs() < 0.00001);
    }
}
