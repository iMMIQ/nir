use nir_format::Preferences;
use std::collections::{BTreeMap, BTreeSet};

macro_rules! preference_fields {
    ($visit:ident) => {
        $visit!(
            ui_locale,
            text_locale,
            font_scale,
            text_speed,
            auto_wait_scale,
            auto_wait_voice,
            voice_continue,
            bgm_volume,
            voice_volume,
            sfx_volume,
            reduced_motion
        );
    };
}

// Track user edits on the owner, including changes back to the starting value.
// A delayed disk read cannot infer that history from two snapshots alone.
pub(crate) struct PreferenceRecovery {
    last: Preferences,
    fields: BTreeSet<&'static str>,
    characters: BTreeMap<String, (bool, bool)>,
}
impl PreferenceRecovery {
    pub(crate) fn new(preferences: &Preferences) -> Self {
        Self {
            last: preferences.clone(),
            fields: BTreeSet::new(),
            characters: BTreeMap::new(),
        }
    }
    pub(crate) fn applied(&mut self, preferences: &Preferences) {
        self.last = preferences.clone();
    }
    pub(crate) fn edited(&mut self, preferences: &Preferences) {
        macro_rules! remember {
            ($($field:ident),*) => {$(if self.last.$field != preferences.$field {self.fields.insert(stringify!($field));})*};
        }
        preference_fields!(remember);
        for id in self
            .last
            .character_voices
            .keys()
            .chain(preferences.character_voices.keys())
        {
            let before = self
                .last
                .character_voices
                .get(id)
                .copied()
                .unwrap_or_default();
            let after = preferences
                .character_voices
                .get(id)
                .copied()
                .unwrap_or_default();
            if before != after {
                let changed = self.characters.entry(id.clone()).or_default();
                changed.0 |= before.volume != after.volume;
                changed.1 |= before.muted != after.muted;
            }
        }
        self.applied(preferences);
    }
    pub(crate) fn has_edits(&self) -> bool {
        !self.fields.is_empty() || !self.characters.is_empty()
    }
    pub(crate) fn merge(&self, mut saved: Preferences, current: &Preferences) -> Preferences {
        fn cloned<T: Clone>(value: &T) -> T {
            value.clone()
        }
        macro_rules! preserve {
            ($($field:ident),*) => {$(if self.fields.contains(stringify!($field)) {saved.$field = cloned(&current.$field);})*};
        }
        preference_fields!(preserve);
        for (id, &(volume, muted)) in &self.characters {
            let current = current
                .character_voices
                .get(id)
                .copied()
                .unwrap_or_default();
            let recovered = saved.character_voices.entry(id.clone()).or_default();
            if volume {
                recovered.volume = current.volume;
            }
            if muted {
                recovered.muted = current.muted;
            }
        }
        saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nir_format::CharacterVoicePreference;
    #[test]
    fn edits_back_to_initial_values_still_win_while_untouched_fields_recover() {
        let mut current = Preferences::default();
        let mut recovery = PreferenceRecovery::new(&current);
        current.bgm_volume = 0.1;
        recovery.edited(&current);
        current.bgm_volume = 0.3;
        recovery.edited(&current);
        let saved = Preferences {
            bgm_volume: 0.9,
            font_scale: 1.4,
            voice_volume: 0.2,
            ..Preferences::default()
        };
        let merged = recovery.merge(saved, &current);
        assert_eq!(merged.bgm_volume, 0.3);
        assert_eq!(merged.font_scale, 1.4);
        assert_eq!(merged.voice_volume, 0.2);
        assert!(recovery.has_edits());
    }
    #[test]
    fn per_character_volume_and_mute_recover_independently_including_resets() {
        let mut current = Preferences::default();
        let mut recovery = PreferenceRecovery::new(&current);
        current.character_voices.insert(
            "aki".into(),
            CharacterVoicePreference {
                volume: 1.,
                muted: true,
            },
        );
        recovery.edited(&current);
        current.character_voices.insert(
            "ren".into(),
            CharacterVoicePreference {
                volume: 0.4,
                muted: false,
            },
        );
        recovery.edited(&current);
        current.character_voices.remove("ren");
        recovery.edited(&current);
        let saved = Preferences {
            character_voices: BTreeMap::from([
                (
                    "aki".into(),
                    CharacterVoicePreference {
                        volume: 0.25,
                        muted: false,
                    },
                ),
                (
                    "ren".into(),
                    CharacterVoicePreference {
                        volume: 0.2,
                        muted: true,
                    },
                ),
                (
                    "other".into(),
                    CharacterVoicePreference {
                        volume: 0.6,
                        muted: false,
                    },
                ),
            ]),
            ..Preferences::default()
        };
        let merged = recovery.merge(saved, &current);
        assert_eq!(
            merged.character_voices["aki"],
            CharacterVoicePreference {
                volume: 0.25,
                muted: true
            }
        );
        assert_eq!(
            merged.character_voices["ren"],
            CharacterVoicePreference {
                volume: 1.,
                muted: true
            }
        );
        assert_eq!(merged.character_voices["other"].volume, 0.6);
    }
    #[test]
    fn applying_host_preferences_is_not_a_user_edit() {
        let mut current = Preferences::default();
        let mut recovery = PreferenceRecovery::new(&current);
        current.font_scale = 1.2;
        recovery.applied(&current);
        current.sfx_volume = 0.1;
        recovery.edited(&current);
        let saved = Preferences {
            font_scale: 1.4,
            sfx_volume: 0.9,
            ..Preferences::default()
        };
        let merged = recovery.merge(saved, &current);
        assert_eq!(merged.font_scale, 1.4);
        assert_eq!(merged.sfx_volume, 0.1);
    }
}
