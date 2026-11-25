use crate::TextRun;
use nir_ir::{AssetId, TextAlign};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// Cached shaped `cosmic-text` buffer paired with precomputed line byte offsets
/// and total shaped height so typewriter clipping and vertical centering never re-shape per frame!
pub struct CachedTextBuffer {
    pub buffer: cosmic_text::Buffer,
    pub line_offsets: Vec<usize>,
    pub content_height: f32,
    pub effective_size: f32,
}

/// Zero-alloc-key CPU text shaping cache backed by `cosmic-text`.
pub struct TextEngine {
    pub fonts: cosmic_text::FontSystem,
    pub buffers: HashMap<u64, CachedTextBuffer>,
    recency: HashMap<u64, u64>,
    clock: u64,
    families: BTreeMap<AssetId, &'static str>,
    fallback_families: Vec<&'static str>,
    fallback_dirty: bool,
    #[cfg(not(target_arch = "wasm32"))]
    system_fonts_loaded: bool,
    pub missing_glyphs: bool,
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
        // Defer scanning OS system fonts until actually needed (games with bundled fonts
        // skip the 100-500ms cold-start system font scan completely!).
        let db = cosmic_text::fontdb::Database::new();
        Self {
            fonts: cosmic_text::FontSystem::new_with_locale_and_db("zh-Hans".into(), db),
            buffers: HashMap::new(),
            recency: HashMap::new(),
            clock: 0,
            families: BTreeMap::new(),
            fallback_families: Vec::new(),
            fallback_dirty: false,
            #[cfg(not(target_arch = "wasm32"))]
            system_fonts_loaded: false,
            missing_glyphs: false,
            shapes: 0,
        }
    }
}

impl TextEngine {
    pub fn add_font_asset(&mut self, id: AssetId, bytes: Vec<u8>) -> Result<(), String> {
        if self.families.contains_key(&id) {
            return Ok(());
        }
        let before: HashSet<_> = self.fonts.db().faces().map(|f| f.id).collect();
        self.fonts.db_mut().load_font_data(bytes);
        let new_names: Vec<String> = self
            .fonts
            .db()
            .faces()
            .filter(|face| !before.contains(&face.id))
            .filter_map(|face| face.families.first().map(|f| f.0.clone()))
            .collect();
        let Some(first) = new_names.first().cloned() else {
            return Err(format!("E_FONT: no font face in asset {}", id.0));
        };
        let static_family = intern_family(&first);
        self.families.insert(id, static_family);
        if self.families.len() == 1 {
            self.fonts.db_mut().set_sans_serif_family(first);
        }
        self.buffers.clear();
        self.recency.clear();
        self.missing_glyphs = false;
        self.fallback_dirty = true;
        Ok(())
    }

    /// Registers a shared fallback font (e.g. Web Pan-CJK fallback `.otf`/`.ttf`/`.ttc`)
    /// consulted AFTER any game-bundled fonts so missing glyphs (SC/TC/JP/symbols) never tofu!
    pub fn add_fallback_font(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        let before: HashSet<_> = self.fonts.db().faces().map(|f| f.id).collect();
        self.fonts.db_mut().load_font_data(bytes);
        let mut added = Vec::new();
        for face in self.fonts.db().faces() {
            if before.contains(&face.id) {
                continue;
            }
            if let Some((name, _)) = face.families.first() {
                if !added.iter().any(|n: &String| n == name) {
                    added.push(name.clone());
                }
            }
        }
        if added.is_empty() {
            return Err("E_FONT: no font face in fallback font".into());
        }
        // Prefer SC / JP faces first when loading a multi-face CJK TTC.
        added.sort_by_key(|name| {
            let lower = name.to_ascii_lowercase();
            if lower.contains(" sc") {
                0u8
            } else if lower.contains(" jp") {
                1u8
            } else if lower.contains(" tc") {
                2u8
            } else if lower.contains(" hk") {
                3u8
            } else {
                4u8
            }
        });
        let had_any = !self.families.is_empty() || !self.fallback_families.is_empty();
        if !had_any {
            self.fonts.db_mut().set_sans_serif_family(added[0].clone());
        }
        for name in added {
            let interned = intern_family(&name);
            if !self.fallback_families.contains(&interned) {
                self.fallback_families.push(interned);
            }
        }
        self.buffers.clear();
        self.recency.clear();
        self.missing_glyphs = false;
        self.fallback_dirty = true;
        Ok(())
    }

    pub fn has_font(&self, id: AssetId) -> bool {
        self.families.contains_key(&id)
    }

