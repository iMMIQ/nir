//! Character ownership comes from the authored speaker reference and an
//! explicit DialogueVoice binding, never a translated name or audio filename.
use super::*;
impl Player {
    pub(super) fn character_voice_views(c: &Core) -> Vec<nir_presentation::CharacterVoiceView> {
        let mut roles = BTreeMap::new();
        let mut add = |id: &str, name: &str, locale: &str| {
            if id.is_empty()
                || id.len() > MAX_CHARACTER_ID_BYTES
                || name.is_empty()
                || (roles.len() >= MAX_CHARACTER_VOICES && !roles.contains_key(id))
            {
                return;
            }
            if let Some(plan) = c.program().locale_config.text.get(locale) {
                roles.entry(id.to_owned()).or_insert_with(|| {
                    nir_presentation::CharacterVoiceView {
                        id: id.to_owned(),
                        name: name.to_owned(),
                        locale: locale.to_owned(),
                        fonts: plan.fonts.clone(),
                    }
                });
            }
        };
        // The current frozen name wins; history supplies other known voices
        // without loading old chapter text bodies or revealing future roles.
        for task in c.state().tasks.values().rev() {
            if let Some(d) = &task.dialogue {
                if d.reading
                    .as_ref()
                    .is_some_and(|r| r.voice.is_some() || !r.active_voices.is_empty())
                {
                    add(&d.speaker_id, &d.speaker, &d.locale);
                }
            }
        }
        for entry in c
            .state()
            .history
            .iter()
            .rev()
            .filter(|e| !e.voices.is_empty())
        {
            add(&entry.speaker_id, &entry.speaker, &entry.locale);
        }
        roles.into_values().collect()
    }
    pub(super) fn character_known(&self, id: &str) -> bool {
        !id.is_empty()
            && id.len() <= MAX_CHARACTER_ID_BYTES
            && Self::character_voice_views(&self.core)
                .iter()
                .any(|role| role.id == id)
    }
    pub(super) fn edit_character_voice(
        &mut self,
        id: String,
        delta: Option<f32>,
        muted: Option<bool>,
    ) {
        if self.is_loading()
            || self.locale_pending()
            || !self.character_known(&id)
            || delta.is_some_and(|d| !d.is_finite() || d.abs() > 1.)
            || (self.preferences.character_voices.len() >= MAX_CHARACTER_VOICES
                && !self.preferences.character_voices.contains_key(&id))
        {
            return;
        }
        let voice = self.preferences.character_voices.entry(id).or_default();
        if let Some(delta) = delta {
            voice.volume = (voice.volume + delta).clamp(0., 1.);
        }
        if let Some(muted) = muted {
            voice.muted = muted;
        }
        if (voice.volume - 1.).abs() < 1e-6 {
            voice.volume = 1.;
        }
        if *voice == CharacterVoicePreference::default() {
            self.preferences
                .character_voices
                .retain(|_, value| *value != CharacterVoicePreference::default());
        }
        self.persist_preferences();
    }
}
