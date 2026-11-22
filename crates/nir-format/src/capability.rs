//! Strongly typed capability identifiers and O(1) bitset storage.
use crate::{Diagnostic, Result};

macro_rules! define_capabilities {
    ($($variant:ident => $name:literal),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u8)]
        pub enum Capability {
            $($variant,)*
        }

        impl Capability {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }

            pub fn parse(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$variant),)*
                    _ => None,
                }
            }

            pub const fn bit(self) -> u8 {
                self as u8
            }
        }
    };
}

define_capabilities! {
    ModuleLazy => "module.lazy.v1",
    Control => "control.v1",
    ControlAdvanceWait => "control.advance-wait.v1",
    StoryProfileRead => "story.profile-read.v1",
    StoryProfileValue => "story.profile-value.v1",
    StoryFloat80 => "story.float80.v1",
    StoryCodePackages => "story.code-packages.v1",
    StageSprite => "stage.sprite.v1",
    StageBitmapText => "stage.bitmap-text.v1",
    StageSpriteContinuity => "stage.sprite-continuity.v1",
    StageDialogueShake => "stage.dialogue-shake.v1",
    StageSpriteWave => "stage.sprite-wave.v1",
    StageSpriteTransform => "stage.sprite-transform.v1",
    StageSpriteTimeline => "stage.sprite-timeline.v1",
    StageWindowFlip => "stage.window-flip.v1",
    StageInheritImage => "stage.inherit-image.v1",
    StageInheritImageGeometry => "stage.inherit-image-geometry.v1",
    StageSpriteShake => "stage.sprite-shake.v1",
    StageSpriteQuake => "stage.sprite-quake.v1",
    StageSpriteLifecycle => "stage.sprite-lifecycle.v1",
    StageSourceMotion => "stage.source-motion.v1",
    StageSourceOpacity => "stage.source-opacity.v1",
    DialogueStyle => "dialogue.style.v1",
    DialogueDecoration => "dialogue.decoration.v1",
    UiStoryModal => "ui.story-modal.v1",
    StageDissolve => "stage.dissolve.v1",
    ClipScalar => "clip.scalar.v1",
    TweenTarget => "tween.target.v1",
    TextStructured => "text.structured.v1",
    TextRevisions => "text.revisions.v1",
    TextGate => "text.gate.v1",
    Choice => "choice.v1",
    ChoiceImage => "choice.image.v1",
    ChoiceDisabledImage => "choice.disabled-image.v1",
    AudioBuffer => "audio.buffer.v1",
    AudioGain => "audio.gain.v1",
    AudioStop => "audio.stop.v1",
    AudioPause => "audio.pause.v1",
    AudioGainTween => "audio.gain-tween.v1",
    AudioLoopRegion => "audio.loop-region.v1",
    UiImageMenu => "ui.image-menu.v1",
    TextVisibility => "text.visibility.v1",
    TextWindowTransition => "text.window-transition.v1",
    TextVoiceBinding => "text.voice-binding.v1",
    TextVoiceTimer => "text.voice-timer.v1",
    PlayerHidePolicy => "player.hide-policy.v1",
    PlayerAutoDelayPolicy => "player.auto-delay-policy.v1",
    TextShadow => "text.shadow.v1",
    TextRect => "text.rect.v1",
    TextPause => "text.pause.v1",
    TextRuby => "text.ruby.v1",
    TextInlineImage => "text.inline-image.v1",
    PlayerMenuAccess => "player.menu-access.v1",
    UiMenuPages => "ui.menu-pages.v1",
    StageWipe => "stage.wipe.v1",
    StageMask => "stage.mask.v1",
    UiMenuElements => "ui.menu-elements.v1",
    UiMenuState => "ui.menu-state.v1",
    UiMenuServices => "ui.menu-services.v1",
    UiMenuNavigation => "ui.menu-navigation.v1",
    UiMenuChrome => "ui.menu-chrome.v1",
    UiMenuHistoryAvailability => "ui.menu-history-availability.v1",
    UiMenuReading => "ui.menu-reading.v1",
    UiMenuStory => "ui.menu-story.v1",
    UiMenuStack => "ui.menu-stack.v1",
    UiMenuTextButton => "ui.menu-text-button.v1",
    UiMenuStorage => "ui.menu-storage.v1",
    UiMenuHistory => "ui.menu-history.v1",
    UiMenuHistoryFlow => "ui.menu-history-flow.v1",
    UiMenuHistoryVoice => "ui.menu-history-voice.v1",
    UiMenuHistoryScrollbar => "ui.menu-history-scrollbar.v1",
    UiMenuValues => "ui.menu-values.v1",
    UiMenuEffects => "ui.menu-effects.v1",
    UiMenuTransition => "ui.menu-transition.v1",
    UiMenuElementTween => "ui.menu-element-tween.v1",
    UiReplay => "ui.replay.v1",
    TaskCompose => "task.compose.v1",
    StoryTypedResult => "story.typed-result.v1",
    MediaWebp => "media.webp.v1",
    MediaMp3 => "media.mp3.v1",
    ContentInternedJson => "content.interned-json.v1",
}