    pub fn has_any_font(&self) -> bool {
        !self.families.is_empty() || !self.fallback_families.is_empty()
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

    fn ensure_fonts_ready(&mut self) {
        if self.fallback_dirty && (!self.families.is_empty() || !self.fallback_families.is_empty())
        {
            let mut unique = Vec::with_capacity(self.families.len() + self.fallback_families.len());
            for &fam in self.families.values() {
                if !unique.contains(&fam) {
                    unique.push(fam);
                }
            }
            for &fam in &self.fallback_families {
                if !unique.contains(&fam) {
                    unique.push(fam);
                }
            }
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
                ExplicitFallback { families: unique },
            );
            self.fallback_dirty = false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.families.is_empty()
            && self.fallback_families.is_empty()
            && !self.system_fonts_loaded
        {
            self.fonts.db_mut().load_system_fonts();
            self.system_fonts_loaded = true;
        }
    }

    fn build_buffer(
        fonts: &mut cosmic_text::FontSystem,
        r: &TextRun,
        attrs: &cosmic_text::Attrs<'_>,
        rich: bool,
        align: Option<cosmic_text::Align>,
        size: f32,
        line_height: f32,
    ) -> cosmic_text::Buffer {
        let mut b = cosmic_text::Buffer::new(
            fonts,
            cosmic_text::Metrics::new(size.max(6.0), line_height.max(8.0)),
        );
        b.set_size(fonts, Some(r.width.max(1.0)), None);
        if !rich {
            b.set_text(fonts, &r.text, attrs, cosmic_text::Shaping::Advanced);
        } else {
            // Merge emphasis ranges (warm gold) and explicit per-span colors into non-overlapping slices.
            let mut styled: Vec<(usize, usize, cosmic_text::Color)> =
                Vec::with_capacity(r.emphasis.len() + r.color_spans.len());
            for &(start, end) in &r.emphasis {
                if start < end
                    && end <= r.text.len()
                    && r.text.is_char_boundary(start)
                    && r.text.is_char_boundary(end)
                {
                    styled.push((start, end, cosmic_text::Color::rgb(244, 214, 136)));
                }
            }
            for &(start, end, rgba) in &r.color_spans {
                if start < end
                    && end <= r.text.len()
                    && r.text.is_char_boundary(start)
                    && r.text.is_char_boundary(end)
                {
                    let c = cosmic_text::Color::rgba(
                        (rgba[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                        (rgba[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                        (rgba[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                        ((rgba[3] * r.color[3]).clamp(0.0, 1.0) * 255.0).round() as u8,
                    );
                    styled.push((start, end, c));
                }
            }
            styled.sort_by_key(|&(s, e, _)| (s, e));

            let mut spans = Vec::new();
            let mut pos = 0;
            for (start, end, col) in styled {
                let s = start.max(pos);
                if s >= end {
                    continue;
                }
                if s > pos {
                    spans.push((&r.text[pos..s], attrs.clone()));
                }
                spans.push((&r.text[s..end], attrs.clone().color(col)));
                pos = end;
            }
            if pos < r.text.len() {
                spans.push((&r.text[pos..], attrs.clone()));
            }
            b.set_rich_text(fonts, spans, attrs, cosmic_text::Shaping::Advanced, None);
        }
        if let Some(al) = align {
            for line in &mut b.lines {
                line.set_align(Some(al));
            }
        }
        b.shape_until_scroll(fonts, false);
        b
    }

    fn measure_buffer(b: &cosmic_text::Buffer) -> (usize, f32) {
        let mut count = 0usize;
        let mut bottom = 0.0f32;
        for run in b.layout_runs() {
            count += 1;
            bottom = bottom.max(run.line_top + run.line_height);
        }
        (count, bottom)
    }

    fn shape_entry(&mut self, r: &TextRun, cache_key: u64, with_emphasis: bool) {
        if self.buffers.contains_key(&cache_key) {
            self.clock = self.clock.wrapping_add(1);
            self.recency.insert(cache_key, self.clock);
            return;
        }
        self.ensure_fonts_ready();

        let primary = r
            .font_assets
            .iter()
            .find_map(|id| self.families.get(id).copied())
            .or_else(|| self.families.values().next().copied())
            .or_else(|| self.fallback_families.first().copied());
        let attrs = match primary {
            Some(name) => cosmic_text::Attrs::new().family(cosmic_text::Family::Name(name)),
            None => cosmic_text::Attrs::new().family(cosmic_text::Family::SansSerif),
        };
        let align = match r.align {
            TextAlign::Left => None,
            TextAlign::Center => Some(cosmic_text::Align::Center),
            TextAlign::Right => Some(cosmic_text::Align::Right),
        };
        let rich = with_emphasis && r.has_rich_colors();

        let mut cur_size = r.size.max(6.0);
        let mut cur_lh = r.line_height.max(cur_size * 1.05);
        let mut b = Self::build_buffer(&mut self.fonts, r, &attrs, rich, align, cur_size, cur_lh);
        let (mut run_count, mut content_h) = Self::measure_buffer(&b);

        if r.auto_fit && r.height > 1.0 && !r.text.is_empty() {
            let want_single_line = !r.text.contains('\n') && r.height < r.size * 1.95;
            let overflows =
                |rc: usize, ch: f32| (want_single_line && rc > 1) || ch > r.height + 1.0;

            // Stage 1: For multi-line dialogue text, gently tighten excess line-height first
            // (absorbs LiveMaker extra leading / font-metric differences without shrinking text!).
            if !want_single_line && content_h > r.height + 1.0 {
                let tight_lh = (cur_size * 1.15).max(cur_size + 2.0);
                if cur_lh > tight_lh + 0.5 {
                    let needed_ratio = (r.height / content_h).clamp(0.75, 0.98);
                    cur_lh = (cur_lh * needed_ratio).max(tight_lh);
                    b = Self::build_buffer(
                        &mut self.fonts,
                        r,
                        &attrs,
                        rich,
                        align,
                        cur_size,
                        cur_lh,
                    );
                    (run_count, content_h) = Self::measure_buffer(&b);
                }
            }

            // Stage 2: Smoothly scale font size down if text STILL overflows container bounds.
            let min_size = (r.size * 0.52).max(8.5).min(cur_size);
            let mut steps = 0u8;
            while overflows(run_count, content_h) && cur_size > min_size + 0.25 && steps < 5 {
                steps += 1;
                let factor = if want_single_line && run_count > 1 {
                    (1.0 / (run_count as f32).sqrt()).clamp(0.74, 0.86)
                } else {
                    (r.height / content_h.max(1.0)).sqrt().clamp(0.76, 0.93)
                };
                let next_size = (cur_size * factor).max(min_size);
                let ratio = next_size / cur_size;
                cur_size = next_size;
                cur_lh = (cur_lh * ratio).max(cur_size * 1.12);
                b = Self::build_buffer(&mut self.fonts, r, &attrs, rich, align, cur_size, cur_lh);
                (run_count, content_h) = Self::measure_buffer(&b);
            }
        }

        if !self.missing_glyphs {
            'scan: for run in b.layout_runs() {
                for g in run.glyphs {
                    if g.glyph_id == 0 {
                        if let Some(slice) = run.text.get(g.start..g.end) {
                            if slice.chars().any(|c| {
                                !c.is_whitespace()
                                    && !c.is_control()
                                    && c != '\u{3000}'
                                    && c != '\u{FFFC}'
                            }) {
                                self.missing_glyphs = true;
                                break 'scan;
                            }
                        }
                    }
                }
            }
        }

        let line_offsets = Self::line_offsets(&r.text, rich);
        self.buffers.insert(
            cache_key,
            CachedTextBuffer {
                buffer: b,
                line_offsets,
                content_height: content_h,
                effective_size: cur_size,
            },
        );
        self.clock = self.clock.wrapping_add(1);
        self.recency.insert(cache_key, self.clock);
        self.shapes = self.shapes.saturating_add(1);
    }

    /// Shapes all runs (and any distinct color-stripped shadow/outline buffers) with zero `TextRun` cloning!
    pub fn layout_texts(&mut self, runs: &[TextRun]) {
        const TARGET_ENTRIES: usize = 256;
        for r in runs {
            self.shape_entry(r, r.cache_key, true);
            if (r.shadow.is_some() || r.outline.is_some()) && r.has_rich_colors() {
                let sk = r.shadow_cache_key();
                self.shape_entry(r, sk, false);
            }
        }

        if self.buffers.len() > TARGET_ENTRIES {
            let mut protected: HashSet<u64> = HashSet::with_capacity(runs.len() * 2);
            for r in runs {
                protected.insert(r.cache_key);
                if (r.shadow.is_some() || r.outline.is_some()) && r.has_rich_colors() {
                    protected.insert(r.shadow_cache_key());
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_font_loads_and_auto_fit_shrinks_overflowing_dialogue() {
        let mut engine = TextEngine::default();
        let noto = std::path::Path::new("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc");
        if noto.is_file() {
            let bytes = std::fs::read(noto).unwrap();
            engine.add_fallback_font(bytes).unwrap();
            assert!(engine.has_any_font());
        }

        // Long 6-line dialogue stuffed into a 70px tall box: auto_fit MUST shrink it to fit!
        let mut run = TextRun {
            text: "这是一段非常非常长的测试台词，第一行第二行第三行第四行第五行，在窄屏或字体行距偏大时绝对不应该被硬裁剪掉底部文字，自动缩放应当让整段字幕完整放进对话框内。".into(),
            visible_bytes: None,
            x: 0.0,
            y: 0.0,
            width: 240.0,
            height: 76.0,
            size: 24.0,
            line_height: 36.0,
            color: [1.0, 1.0, 1.0, 1.0],
            emphasis: Vec::new(),
            color_spans: Vec::new(),
            ruby_spans: Vec::new(),
            shadow: None,
            outline: None,
            scroll: 0.0,
            clip: Some([0.0, 0.0, 240.0, 76.0]),
            font_assets: Vec::new(),
            align: TextAlign::Left,
            valign_center: false,
            auto_fit: true,
            inline_images: Vec::new(),
            cache_key: 0,
        };
        run.compute_key();
        engine.layout_texts(std::slice::from_ref(&run));
        let entry = engine
            .buffers
            .get(&run.cache_key)
            .expect("shaped buffer cached");
        assert!(
            entry.effective_size < 24.0,
            "auto_fit should shrink overflowing dialogue font (got {})",
            entry.effective_size
        );
        assert!(
            entry.content_height <= 82.0,
            "auto_fit should bring dialogue height near box height (got {})",
            entry.content_height
        );
    }
}
