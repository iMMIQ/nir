use fluent_bundle::{FluentBundle, FluentResource};
use unic_langid::LanguageIdentifier;

pub struct Messages {
    en: FluentBundle<FluentResource>,
    zh: FluentBundle<FluentResource>,
}

impl Default for Messages {
    fn default() -> Self {
        let build = |lang: &str, src: &str| {
            let id: LanguageIdentifier = lang.parse().unwrap();
            let mut bundle = FluentBundle::new(vec![id]);
            bundle.set_use_isolating(false);
            if let Ok(res) = FluentResource::try_new(src.to_owned()) {
                let _ = bundle.add_resource(res);
            }
            bundle
        };
        Self {
            en: build("en", nir_ir::ui_messages::EN_FTL),
            zh: build("zh-Hans", nir_ir::ui_messages::ZH_HANS_FTL),
        }
    }
}

impl Messages {
    pub fn get(&self, locale: &str, key: &str) -> String {
        let bundle = if locale.starts_with("zh") {
            &self.zh
        } else {
            &self.en
        };
        bundle
            .get_message(key)
            .and_then(|m| m.value())
            .map(|p| {
                let mut errs = Vec::new();
                bundle.format_pattern(p, None, &mut errs).into_owned()
            })
            .unwrap_or_else(|| key.to_owned())
    }
}
