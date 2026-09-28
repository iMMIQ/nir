//! Continuous history layout over a frozen Player projection. This is a renderer
//! primitive, not an NIR capability or a source-history unit conversion.
use super::{MenuHistoryRow, TextEngine, TextRun};
use std::sync::Arc;

const MAX_ENTRIES: usize = 1000;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_EXTENT: f32 = 16_777_216.;
const BATCH_ENTRIES: usize = 16;
const BATCH_BYTES: usize = 64 * 1024;
const MAX_VISIBLE: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HistoryStyle {
    /// Rendered viewport units, after stage and reader font scaling.
    pub width: f32,
    pub height: f32,
    pub size: f32,
    pub line_height: f32,
    /// Gap between frozen records, not a source scenario-page gap.
    pub gap: f32,
}
impl HistoryStyle {
    fn validate(self) -> Result<(), String> {
        if ![
            self.width,
            self.height,
            self.size,
            self.line_height,
            self.gap,
        ]
        .into_iter()
        .all(f32::is_finite)
            || self.width <= 0.
            || self.width > 32768.
            || self.height <= 0.
            || self.height > 32768.
            || self.size <= 0.
            || self.size > 4096.
            || !(self.size..=8192.).contains(&self.line_height)
            || !(0. ..=8192.).contains(&self.gap)
        {
            return Err("E_HISTORY_LAYOUT: invalid style".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct Extent {
    top: f32,
    height: f32,
}
#[derive(Debug, Clone, Copy)]
enum Anchor {
    Latest,
    Text {
        key: usize,
        byte: usize,
        line_fraction: f32,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasureProgress {
    pub entries: usize,
    pub bytes: usize,
    pub ready: bool,
}

/// Shares one immutable history snapshot, retaining only one extent per entry.
/// Shaped buffers remain in TextEngine's bounded LRU, not in this index.
/// Navigation is unavailable until measurement completes; partial height sums
/// must never become a scrollbar's authoritative range.
#[derive(Debug)]
pub struct HistoryLayout {
    rows: Arc<[MenuHistoryRow]>,
    style: HistoryStyle,
    extents: Vec<Extent>,
    total: f32,
    offset: f32,
    anchor: Anchor,
    prepared: bool,
    anchor_offset: Option<f32>,
}
impl HistoryLayout {
    pub fn new(rows: Arc<[MenuHistoryRow]>, style: HistoryStyle) -> Result<Self, String> {
        style.validate()?;
        let mut bytes = 0usize;
        let mut previous = None;
        if rows.len() > MAX_ENTRIES {
            return Err("E_HISTORY_LAYOUT: too many records".into());
        }
        for row in rows.iter() {
            let entry = &row.entry;
            bytes = bytes
                .saturating_add(entry.text.len())
                .saturating_add(entry.speaker.len());
            if bytes > MAX_BYTES || previous.is_some_and(|key| key >= row.key) {
                return Err("E_HISTORY_LAYOUT: invalid frozen history budget or keys".into());
            }
            if entry.locale.is_empty()
                || entry.font_plan_digest.is_empty()
                || entry.font_assets.is_empty()
            {
                return Err("E_HISTORY_LAYOUT: missing frozen font plan".into());
            }
            previous = Some(row.key);
        }
        let prepared = rows.is_empty();
        Ok(Self {
            rows,
            style,
            extents: vec![],
            total: 0.,
            offset: 0.,
            anchor: Anchor::Latest,
            prepared,
            anchor_offset: None,
        })
    }
    pub fn ready(&self) -> bool {
        self.prepared
    }
    pub fn offset(&self) -> Option<f32> {
        self.ready().then_some(self.offset)
    }
    pub fn max_offset(&self) -> Option<f32> {
        self.ready()
            .then_some((self.total - self.style.height).max(0.))
    }
    fn run(&self, index: usize) -> TextRun {
        let entry = &self.rows[index].entry;
        TextRun {
            text: if entry.speaker.is_empty() {
                entry.text.clone()
            } else {
                format!("{}\n{}", entry.speaker, entry.text)
            },
            visible: None,
            x: 0.,
            y: 0.,
            width: self.style.width,
            // Shaping does not depend on visible height. Use the same bounded
            // height for measurement and painting to reuse its exact cache key.
            height: MAX_EXTENT,
            size: self.style.size,
            line_height: self.style.line_height,
            color: [1.; 4],
            emphasis: vec![],
            scroll: 0.,
            clip: None,
            region: None,
            locale: entry.locale.clone(),
            font_assets: entry.font_assets.clone(),
            font_plan_digest: entry.font_plan_digest.clone(),
            preflight_only: false,
            shadow: None,
            monochrome: false,
        }
    }
    fn shape<'a>(
        text: &'a mut TextEngine,
        run: &TextRun,
    ) -> Result<&'a super::cosmic_text::Buffer, String> {
        text.layout_texts(std::slice::from_ref(run));
        if let Some(error) = &text.missing_font {
            return Err(error.clone());
        }
        text.buffers
            .get(&TextEngine::key(run))
            .ok_or_else(|| "E_HISTORY_LAYOUT: missing shaped buffer".into())
    }
    /// At most 16 records and normally 64 KiB per call. A single larger record
    /// is measured whole (up to the existing 4 MiB history budget), because
    /// cutting arbitrary UTF-8 chunks changes shaping and line wrapping.
    /// This is a work bound, not a wall-clock deadline guarantee.
    pub fn measure_next(&mut self, text: &mut TextEngine) -> Result<MeasureProgress, String> {
        let mut progress = MeasureProgress {
            entries: 0,
            bytes: 0,
            ready: self.ready(),
        };
        if self.ready() {
            return Ok(progress);
        }
        while self.extents.len() < self.rows.len() && progress.entries < BATCH_ENTRIES {
            let index = self.extents.len();
            let entry = &self.rows[index].entry;
            let bytes =
                entry.text.len() + entry.speaker.len() + usize::from(!entry.speaker.is_empty());
            if progress.entries > 0 && progress.bytes.saturating_add(bytes) > BATCH_BYTES {
                break;
            }
            let run = self.run(index);
            let buffer = Self::shape(text, &run)?;
            let height = buffer
                .layout_runs()
                .map(|line| line.line_top + line.line_height)
                .fold(self.style.line_height, f32::max);
            let top = self.total + if index == 0 { 0. } else { self.style.gap };
            let total = top + height;
            if !total.is_finite() || total > MAX_EXTENT || total <= top {
                return Err("E_HISTORY_LAYOUT: measured extent exceeds budget".into());
            }
            if let Anchor::Text {
                key,
                byte,
                line_fraction,
            } = self.anchor
            {
                if self.rows[index].key == key {
                    let offsets = TextEngine::line_offsets(&run);
                    let line_top = buffer
                        .layout_runs()
                        .filter_map(|line| {
                            let start = offsets[line.line_i]
                                + line.glyphs.iter().map(|g| g.start).min().unwrap_or(0);
                            (start <= byte).then_some(line.line_top)
                        })
                        .last()
                        .unwrap_or(0.);
                    self.anchor_offset =
                        Some(top + line_top + line_fraction * self.style.line_height);
                }
            }
            self.extents.push(Extent { top, height });
            self.total = total;
            progress.entries += 1;
            progress.bytes += bytes;
        }
        if self.extents.len() == self.rows.len() {
            self.restore_anchor()?;
            self.prepared = true;
        }
        progress.ready = self.ready();
        Ok(progress)
    }
    fn restore_anchor(&mut self) -> Result<(), String> {
        let max = (self.total - self.style.height).max(0.);
        self.offset = match self.anchor {
            Anchor::Latest => max,
            Anchor::Text { .. } => self
                .anchor_offset
                .ok_or("E_HISTORY_LAYOUT: stale text anchor")?
                .clamp(0., max),
        };
        Ok(())
    }
    fn capture_anchor(&self, text: &mut TextEngine) -> Result<Anchor, String> {
        let max = self
            .max_offset()
            .ok_or("E_HISTORY_LAYOUT: incomplete measurement")?;
        if max - self.offset <= 0.5 || self.rows.is_empty() {
            return Ok(Anchor::Latest);
        }
        let index = self
            .extents
            .partition_point(|e| e.top + e.height <= self.offset)
            .min(self.rows.len() - 1);
        let extent = self.extents[index];
        let run = self.run(index);
        let offsets = TextEngine::line_offsets(&run);
        let buffer = Self::shape(text, &run)?;
        let (byte, top) = buffer
            .layout_runs()
            .filter(|line| line.line_top <= self.offset - extent.top)
            .map(|line| {
                (
                    offsets[line.line_i] + line.glyphs.iter().map(|g| g.start).min().unwrap_or(0),
                    line.line_top,
                )
            })
            .last()
            .unwrap_or((0, 0.));
        Ok(Anchor::Text {
            key: self.rows[index].key,
            byte,
            line_fraction: (self.offset - extent.top - top) / self.style.line_height,
        })
    }
    /// Reflow retains the first visible logical character and its fractional
    /// line position. A viewport at the newest edge continues to follow it.
    pub fn reflow(&mut self, style: HistoryStyle, text: &mut TextEngine) -> Result<bool, String> {
        style.validate()?;
        if style == self.style {
            return Ok(false);
        }
        if self.ready() {
            self.anchor = self.capture_anchor(text)?;
        }
        self.style = style;
        self.extents.clear();
        self.anchor_offset = None;
        self.total = 0.;
        self.offset = 0.;
        self.prepared = self.rows.is_empty();
        Ok(true)
    }
    pub fn scroll_to(&mut self, offset: f32) -> bool {
        let Some(max) = self.max_offset() else {
            return false;
        };
        if !offset.is_finite() {
            return false;
        }
        self.offset = offset.clamp(0., max);
        // capture_anchor is deferred to reflow, so ordinary navigation never shapes.
        true
    }
    pub fn scroll_by(&mut self, delta: f32) -> bool {
        if !delta.is_finite() {
            return false;
        }
        self.scroll_to(self.offset + delta)
    }
    /// Return only records intersecting the viewport. Both partial edge records
    /// retain their full shaped text and are clipped; no fixed-height truncation.
    /// Exceeding the text-run budget fails explicitly rather than dropping rows.
    pub fn visible_runs(
        &self,
        origin: [f32; 2],
        clip: Option<[f32; 4]>,
        color: [f32; 4],
    ) -> Result<Vec<TextRun>, String> {
        if !self.ready() {
            return Err("E_HISTORY_LAYOUT: incomplete measurement".into());
        }
        if origin
            .iter()
            .any(|v| !v.is_finite() || v.abs() > MAX_EXTENT)
            || color
                .iter()
                .any(|c| !c.is_finite() || !(0. ..=1.).contains(c))
        {
            return Err("E_HISTORY_LAYOUT: invalid paint arguments".into());
        }
        let [x, y] = origin;
        let mut rect = [x, y, self.style.width, self.style.height];
        if let Some(parent) = clip {
            if parent
                .iter()
                .any(|v| !v.is_finite() || v.abs() > MAX_EXTENT)
                || parent[2] < 0.
                || parent[3] < 0.
            {
                return Err("E_HISTORY_LAYOUT: invalid clip".into());
            }
            let right = (x + rect[2]).min(parent[0] + parent[2]);
            let bottom = (y + rect[3]).min(parent[1] + parent[3]);
            rect[0] = x.max(parent[0]);
            rect[1] = y.max(parent[1]);
            rect[2] = (right - rect[0]).max(0.);
            rect[3] = (bottom - rect[1]).max(0.);
        }
        if rect[2] == 0. || rect[3] == 0. {
            return Ok(vec![]);
        }
        let top = self.offset + rect[1] - y;
        let bottom = top + rect[3];
        let first = self.extents.partition_point(|e| e.top + e.height <= top);
        let end = self.extents.partition_point(|e| e.top < bottom);
        if end.saturating_sub(first) > MAX_VISIBLE {
            return Err("E_HISTORY_LAYOUT: visible text-run budget exceeded".into());
        }
        Ok((first..end)
            .map(|index| {
                let mut run = self.run(index);
                run.x = x;
                run.y = y + self.extents[index].top - self.offset;
                run.color = color;
                run.clip = Some(rect);
                run
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HistoryView;

    fn engine() -> TextEngine {
        let mut text = TextEngine::default();
        text.add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
        text
    }
    fn row(key: usize, text: impl Into<String>) -> MenuHistoryRow {
        MenuHistoryRow {
            key,
            entry: HistoryView {
                speaker: String::new(),
                text: text.into(),
                locale: "en".into(),
                font_plan_digest: "frozen-plan".into(),
                font_assets: vec!["font.reader".into()],
            },
        }
    }
    fn style() -> HistoryStyle {
        HistoryStyle {
            width: 320.,
            height: 96.,
            size: 16.,
            line_height: 24.,
            gap: 12.,
        }
    }
    fn finish(layout: &mut HistoryLayout, text: &mut TextEngine) {
        for _ in 0..1001 {
            if layout.ready() {
                return;
            }
            let progress = layout.measure_next(text).unwrap();
            assert!(progress.entries <= BATCH_ENTRIES);
        }
        panic!("measurement did not terminate");
    }
    #[test]
    fn actual_wrapping_produces_continuous_clipped_records_and_reuses_shapes() {
        let rows: Arc<[_]> = vec![
            row(2, "A short record"),
            row(4, "long wrapped text ".repeat(80)),
            row(8, "Latest record"),
        ]
        .into();
        let original = rows[1].entry.text.clone();
        let mut layout = HistoryLayout::new(rows.clone(), style()).unwrap();
        assert!(!layout.scroll_by(24.));
        assert!(layout.visible_runs([0., 0.], None, [1.; 4]).is_err());
        let mut text = engine();
        finish(&mut layout, &mut text);
        assert!(layout.extents[1].height > style().height * 3.);
        assert_eq!(layout.offset(), layout.max_offset());
        assert_eq!(
            layout.extents[1].top,
            layout.extents[0].height + style().gap
        );
        let offset = layout.extents[1].top + 7.;
        assert!(layout.scroll_to(offset));
        let runs = layout
            .visible_runs([20., 30.], Some([25., 35., 300., 70.]), [0.5; 4])
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, original);
        assert_eq!(runs[0].y, 23.);
        assert_eq!(runs[0].clip, Some([25., 35., 300., 70.]));
        assert_eq!(runs[0].locale, "en");
        assert_eq!(runs[0].font_plan_digest, "frozen-plan");
        let shapes = text.shapes;
        text.layout_texts(&runs);
        assert_eq!(text.shapes, shapes);
        assert_eq!(rows[1].entry.text, original);
        let progress = layout.measure_next(&mut text).unwrap();
        assert_eq!(
            progress,
            MeasureProgress {
                entries: 0,
                bytes: 0,
                ready: true
            }
        );
        assert_eq!(layout.offset(), Some(offset));
    }
    #[test]
    fn thousand_records_are_measured_in_batches_with_bounded_cache_and_visible_output() {
        let rows: Arc<[_]> = (0..1000)
            .map(|i| row(i, format!("Record {i}")))
            .collect::<Vec<_>>()
            .into();
        let mut layout = HistoryLayout::new(rows, style()).unwrap();
        let mut text = engine();
        let first = layout.measure_next(&mut text).unwrap();
        assert_eq!(first.entries, 16);
        assert!(!first.ready);
        assert_eq!(layout.offset(), None);
        finish(&mut layout, &mut text);
        assert!(text.cache_stats().entries <= 128);
        let visible = layout.visible_runs([0., 0.], None, [1.; 4]).unwrap();
        assert!(visible.len() <= 4);
        assert_eq!(visible.last().unwrap().text, "Record 999");
        assert!(layout.scroll_to(0.));
        let visible = layout.visible_runs([0., 0.], None, [1.; 4]).unwrap();
        assert_eq!(visible[0].text, "Record 0");
        assert!(visible.len() <= 4);
    }
    #[test]
    fn reflow_preserves_logical_text_and_latest_edge() {
        let rows: Arc<[_]> = vec![row(20, "日本語の文章と English words. ".repeat(100))].into();
        let mut text = engine();
        let mut layout = HistoryLayout::new(rows, style()).unwrap();
        finish(&mut layout, &mut text);
        layout.scroll_to(24. * 10. + 6.);
        let old = layout.capture_anchor(&mut text).unwrap();
        let Anchor::Text {
            byte,
            line_fraction,
            ..
        } = old
        else {
            panic!()
        };
        assert!(byte > 0);
        assert_eq!(line_fraction, 0.25);
        let changed = HistoryStyle {
            width: 200.,
            size: 20.,
            line_height: 30.,
            ..style()
        };
        assert!(layout.reflow(changed, &mut text).unwrap());
        assert!(!layout.ready());
        // A second resize during preparation must not lose the original anchor.
        assert!(layout
            .reflow(
                HistoryStyle {
                    width: 220.,
                    ..changed
                },
                &mut text
            )
            .unwrap());
        finish(&mut layout, &mut text);
        let run = layout.run(0);
        let offsets = TextEngine::line_offsets(&run);
        let buffer = HistoryLayout::shape(&mut text, &run).unwrap();
        let expected_top = buffer
            .layout_runs()
            .filter_map(|line| {
                let start =
                    offsets[line.line_i] + line.glyphs.iter().map(|g| g.start).min().unwrap_or(0);
                (start <= byte).then_some(line.line_top)
            })
            .last()
            .unwrap();
        assert!((layout.offset().unwrap() - expected_top - 7.5).abs() < 0.1);
        layout.scroll_to(f32::MAX);
        layout.reflow(style(), &mut text).unwrap();
        finish(&mut layout, &mut text);
        assert_eq!(layout.offset(), layout.max_offset());
        let before = layout.offset();
        assert!(!layout.scroll_by(f32::NAN));
        assert!(!layout.scroll_to(f32::INFINITY));
        assert_eq!(layout.offset(), before);
    }
    #[test]
    fn missing_font_never_publishes_partial_range_and_can_retry() {
        let rows: Arc<[_]> = vec![row(0, "Frozen font")].into();
        let mut layout = HistoryLayout::new(rows, style()).unwrap();
        let mut text = TextEngine::default();
        assert!(layout
            .measure_next(&mut text)
            .unwrap_err()
            .starts_with("E_FONT_PLAN"));
        assert!(!layout.ready());
        assert!(layout.extents.is_empty());
        assert_eq!(layout.max_offset(), None);
        text = engine();
        finish(&mut layout, &mut text);
        assert_eq!(layout.max_offset(), Some(0.));
    }
    #[test]
    fn bad_inputs_and_excess_visible_rows_are_rejected_not_silently_dropped() {
        let mut text = engine();
        assert!(HistoryLayout::new(vec![row(2, "a"), row(2, "b")].into(), style()).is_err());
        assert!(
            HistoryLayout::new(vec![row(0, "a".repeat(MAX_BYTES + 1))].into(), style()).is_err()
        );
        assert!(HistoryLayout::new(
            (0..1001).map(|i| row(i, "a")).collect::<Vec<_>>().into(),
            style()
        )
        .is_err());
        assert!(HistoryLayout::new(
            vec![row(0, "a")].into(),
            HistoryStyle {
                width: f32::NAN,
                ..style()
            }
        )
        .is_err());
        let rows = (0..100).map(|i| row(i, "a")).collect::<Vec<_>>().into();
        let mut layout = HistoryLayout::new(
            rows,
            HistoryStyle {
                height: 1000.,
                size: 8.,
                line_height: 8.,
                gap: 0.,
                ..style()
            },
        )
        .unwrap();
        finish(&mut layout, &mut text);
        assert!(layout
            .visible_runs([0., 0.], None, [1.; 4])
            .unwrap_err()
            .contains("budget"));
        assert_eq!(
            layout
                .visible_runs([0., 0.], Some([0., 0., 200., 80.]), [1.; 4])
                .unwrap()
                .len(),
            10
        );
        assert!(layout
            .visible_runs([0., 0.], Some([0., 0., -1., 1.]), [1.; 4])
            .is_err());
        assert!(layout.visible_runs([f32::MAX, 0.], None, [1.; 4]).is_err());
        assert!(layout
            .visible_runs([0., 0.], Some([1000., 0., 20., 20.]), [1.; 4])
            .unwrap()
            .is_empty());
    }
    #[test]
    fn byte_budget_and_oversized_single_record_make_bounded_progress() {
        let mut text = engine();
        // Whole paragraphs must retain shaping; an oversized record is the only
        // one accepted in its batch, rather than silently split or truncated.
        let large = "word ".repeat(14000);
        let rows = vec![row(0, large.clone()), row(1, "small"), row(2, large)].into();
        let mut layout = HistoryLayout::new(rows, style()).unwrap();
        let first = layout.measure_next(&mut text).unwrap();
        assert_eq!((first.entries, first.bytes, first.ready), (1, 70000, false));
        let second = layout.measure_next(&mut text).unwrap();
        assert_eq!((second.entries, second.bytes, second.ready), (1, 5, false));
        let third = layout.measure_next(&mut text).unwrap();
        assert_eq!((third.entries, third.bytes, third.ready), (1, 70000, true));
        assert!(layout.extents[0].height > style().height);
        assert_eq!(
            layout.visible_runs([0., 0.], None, [1.; 4]).unwrap()[0]
                .text
                .len(),
            70000
        );
    }
    #[test]
    fn reflow_measurement_never_adds_an_unaccounted_anchor_shape() {
        let mut text = engine();
        let rows = (0..160)
            .map(|i| row(i, format!("Record {i}")))
            .collect::<Vec<_>>()
            .into();
        let mut layout = HistoryLayout::new(rows, style()).unwrap();
        finish(&mut layout, &mut text);
        layout.scroll_to(60.);
        layout
            .reflow(
                HistoryStyle {
                    width: 200.,
                    ..style()
                },
                &mut text,
            )
            .unwrap();
        while !layout.ready() {
            let before = text.shapes;
            let measured = layout.measure_next(&mut text).unwrap();
            assert!(text.shapes - before <= measured.entries as u64);
        }
        assert!(layout.offset().unwrap() < 200.);
    }
    #[test]
    fn projected_small_viewports_do_not_reapply_authored_font_minimums() {
        let mut text = engine();
        let mut layout = HistoryLayout::new(
            vec![row(0, "Earlier"), row(1, "Latest")].into(),
            HistoryStyle {
                width: 80.,
                height: 12.,
                size: 4.,
                line_height: 6.,
                gap: 3.,
            },
        )
        .unwrap();
        finish(&mut layout, &mut text);
        let runs = layout.visible_runs([0., 0.], None, [1.; 4]).unwrap();
        assert_eq!(runs.last().unwrap().text, "Latest");
        assert_eq!(runs.last().unwrap().size, 4.);
        assert_eq!(layout.max_offset(), Some(3.));
        assert!(HistoryLayout::new(
            Arc::from([]),
            HistoryStyle {
                size: 0.,
                ..style()
            }
        )
        .is_err());
        // Extreme mixtures of tiny text and large gaps must not round a
        // record's positive height to zero and silently lose the record.
        let mut tiny = HistoryLayout::new(
            vec![row(0, ""), row(1, "")].into(),
            HistoryStyle {
                size: 0.000001,
                line_height: 0.000001,
                gap: 8192.,
                ..style()
            },
        )
        .unwrap();
        assert!(tiny.measure_next(&mut text).is_err());
        assert!(!tiny.ready());
    }
    #[test]
    fn empty_history_is_ready_and_no_phantom_gap_is_added() {
        let mut layout = HistoryLayout::new(Arc::from([]), style()).unwrap();
        assert!(layout.ready());
        assert_eq!(layout.max_offset(), Some(0.));
        assert!(layout
            .visible_runs([0., 0.], None, [1.; 4])
            .unwrap()
            .is_empty());
        let mut text = engine();
        assert!(layout
            .reflow(
                HistoryStyle {
                    height: 200.,
                    ..style()
                },
                &mut text
            )
            .unwrap());
        assert!(layout.ready());
        assert_eq!(layout.measure_next(&mut text).unwrap().entries, 0);
    }
}
