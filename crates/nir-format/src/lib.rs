//! Versioned, platform-independent wire contracts. Unknown semantic fields fail closed.
#![forbid(unsafe_code)]
mod capability;
pub mod lame;
pub use capability::{Capability, CapabilitySet};
pub mod ui_messages {
    pub const EN_FTL: &str = include_str!("../messages/en.ftl");
    pub const ZH_HANS_FTL: &str = include_str!("../messages/zh-Hans.ftl");
}
mod menu;
mod timeline;
mod transition;
mod tween;
pub use menu::{
    MenuCondition, MenuContent, MenuEffects, MenuElement, MenuElementProperty, MenuElementTween,
    MenuImageStates, MenuLocal, MenuMusic, MenuPreference, MenuRangeBinding, MenuSlot,
    MenuToggleBinding, MenuTransition, MenuValue, MenuValueInput, MAX_MENU_PARENTS,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub use timeline::{SpriteKeyframe, SpriteTimeline, SpriteTimelineTrack};
pub use transition::{MaskChannel, StageTransition, WipeDirection};
pub use tween::{interpolate, DialogueAppearance, ScalarTween, SourceMotionCurve};

mod float80;
pub use float80::Float80;

pub const FORMAT_VERSION: u32 = 1;
/// Wire version for the indexed, lazily loaded runtime root. Source `Program`
/// documents deliberately keep `FORMAT_VERSION` so authoring formats can evolve
/// independently from the runtime package layout.
pub const RUNTIME_FORMAT_VERSION: u32 = 2;
pub const CONTENT_PACKAGE_VERSION: u32 = 2;
pub const SNAPSHOT_VERSION: u32 = 2;
pub const CAPABILITIES: &[&str] = &[
    "module.lazy.v1",
    "control.v1",
    "control.advance-wait.v1",
    "story.profile-read.v1",
    "story.profile-value.v1",
    "story.float80.v1",
    "story.code-packages.v1",
    "stage.sprite.v1",
    "stage.bitmap-text.v1",
    "stage.sprite-continuity.v1",
    "stage.dialogue-shake.v1",
    "stage.sprite-wave.v1",
    "stage.sprite-transform.v1",
    "stage.sprite-timeline.v1",
    "stage.window-flip.v1",
    "stage.inherit-image.v1",
    "stage.inherit-image-geometry.v1",
    "stage.sprite-shake.v1",
    "stage.sprite-quake.v1",
    "stage.sprite-lifecycle.v1",
    "stage.source-motion.v1",
    "stage.source-opacity.v1",
    "dialogue.style.v1",
    "dialogue.decoration.v1",
    "ui.story-modal.v1",
    "stage.dissolve.v1",
    "clip.scalar.v1",
    "tween.target.v1",
    "text.structured.v1",
    "text.revisions.v1",
    "text.gate.v1",
    "choice.v1",
    "choice.image.v1",
    "choice.disabled-image.v1",
    "audio.buffer.v1",
    "audio.gain.v1",
    "audio.stop.v1",
    "audio.pause.v1",
    "audio.gain-tween.v1",
    "audio.loop-region.v1",
    "ui.image-menu.v1",
    "text.visibility.v1",
    "text.window-transition.v1",
    "text.voice-binding.v1",
    "text.voice-timer.v1",
    "player.hide-policy.v1",
    "player.auto-delay-policy.v1",
    "text.shadow.v1",
    "text.rect.v1",
    "text.pause.v1",
    "text.ruby.v1",
    "text.inline-image.v1",
    "player.menu-access.v1",
    "ui.menu-pages.v1",
    "stage.wipe.v1",
    "stage.mask.v1",
    "ui.menu-elements.v1",
    "ui.menu-state.v1",
    "ui.menu-services.v1",
    "ui.menu-navigation.v1",
    "ui.menu-chrome.v1",
    "ui.menu-history-availability.v1",
    "ui.menu-reading.v1",
    "ui.menu-story.v1",
    "ui.menu-stack.v1",
    "ui.menu-text-button.v1",
    "ui.menu-storage.v1",
    "ui.menu-history.v1",
    "ui.menu-history-flow.v1",
    "ui.menu-history-voice.v1",
    "ui.menu-history-scrollbar.v1",
    "ui.menu-values.v1",
    "ui.menu-effects.v1",
    "ui.menu-transition.v1",
    "ui.menu-element-tween.v1",
    "ui.replay.v1",
    "task.compose.v1",
    "story.typed-result.v1",
    "media.webp.v1",
    "media.mp3.v1",
    "content.interned-json.v1",
];
/// Device observation, separate from the deterministic Story task clock.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<AudioEnvelopePosition>,
    pub task: u32,
    pub position_us: Micros,
}

/// Device envelope progress belongs to a concrete AudioStop task, not a bus.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AudioEnvelopePosition {
    pub owner: u32,
    pub elapsed_us: Micros,
}

pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TASKS: usize = 256;
pub const MAX_FRAMES: usize = 64;
pub const MAX_NODES: usize = 1024;

