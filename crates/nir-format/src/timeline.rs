//! Immutable sprite keyframes. Clocks, preparation and task ownership belong
//! to Core; a sampled pose never owns or decodes a texture.
use crate::{Micros, Node, SpriteTransform};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpriteTimeline {
    pub id: String,
    pub duration_us: Micros,
    pub tracks: Vec<SpriteTimelineTrack>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpriteTimelineTrack {
    pub node: String,
    pub frames: Vec<SpriteKeyframe>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpriteKeyframe {
    pub at_us: Micros,
    /// Local geometry; the parent keeps its independent pose and clock.
    pub rect: [f32; 4],
    pub opacity: f32,
    #[serde(default = "crate::white", skip_serializing_if = "color_is_white")]
    pub color: [f32; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<SpriteTransform>,
}
fn color_is_white(color: &[f32; 4]) -> bool {
    *color == [1.; 4]
}
impl SpriteKeyframe {
    pub fn valid(&self) -> bool {
        self.rect.iter().all(|v| v.is_finite())
            && self.rect[..2].iter().all(|v| v.abs() <= 32768.)
            && self.rect[2..].iter().all(|v| (0. ..=8192.).contains(v))
            && self.opacity.is_finite()
            && (0. ..=1.).contains(&self.opacity)
            && self
                .color
                .iter()
                .all(|v| v.is_finite() && (0. ..=1.).contains(v))
            && self
                .transform
                .is_none_or(|t| t.valid(self.rect[2], self.rect[3]))
    }
    pub fn apply(&self, node: &mut Node) {
        [node.x, node.y, node.width, node.height] = self.rect;
        node.opacity = self.opacity;
        node.color = self.color;
        node.sprite_transform = self.transform;
    }
}
impl SpriteTimeline {
    pub fn valid(&self) -> bool {
        !self.id.is_empty()
            && self.id.len() <= 256
            && self.duration_us.0 > 0
            && self.duration_us.0 <= 3_600_000_000
            && !self.tracks.is_empty()
            && self.tracks.len() <= 256
            && self
                .tracks
                .iter()
                .map(|t| t.node.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.tracks.len()
            && self.tracks.iter().map(|t| t.frames.len()).sum::<usize>() <= 65_536
            && self.tracks.iter().all(|track| {
                !track.node.is_empty()
                    && track.node.len() <= 256
                    && !track.frames.is_empty()
                    && track.frames[0].at_us.0 == 0
                    && track
                        .frames
                        .iter()
                        .all(|frame| frame.valid() && frame.at_us.0 <= self.duration_us.0)
                    && track
                        .frames
                        .windows(2)
                        .all(|pair| pair[0].at_us < pair[1].at_us)
            })
    }
    pub fn apply(&self, nodes: &mut [Node], elapsed_us: u64) {
        let elapsed_us = elapsed_us.min(self.duration_us.0);
        for track in &self.tracks {
            let next = track
                .frames
                .partition_point(|frame| frame.at_us.0 <= elapsed_us);
            if let (Some(frame), Some(node)) = (
                track.frames.get(next.saturating_sub(1)),
                nodes.iter_mut().find(|node| node.id == track.node),
            ) {
                frame.apply(node);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyframes_are_discrete_bounded_and_binary_search_handles_exact_boundaries() {
        let frame = |at, x| SpriteKeyframe {
            at_us: Micros(at),
            rect: [x, 2., 32., 24.],
            opacity: 1.,
            color: [1.; 4],
            transform: None,
        };
        let timeline = SpriteTimeline {
            id: "neutral".into(),
            duration_us: Micros(1000),
            tracks: vec![SpriteTimelineTrack {
                node: "sprite".into(),
                frames: vec![frame(0, 0.), frame(9, 1.), frame(26, 2.), frame(1000, 3.)],
            }],
        };
        assert!(timeline.valid());
        let node: Node =
            serde_json::from_str(r#"{"id":"sprite","x":0,"y":0,"width":32,"height":24}"#).unwrap();
        for (time, expected) in [
            (0, 0.),
            (8, 0.),
            (9, 1.),
            (25, 1.),
            (26, 2.),
            (999, 2.),
            (1000, 3.),
            (2000, 3.),
        ] {
            let mut nodes = vec![node.clone()];
            timeline.apply(&mut nodes, time);
            assert_eq!(nodes[0].x, expected);
        }
        let mut bad = timeline.clone();
        bad.tracks[0].frames[1].at_us = Micros(0);
        assert!(!bad.valid());
        let mut bad = timeline.clone();
        bad.tracks[0].frames[1].rect[0] = f32::NAN;
        assert!(!bad.valid());
        let mut bad = timeline.clone();
        bad.tracks.push(bad.tracks[0].clone());
        assert!(!bad.valid());
        assert_eq!(
            serde_json::from_str::<SpriteTimeline>(&serde_json::to_string(&timeline).unwrap())
                .unwrap(),
            timeline
        );
    }
}
