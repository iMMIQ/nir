use super::*;

/// CPU shaping cache, kept independent of the GPU atlas and the VM.
pub struct TextEngine {
    pub fonts: cosmic_text::FontSystem,
    pub buffers: std::collections::HashMap<String, cosmic_text::Buffer>,
    pub shapes: u64,
    pub(super) families: std::collections::BTreeMap<String, &'static str>,
    pub(super) configured_plan: String,
    pub missing_font: Option<String>,
    pub(super) cache_recency: std::collections::HashMap<String, u64>,
    pub(super) cache_clock: u64,
    pub(super) cache_hits: u64,
    pub(super) cache_misses: u64,
    pub(super) cache_evictions: u64,
}

/// Process-wide interning table for font family names required by `cosmic_text::Fallback`'s
/// `&'static str` interface. Each unique font family name is interned at most once across
/// the process lifetime rather than leaking on every `TextEngine` creation or device recovery.
pub(super) fn intern_font_family(name: &str) -> &'static str {
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};
    static INTERNED: OnceLock<Mutex<BTreeSet<&'static str>>> = OnceLock::new();
    let table = INTERNED.get_or_init(|| Mutex::new(BTreeSet::new()));
    let mut guard = table.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&existing) = guard.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    guard.insert(leaked);
    leaked
}

/// A snapshot of the CPU text-shaping cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextCacheStats {
    /// Number of successful lookups of a buffer already in the cache.
    pub hits: u64,
    /// Number of lookups whose buffer was not in the cache.
    pub misses: u64,
    /// Number of buffers removed to enforce the cache target.
    pub evictions: u64,
    /// Number of buffers currently in the cache.
    pub entries: usize,
}
pub(super) struct ExplicitFallback {
    pub(super) families: Vec<&'static str>,
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
        let db = cosmic_text::fontdb::Database::new();
        Self {
            fonts: cosmic_text::FontSystem::new_with_locale_and_db("zh-Hans".into(), db),
            buffers: Default::default(),
            shapes: 0,
            families: Default::default(),
            configured_plan: String::new(),
            missing_font: None,
            cache_recency: Default::default(),
            cache_clock: 0,
            cache_hits: 0,
            cache_misses: 0,
            cache_evictions: 0,
        }
    }
}
impl TextEngine {
    pub fn add_font_asset(
        &mut self,
        asset: &str,
        bytes: Vec<u8>,
    ) -> std::result::Result<(), String> {
        let before: std::collections::BTreeSet<_> = self.fonts.db().faces().map(|f| f.id).collect();
        self.fonts.db_mut().load_font_data(bytes);
        let family = self
            .fonts
            .db()
            .faces()
            .find(|face| !before.contains(&face.id))
            .and_then(|face| face.families.first().map(|f| f.0.clone()))
            .ok_or_else(|| format!("E_FONT: no font face in asset {asset}"))?;
        if self
            .families
            .iter()
            .any(|(other, name)| other != asset && *name == family)
        {
            return Err(format!(
                "E_FONT_FAMILY_AMBIGUOUS: duplicate family {family}"
            ));
        }
        let static_family = intern_font_family(&family);
        self.families.insert(asset.into(), static_family);
        // This default is only used by isolated presentation tests with no plan.
        if self.families.len() == 1 {
            self.fonts.db_mut().set_sans_serif_family(family);
        }
        self.buffers.clear();
        self.cache_recency.clear();
        self.configured_plan.clear();
        Ok(())
    }

    /// Return cumulative cache counters and the current number of buffers.
    ///
    /// Hit, miss, and eviction counters are cumulative for the lifetime of
    /// this engine. Loading a font invalidates all cached buffers without
    /// resetting the counters or counting the invalidated buffers as
    /// evictions; `entries` always reports the live buffer count.
    pub fn cache_stats(&self) -> TextCacheStats {
        TextCacheStats {
            hits: self.cache_hits,
            misses: self.cache_misses,
            evictions: self.cache_evictions,
            entries: self.buffers.len(),
        }
    }

    pub(super) fn touch_cache_key(&mut self, key: &str) {
        if self.cache_clock == u64::MAX {
            let mut oldest_first: Vec<_> = self
                .cache_recency
                .iter()
                .map(|(key, age)| (key.clone(), *age))
                .collect();
            oldest_first.sort_by_key(|(_, age)| *age);
            for (index, (key, _)) in oldest_first.into_iter().enumerate() {
                self.cache_recency.insert(key, index as u64 + 1);
            }
            self.cache_clock = self.cache_recency.len() as u64;
        }
        self.cache_clock += 1;
        if let Some(slot) = self.cache_recency.get_mut(key) {
            *slot = self.cache_clock;
        } else {
            self.cache_recency.insert(key.to_owned(), self.cache_clock);
        }
    }

    pub(super) fn sync_cache_recency(&mut self) {
        self.cache_recency
            .retain(|key, _| self.buffers.contains_key(key));
        let untracked: Vec<_> = self
            .buffers
            .keys()
            .filter(|key| !self.cache_recency.contains_key(*key))
            .cloned()
            .collect();
        for key in untracked {
            self.touch_cache_key(&key);
        }
    }