/// Host clocks and pause routes; not an additional story execution context.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TimeDomain {
    Story,
    ForegroundUi,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(with = "String"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Micros(pub u64);
impl Serialize for Micros {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let mut buf = [0u8; 20];
        let mut n = self.0;
        let mut idx = buf.len();
        loop {
            idx -= 1;
            buf[idx] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        // SAFETY-free: digits are strictly ASCII.
        let s = std::str::from_utf8(&buf[idx..]).expect("ASCII decimal digits");
        serializer.serialize_str(s)
    }
}
impl<'de> Deserialize<'de> for Micros {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct MicrosVisitor;
        impl serde::de::Visitor<'_> for MicrosVisitor {
            type Value = Micros;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("unsigned decimal microsecond string")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<Micros, E> {
                if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(E::custom("E_TIME: expected unsigned decimal string"));
                }
                s.parse::<u64>()
                    .map(Micros)
                    .map_err(|_| E::custom("E_TIME: u64 overflow"))
            }
        }
        deserializer.deserialize_str(MicrosVisitor)
    }
}
impl TryFrom<String> for Micros {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err("E_TIME: expected unsigned decimal string".into());
        }
        s.parse()
            .map(Self)
            .map_err(|_| "E_TIME: u64 overflow".into())
    }
}
impl From<Micros> for String {
    fn from(v: Micros) -> Self {
        v.0.to_string()
    }
}
impl Micros {
    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

mod action;
mod diagnostics;
mod effect;
mod program;
mod release;
mod stage;
mod text;
mod theme;
mod value;
pub use action::*;
pub use diagnostics::*;
use effect::is_unit_gain;
pub use effect::*;
pub use program::*;
pub use release::*;
pub use stage::*;
use stage::{is_false, one, white};
pub use text::*;
pub use theme::*;
pub use value::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_nodes_preserve_legacy_defaults_and_authored_values() {
        let legacy = serde_json::json!({"id":"sprite","parent":null,"asset":null,
            "x":10,"y":20,"width":30,"height":40,"scale":1.0,"opacity":1.0,
            "color":[1.0,1.0,1.0,1.0],"order":0,"clip":null});
        let node: Node = serde_json::from_value(legacy).unwrap();
        let sparse = serde_json::to_value(&node).unwrap();
        assert_eq!(sparse.as_object().unwrap().len(), 5);
        let restored: Node = serde_json::from_value(sparse).unwrap();
        assert_eq!(restored, node);
        let mut authored = node;
        authored.parent = Some("parent".into());
        authored.asset = Some("image".into());
        authored.scale = 0.5;
        authored.opacity = 0.25;
        authored.color = [0., 0.25, 0.5, 0.75];
        authored.order = -2;
        authored.clip = Some([1., 2., 3., 4.]);
        assert_eq!(
            serde_json::from_value::<Node>(serde_json::to_value(&authored).unwrap()).unwrap(),
            authored
        );
    }

    /// text.window-transition.v1: the styled reveal fields are optional and
    /// omit cleanly; the legacy two-field op still deserializes unchanged.
    #[test]
    fn dialogue_visibility_transition_fields_roundtrip_and_default_away() {
        let legacy: Operation = serde_json::from_value(serde_json::json!({
            "type":"dialogue_visibility","visible":false
        }))
        .unwrap();
        match &legacy {
            Operation::DialogueVisibility {
                visible,
                transition,
                duration_us,
            } => {
                assert!(!*visible);
                assert_eq!(transition, &None);
                assert_eq!(*duration_us, Micros(0));
            }
            _ => panic!("wrong variant"),
        }
        assert_eq!(
            serde_json::to_value(&legacy).unwrap(),
            serde_json::json!({"type":"dialogue_visibility","visible":false})
        );
        let styled: Operation = serde_json::from_value(serde_json::json!({
            "type":"dialogue_visibility","visible":true,
            "transition":{"type":"dissolve"},
            "duration_us":"1000000"
        }))
        .unwrap();
        match &styled {
            Operation::DialogueVisibility {
                visible,
                transition,
                duration_us,
            } => {
                assert!(*visible);
                assert_eq!(transition, &Some(StageTransition::Dissolve));
                assert_eq!(*duration_us, Micros(1_000_000));
            }
            _ => panic!("wrong variant"),
        }
        assert_eq!(
            serde_json::to_value(&styled).unwrap(),
            serde_json::json!({
                "type":"dialogue_visibility","visible":true,
                "transition":{"type":"dissolve"},
                "duration_us":"1000000"
            })
        );
    }

    #[test]
    fn micros_roundtrip_zero_alloc_and_rejects_invalid_strings() {
        for value in [0u64, 1, 42, 1_200_000, u64::MAX] {
            let m = Micros(value);
            let encoded = serde_json::to_string(&m).unwrap();
            assert_eq!(encoded, format!("\"{value}\""));
            let decoded: Micros = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, m);
        }
        assert!(serde_json::from_str::<Micros>("\"-1\"").is_err());
        assert!(serde_json::from_str::<Micros>("\"18446744073709551616\"").is_err());
        assert!(serde_json::from_str::<Micros>("123").is_err());
    }
}
