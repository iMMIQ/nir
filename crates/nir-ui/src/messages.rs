use fluent_bundle::{FluentBundle, FluentResource};
use std::collections::HashMap;
use unic_langid::LanguageIdentifier;

/// Pre-formatted static UI message table (zero Fluent formatting or Vec/String allocation per frame!).
pub struct Messages {
    en: HashMap<String, String>,
    zh: HashMap<String, String>,
}

fn build_cache(lang: &str, src: &str) -> HashMap<String, String> {
    let id: LanguageIdentifier = lang.parse().unwrap();
    let mut bundle = FluentBundle::new(vec![id]);
    bundle.set_use_isolating(false);
    if let Ok(res) = FluentResource::try_new(src.to_owned()) {
        let _ = bundle.add_resource(res);
    }
    let mut map = HashMap::new();
    let mut errs = Vec::new();
    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, _)) = trimmed.split_once('=') {
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            if let Some(val) = bundle.get_message(key).and_then(|m| m.value()) {
                errs.clear();
                let formatted = bundle.format_pattern(val, None, &mut errs).into_owned();
                map.insert(key.to_owned(), formatted);
            }
        }
    }
    map
}

impl Default for Messages {
    fn default() -> Self {
        Self {
            en: build_cache("en", nir_ir::ui_messages::EN_FTL),
            zh: build_cache("zh-Hans", nir_ir::ui_messages::ZH_HANS_FTL),
        }
    }
}

impl Messages {
    #[inline]
    pub fn get_str<'a>(&'a self, locale: &str, key: &'a str) -> &'a str {
        let table = if locale.starts_with("zh") {
            &self.zh
        } else {
            &self.en
        };
        table.get(key).map_or(key, String::as_str)
    }

    #[inline]
    pub fn get(&self, locale: &str, key: &str) -> String {
        self.get_str(locale, key).to_owned()
    }
}