    pub(super) fn evict_unprotected(&mut self, protected: &std::collections::HashSet<&str>) {
        const TARGET_ENTRIES: usize = 128;
        // `buffers` stays public for compatibility, so reconcile direct map
        // edits only on the uncommon over-capacity path.
        self.sync_cache_recency();
        let remove_count = self.buffers.len().saturating_sub(TARGET_ENTRIES);
        if remove_count == 0 {
            return;
        }

        let mut candidates: Vec<_> = self
            .cache_recency
            .iter()
            .filter(|(key, _)| !protected.contains(key.as_str()))
            .map(|(key, age)| (key.clone(), *age))
            .collect();
        candidates.sort_by_key(|(_, age)| *age);
        for (key, _) in candidates.into_iter().take(remove_count) {
            if self.buffers.remove(&key).is_some() {
                self.cache_recency.remove(&key);
                self.cache_evictions = self.cache_evictions.saturating_add(1);
            }
        }
    }
    pub fn key(run: &TextRun) -> String {
        use std::fmt::Write;
        // `height` is not passed to cosmic-text shaping (`set_size(Some(width), None)`),
        // and `monochrome` only affects runs that carry emphasis ranges.
        let monochrome = run.monochrome && !run.emphasis.is_empty();
        let mut out = String::with_capacity(run.text.len() + 96);
        let _ = write!(
            out,
            "{}:{}:{}:{}:{}:{:?}:{:?}:{}:{}:{:?}:{}",
            run.size.to_bits(),
            run.line_height.to_bits(),
            run.width.to_bits(),
            run.locale,
            run.font_plan_digest,
            run.font_assets,
            run.emphasis,
            run.text,
            monochrome,
            run.images,
            run.image_scale.to_bits()
        );
        out
    }
    /// Original UTF-8 offsets for the exact paragraph splitter used by shaping.
    pub fn line_offsets(run: &TextRun) -> Vec<usize> {
        let mut offsets: Vec<_> = if run.emphasis.is_empty() && run.images.is_empty() {
            cosmic_text::LineIter::new(&run.text)
                .map(|(range, _)| range.start)
                .collect()
        } else {
            cosmic_text::BidiParagraphs::new(&run.text)
                .map(|line| line.as_ptr() as usize - run.text.as_ptr() as usize)
                .collect()
        };
        // cosmic-text keeps one empty BufferLine for an empty string.
        if offsets.is_empty() {
            offsets.push(0);
        }
        offsets
    }
    pub fn layout(&mut self, packet: &DrawPacket) {
        self.layout_texts(&packet.texts);
    }
    pub fn layout_texts(&mut self, texts: &[TextRun]) {
        self.missing_font = None;
        // Retain each formatted key for lookup and possible end-of-layout
        // eviction, avoiding a second key allocation for active runs.
        let packet_keys: Vec<_> = texts.iter().map(Self::key).collect();
        // Group cache misses by `(locale, font_plan_digest, font_assets)` so
        // interleaved UI/story runs do not thrash `FontSystem` mid-frame.
        let mut misses: Vec<usize> = Vec::new();
        for (index, key) in packet_keys.iter().enumerate() {
            if self.buffers.contains_key(key) {
                self.cache_hits = self.cache_hits.saturating_add(1);
                self.touch_cache_key(key);
            } else {
                misses.push(index);
            }
        }
        misses.sort_by(|&a, &b| {
            let ra = &texts[a];
            let rb = &texts[b];
            ra.locale
                .cmp(&rb.locale)
                .then_with(|| ra.font_plan_digest.cmp(&rb.font_plan_digest))
                .then_with(|| ra.font_assets.cmp(&rb.font_assets))
        });
        for index in misses {
            let r = &texts[index];
            let key = &packet_keys[index];
            if self.buffers.contains_key(key) {
                self.cache_hits = self.cache_hits.saturating_add(1);
                self.touch_cache_key(key);
                continue;
            }
            self.cache_misses = self.cache_misses.saturating_add(1);
            {
                let explicit: Vec<_> = r
                    .font_assets
                    .iter()
                    .filter_map(|id| self.families.get(id).copied())
                    .collect();
                if !r.font_assets.is_empty() && explicit.len() != r.font_assets.len() {
                    let missing = r
                        .font_assets
                        .iter()
                        .find(|id| !self.families.contains_key(*id))
                        .cloned()
                        .unwrap_or_default();
                    self.missing_font = Some(format!(
                        "E_FONT_PLAN: font asset {missing} was not prepared"
                    ));
                    // A missing face must never reach cosmic-text shaping: an
                    // empty font database panics instead of reporting an error.
                    continue;
                }
                if explicit.is_empty() {
                    self.missing_font =
                        Some("E_FONT_PLAN: text run has no explicit font plan".into());
                    continue;
                }
                let signature = format!("{}:{}:{:?}", r.locale, r.font_plan_digest, explicit);
                if self.configured_plan != signature && !explicit.is_empty() {
                    let old = std::mem::replace(
                        &mut self.fonts,
                        cosmic_text::FontSystem::new_with_locale_and_db(
                            "en".into(),
                            cosmic_text::fontdb::Database::new(),
                        ),
                    );
                    let (_, db) = old.into_locale_and_db();
                    self.fonts = cosmic_text::FontSystem::new_with_locale_and_db_and_fallback(
                        r.locale.clone(),
                        db,
                        ExplicitFallback {
                            families: explicit.clone(),
                        },
                    );
                    self.configured_plan = signature;
                }
                let primary = explicit.first().copied();
                let mut b = cosmic_text::Buffer::new(
                    &mut self.fonts,
                    cosmic_text::Metrics::new(r.size, r.line_height),
                );
                b.set_size(&mut self.fonts, Some(r.width), None);
                let attrs = match primary {
                    Some(name) => cosmic_text::Attrs::new().family(cosmic_text::Family::Name(name)),
                    None => cosmic_text::Attrs::new().family(cosmic_text::Family::SansSerif),
                };
                if !r.images.is_empty() {
                    // The object-replacement character is a layout slot, never
                    // a font glyph. Measure its advance in this exact font plan
                    // and reserve the authored image width with rich metrics.
                    let mut probe = cosmic_text::Buffer::new(
                        &mut self.fonts,
                        cosmic_text::Metrics::new(1., 1.),
                    );
                    probe.set_text(
                        &mut self.fonts,
                        "\u{fffc}",
                        &attrs,
                        cosmic_text::Shaping::Advanced,
                    );
                    probe.shape_until_scroll(&mut self.fonts, false);
                    let unit_width = probe
                        .layout_runs()
                        .flat_map(|line| line.glyphs.iter())
                        .map(|glyph| glyph.w)
                        .sum::<f32>();
                    let mut cuts = vec![0, r.text.len()];
                    for &(start, end) in &r.emphasis {
                        cuts.extend([start, end]);
                    }
                    for image in &r.images {
                        cuts.extend([image.offset as usize, image.offset as usize + 3]);
                    }
                    cuts.sort_unstable();
                    cuts.dedup();
                    let mut spans = vec![];
                    for range in cuts.windows(2) {
                        let [start, end] = [range[0], range[1]];
                        let Some(value) = r.text.get(start..end) else {
                            continue;
                        };
                        let attr = if let Some(placement) = r
                            .images
                            .iter()
                            .find(|i| i.offset as usize == start && end == start + 3)
                        {
                            let image = &placement.image;
                            let width = (image.width + image.margins[0] + image.margins[1]) as f32
                                * r.image_scale;
                            let height = (image.height + image.margins[2] + image.margins[3])
                                as f32
                                * r.image_scale;
                            attrs
                                .clone()
                                .metrics(cosmic_text::Metrics::new(1., height.max(r.line_height)))
                                .letter_spacing(width - unit_width)
                                .color(cosmic_text::Color::rgba(0, 0, 0, 0))
                        } else if !r.monochrome
                            && r.emphasis.iter().any(|&(a, b)| a <= start && end <= b)
                        {
                            attrs.clone().color(cosmic_text::Color::rgb(224, 202, 153))
                        } else {
                            attrs.clone()
                        };
                        spans.push((value, attr));
                    }
                    b.set_rich_text(
                        &mut self.fonts,
                        spans,
                        &attrs,
                        cosmic_text::Shaping::Advanced,
                        None,
                    );
                } else if r.emphasis.is_empty() {
                    b.set_text(
                        &mut self.fonts,
                        &r.text,
                        &attrs,
                        cosmic_text::Shaping::Advanced,
                    );
                } else {
                    let mut spans = vec![];
                    let mut pos = 0;
                    for &(start, end) in &r.emphasis {
                        if start >= pos
                            && end <= r.text.len()
                            && r.text.is_char_boundary(start)
                            && r.text.is_char_boundary(end)
                        {
                            spans.push((&r.text[pos..start], attrs.clone()));
                            spans.push((
                                &r.text[start..end],
                                if r.monochrome {
                                    attrs.clone()
                                } else {
                                    attrs.clone().color(cosmic_text::Color::rgb(224, 202, 153))
                                },
                            ));
                            pos = end;
                        }
                    }
                    spans.push((&r.text[pos..], attrs.clone()));
                    b.set_rich_text(
                        &mut self.fonts,
                        spans,
                        &attrs,
                        cosmic_text::Shaping::Advanced,
                        None,
                    );
                }
                b.shape_until_scroll(&mut self.fonts, false);
                self.buffers.insert(key.clone(), b);
                self.touch_cache_key(key);
                self.shapes = self.shapes.saturating_add(1);
            }
        }
        if self.buffers.len() > 128 {
            // Protect all runs in this packet before the first eviction. If
            // the working set itself is oversized, it remains intact until
            // a later layout offers less protection and the cache can shrink.
            let protected: std::collections::HashSet<&str> =
                packet_keys.iter().map(String::as_str).collect();
            self.evict_unprotected(&protected);
        }
    }
}