/// Fixed-size 128-bit set of declared capabilities with O(1) membership checks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CapabilitySet([u64; 2]);

impl CapabilitySet {
    pub const fn empty() -> Self {
        Self([0; 2])
    }

    pub const fn insert(&mut self, cap: Capability) {
        let bit = cap.bit() as usize;
        self.0[bit / 64] |= 1u64 << (bit % 64);
    }

    pub const fn remove(&mut self, cap: Capability) {
        let bit = cap.bit() as usize;
        self.0[bit / 64] &= !(1u64 << (bit % 64));
    }

    pub const fn contains(self, cap: Capability) -> bool {
        let bit = cap.bit() as usize;
        (self.0[bit / 64] & (1u64 << (bit % 64))) != 0
    }

    pub fn contains_name(self, name: &str) -> bool {
        Capability::parse(name).is_some_and(|cap| self.contains(cap))
    }

    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<Self> {
        let mut set = Self::empty();
        for name in names {
            let cap = Capability::parse(name)
                .ok_or_else(|| Diagnostic::new("E_CAPABILITY", "requires", name))?;
            set.insert(cap);
        }
        Ok(set)
    }

    pub fn from_names_known<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut set = Self::empty();
        for name in names {
            if let Some(cap) = Capability::parse(name) {
                set.insert(cap);
            }
        }
        set
    }

    /// Materializes declared capabilities in canonical [`CAPABILITIES`] order.
    pub fn to_vec(self) -> Vec<String> {
        let mut out = Vec::new();
        for &cap in Capability::ALL {
            if self.contains(cap) {
                out.push(cap.as_str().to_owned());
            }
        }
        out
    }

    /// Returns the baseline capabilities historically emitted unconditionally
    /// before per-container release pruning (`media.*`, `content.*`, `story.code-packages`).
    pub const fn baseline() -> Self {
        let mut set = Self::empty();
        set.insert(Capability::ModuleLazy);
        set.insert(Capability::Control);
        set.insert(Capability::StoryCodePackages);
        set.insert(Capability::StageSprite);
        set.insert(Capability::StageDissolve);
        set.insert(Capability::ClipScalar);
        set.insert(Capability::TextStructured);
        set.insert(Capability::TextRevisions);
        set.insert(Capability::TextGate);
        set.insert(Capability::Choice);
        set.insert(Capability::AudioBuffer);
        set.insert(Capability::UiImageMenu);
        set.insert(Capability::TextVisibility);
        set.insert(Capability::MediaWebp);
        set.insert(Capability::MediaMp3);
        set.insert(Capability::ContentInternedJson);
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CAPABILITIES;

    #[test]
    fn capability_enum_matches_advertised_table_in_exact_order() {
        let enum_names: Vec<&str> = Capability::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(enum_names.as_slice(), CAPABILITIES);
        assert!(Capability::ALL.len() <= 128);
        for &cap in Capability::ALL {
            assert_eq!(Capability::parse(cap.as_str()), Some(cap));
        }
        assert_eq!(Capability::parse("unknown.v99"), None);
    }

    #[test]
    fn capability_set_round_trips_and_rejects_unknown() {
        let mut set = CapabilitySet::empty();
        set.insert(Capability::StageMask);
        set.insert(Capability::Control);
        assert!(set.contains(Capability::Control));
        assert!(set.contains(Capability::StageMask));
        assert!(!set.contains(Capability::UiReplay));
        assert_eq!(set.to_vec(), vec!["control.v1", "stage.mask.v1"]);
        assert_eq!(
            CapabilitySet::from_names(["stage.mask.v1", "control.v1"]).unwrap(),
            set
        );
        assert!(CapabilitySet::from_names(["bogus.v1"]).is_err());
    }
}
