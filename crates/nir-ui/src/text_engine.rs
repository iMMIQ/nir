use crate::TextRun;
use nir_ir::AssetId;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// Zero-alloc-key CPU text shaping cache backed by `cosmic-text`.
pub struct TextEngine {
    pub fonts: cosmic_text::FontSystem,
    pub buffers: HashMap<u64, cosmic_text::Buffer>,
    recency: HashMap<u64, u64>,
    clock: u64,
    families: BTreeMap<AssetId, &'static str>,
    configured_fallback: Vec<&'static str>,
    pub shapes: u64,
}

fn intern_family(name: &str) -> &'static str {
    static INTERNED: OnceLock<Mutex<std::collections::BTreeSet<&'static str>>> = OnceLock::new();
    let table = INTERNED.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()));
    let mut guard = table.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&existing) = guard.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    guard.insert(leaked);
    leaked
}

struct ExplicitFallback {
    families: Vec<&'static str>,
}

impl cosmic_text::Fallback for ExplicitFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &self.families
    }
    fn forbidden_fallback(&self) -> &[&'static str] {
        &[]
    }
    fn script_fallback(&self, _script: unicode_script::Script, _locale: &str) -> &[&'static str] {
        &self.families
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut db = cosmic_text::fontdb::Database::new();
        // On native platforms, system fonts act as graceful fallback if a game
        // ships without bundled fonts during rapid prototyping.
        #[cfg(not(target_arch = "wasm32"))]
        db.load_system_fonts();
        Self {
            fonts: cosmic_text::FontSystem::new_with_locale_and_db("zh-Hans".into(), db),
            buffers: HashMap::new(),
            recency: HashMap::new(),
            clock: 0,
            families: BTreeMap::new(),
            configured_fallback: Vec::new(),
            shapes: 0,
        }
    }
}

impl TextEngine {
    pub fn add_font_asset(&mut self, id: AssetId, bytes: Vec<u8>) -> Result<(), String> {
        let before: HashSet<_> = self.fonts.db().faces().map(|f| f.id).collect();
        self.fonts.db_mut().load_font_data(bytes);
        let family = self
            .fonts
            .db()
            .faces()
            .find(|face| !before.contains(&face.id))
            .and_then(|face| face.families.first().map(|f| f.0.clone()))
            .ok_or_else(|| format!("E_FONT: no font face in asset {}", id.0))?;
        let static_family = intern_family(&family);
        self.families.insert(id, static_family);
        if self.families.len() == 1 {
            self.fonts.db_mut().set_sans_serif_family(family);
        }
        self.buffers.clear();
        self.recency.clear();
        self.configured_fallback.clear();
        Ok(())
    }

    pub fn has_font(&self, id: AssetId) -> bool {
        self.families.contains_key(&id)
    }

    pub fn line_offsets(text: &str, rich: bool) -> Vec<usize> {
        let mut offsets: Vec<_> = if !rich {
            cosmic_text::LineIter::new(text)
                .map(|(range, _)| range.start)
                .collect()
        } else {
            cosmic_text::BidiParagraphs::new(text)
                .map(|line| line.as_ptr() as usize - text.as_ptr() as usize)
                .collect()
        };
        if offsets.is_empty() {
            offsets.push(0);
        }
        offsets
    }

    pub fn layout_texts(&mut self, runs: &[TextRun]) {
        const TARGET_ENTRIES: usize = 192;
        for r in runs {
            if self.buffers.contains_key(&r.cache_key) {
                self.clock = self.clock.wrapping_add(1);
                self.recency.insert(r.cache_key, self.clock);
                continue;
            }
            let explicit: Vec<&'static str> = r
                .font_assets
                .iter()
                .filter_map(|id| self.families.get(id).copied())
                .collect();
            if !explicit.is_empty() && self.configured_fallback != explicit {
                let old = std::mem::replace(
                    &mut self.fonts,
                    cosmic_text::FontSystem::new_with_locale_and_db(
                        "en".into(),
                        cosmic_text::fontdb::Database::new(),
                    ),
                );
                let (locale, db) = old.into_locale_and_db();
                self.fonts = cosmic_text::FontSystem::new_with_locale_and_db_and_fallback(
                    locale,
                    db,
                    ExplicitFallback {
                        families: explicit.clone(),
                    },
                );
                self.configured_fallback = explicit.clone();
            }
            let attrs = match explicit.first().copied() {
                Some(name) => cosmic_text::Attrs::new().family(cosmic_text::Family::Name(name)),
                None => cosmic_text::Attrs::new().family(cosmic_text::Family::SansSerif),
            };
            let mut b = cosmic_text::Buffer::new(
                &mut self.fonts,
                cosmic_text::Metrics::new(r.size.max(6.0), r.line_height.max(8.0)),
            );
            b.set_size(&mut self.fonts, Some(r.width.max(1.0)), None);
            if r.emphasis.is_empty() {
                b.set_text(
                    &mut self.fonts,
                    &r.text,
                    &attrs,
                    cosmic_text::Shaping::Advanced,
                );
            } else {
                let mut spans = Vec::new();
                let mut pos = 0;
                for &(start, end) in &r.emphasis {
                    if start >= pos
                        && end <= r.text.len()
                        && r.text.is_char_boundary(start)
                        && r.text.is_char_boundary(end)
                    {
                        if start > pos {
                            spans.push((&r.text[pos..start], attrs.clone()));
                        }
                        spans.push((
                            &r.text[start..end],
                            attrs.clone().color(cosmic_text::Color::rgb(240, 212, 138)),
                        ));
                        pos = end;
                    }
                }
                if pos < r.text.len() {
                    spans.push((&r.text[pos..], attrs.clone()));
                }
                b.set_rich_text(
                    &mut self.fonts,
                    spans,
                    &attrs,
                    cosmic_text::Shaping::Advanced,
                    None,
                );
            }
            b.shape_until_scroll(&mut self.fonts, false);
            self.buffers.insert(r.cache_key, b);
            self.clock = self.clock.wrapping_add(1);
            self.recency.insert(r.cache_key, self.clock);
            self.shapes = self.shapes.saturating_add(1);
        }

        if self.buffers.len() > TARGET_ENTRIES {
            let protected: HashSet<u64> = runs.iter().map(|r| r.cache_key).collect();
            let remove_count = self.buffers.len().saturating_sub(TARGET_ENTRIES);
            let mut candidates: Vec<(u64, u64)> = self
                .recency
                .iter()
                .filter(|(k, _)| !protected.contains(k))
                .map(|(&k, &age)| (k, age))
                .collect();
            candidates.sort_by_key(|(_, age)| *age);
            for (k, _) in candidates.into_iter().take(remove_count) {
                self.buffers.remove(&k);
                self.recency.remove(&k);
            }
        }
    }
}
