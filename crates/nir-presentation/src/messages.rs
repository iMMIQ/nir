use super::*;

pub struct Messages {
    pub(super) en: FluentBundle<FluentResource>,
    pub(super) zh: FluentBundle<FluentResource>,
    pub(super) keys: Vec<String>,
}
impl Default for Messages {
    fn default() -> Self {
        fn bundle(loc: &str, src: &str) -> FluentBundle<FluentResource> {
            let mut b = FluentBundle::new(vec![loc.parse().unwrap()]);
            b.add_resource(
                FluentResource::try_new(src.to_owned()).expect("bundled messages parse"),
            )
            .expect("unique bundled messages");
            b
        }
        let en_src = nir_format::ui_messages::EN_FTL;
        let en = bundle("en", en_src);
        let zh = bundle("zh-Hans", nir_format::ui_messages::ZH_HANS_FTL);
        let keys = en_src
            .lines()
            .filter_map(|line| {
                let id = line.split('=').next()?.trim();
                (!id.is_empty() && !id.starts_with('#') && en.has_message(id))
                    .then(|| id.to_owned())
            })
            .collect();
        Self { en, zh, keys }
    }
}
impl Messages {
    pub fn preflight(&self, locale: &str) -> Vec<String> {
        self.keys.iter().map(|id| self.text(locale, id)).collect()
    }
    pub fn diagnostic(&self, diagnostic: &Diagnostic, locale: &str) -> String {
        if diagnostic.code == "E_STORAGE_UNCERTAIN" && diagnostic.location == "save" {
            return self.text(locale, "save-pending");
        }
        let key = match diagnostic.details.as_ref().map(|d| &d.domain) {
            Some(ErrorDomain::Prepare) => "error-prepare",
            Some(ErrorDomain::Storage) => "error-storage",
            Some(ErrorDomain::Render) => "error-render",
            Some(ErrorDomain::Core | ErrorDomain::Content) => "error-content",
            _ => "error-host",
        };
        format!("{}: {}", diagnostic.code, self.text(locale, key))
    }
    pub fn text(&self, locale: &str, id: &str) -> String {
        let b = if locale == "zh-Hans" {
            &self.zh
        } else {
            &self.en
        };
        let b = if b.has_message(id) { b } else { &self.en };
        b.get_message(id)
            .and_then(|m| m.value())
            .map(|p| b.format_pattern(p, None, &mut vec![]).into_owned())
            .unwrap_or_else(|| id.into())
    }
}
