//! Shared decoded audio with O(1) offset seeks.
//!
//! `rodio::SamplesBuffer` + `skip_duration` pulls one sample per skipped
//! frame on the owner thread at voice start; `BufferSource` computes the
//! start index directly from the same integer math rodio uses, so playback
//! positions match the previous construction bit for bit while the sample
//! data is shared between voices through one `Arc`.
use std::sync::Arc;
use std::time::Duration;

/// One decoded asset held for playback; voices borrow the samples.
pub struct AudioBuffer {
    samples: Arc<Vec<f32>>,
    channels: u16,
    rate: u32,
}
impl AudioBuffer {
    pub fn from_parts(samples: Arc<Vec<f32>>, channels: u16, rate: u32) -> Self {
        Self {
            samples,
            channels: channels.max(1),
            rate: rate.max(1),
        }
    }
    pub fn channels(&self) -> u16 {
        self.channels
    }
    pub fn rate(&self) -> u32 {
        self.rate
    }
    pub fn source(&self, offset_us: u64, looped: bool) -> BufferSource {
        BufferSource::new(
            self.samples.clone(),
            self.channels,
            self.rate,
            offset_us,
            looped,
        )
    }
}
/// Infinite or single-pass playback over shared samples starting at an
/// offset computed in constant time.
pub struct BufferSource {
    samples: Arc<Vec<f32>>,
    channels: u16,
    rate: u32,
    pos: usize,
    looped: bool,
}
impl BufferSource {
    /// `offset_us` semantics mirror the previous rodio construction:
    /// looped voices reduce the offset modulo the buffer duration, then
    /// advance whole spans of the repeated stream; finite voices skip with
    /// the per-channel frame count of a span-less buffer.
    pub fn new(
        samples: Arc<Vec<f32>>,
        channels: u16,
        rate: u32,
        offset_us: u64,
        looped: bool,
    ) -> Self {
        let channels = channels.max(1);
        let rate = rate.max(1);
        let len = samples.len();
        let mut pos = 0;
        if len > 0 && offset_us > 0 {
            if looped {
                // rodio::buffer::SamplesBuffer::total_duration:
                // 1e9 * len / rate / channels nanoseconds.
                let total_ns =
                    (len as u64).saturating_mul(1_000_000_000) / rate as u64 / channels as u64;
                let total_us = total_ns / 1_000;
                let offset = if total_us > 0 {
                    offset_us % total_us
                } else {
                    offset_us
                };
                // skip_duration over repeat_infinite(): spans of `len`
                // samples, each skipped sample costing 1e9/rate/channels ns.
                let ns_per_sample: u128 = 1_000_000_000 / rate as u128 / channels as u128;
                let span_ns = len as u128 * ns_per_sample;
                let mut remaining = offset as u128 * 1_000;
                let mut skipped: u128 = 0;
                while span_ns <= remaining {
                    skipped += len as u128;
                    remaining -= span_ns;
                }
                skipped += remaining / ns_per_sample;
                pos = (skipped % len as u128) as usize;
            } else {
                // SamplesBuffer::current_span_len() is None, so skip_duration
                // takes the unchecked path: frames * channels samples.
                let frames = offset_us as u128 * 1_000 * rate as u128 / 1_000_000_000;
                let skipped = (frames * channels as u128).min(usize::MAX as u128) as usize;
                pos = skipped.min(len);
            }
        }
        Self {
            samples,
            channels,
            rate,
            pos,
            looped,
        }
    }
}
impl Iterator for BufferSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos < self.samples.len() {
            let sample = self.samples[self.pos];
            self.pos += 1;
            Some(sample)
        } else if self.looped && !self.samples.is_empty() {
            // Looping restarts the asset from its first sample.
            self.pos = 1;
            Some(self.samples[0])
        } else {
            None
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.looped {
            (0, None)
        } else {
            let remaining = self.samples.len() - self.pos;
            (remaining, Some(remaining))
        }
    }
}
#[cfg(any(windows, target_os = "linux"))]
impl rodio::Source for BufferSource {
    fn current_span_len(&self) -> Option<usize> {
        if self.looped {
            None
        } else {
            Some(self.samples.len() - self.pos)
        }
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        if self.looped {
            return None;
        }
        let remaining_ns = (self.samples.len() - self.pos) as u64 * 1_000_000_000
            / self.rate as u64
            / self.channels as u64;
        Some(Duration::from_nanos(remaining_ns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(len: usize) -> Vec<f32> {
        (0..len).map(|i| i as f32 * 0.5 - 3.).collect()
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn rodio_reference(
        samples: &[f32],
        channels: u16,
        rate: u32,
        offset_us: u64,
        looped: bool,
        take: usize,
    ) -> Vec<f32> {
        use rodio::{buffer::SamplesBuffer, Source};
        let buffer = SamplesBuffer::new(channels, rate, samples.to_vec());
        if looped {
            let duration = buffer.total_duration().unwrap();
            let offset = if looped && !duration.is_zero() {
                offset_us % duration.as_micros() as u64
            } else {
                offset_us
            };
            buffer
                .repeat_infinite()
                .skip_duration(Duration::from_micros(offset))
                .take(take)
                .collect()
        } else {
            buffer
                .skip_duration(Duration::from_micros(offset_us))
                .take(take)
                .collect()
        }
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn finite_sources_match_skip_duration_at_all_offsets() {
        for (channels, rate, frames) in [(1u16, 44_100u32, 5_000), (2, 48_000, 3_731)] {
            let samples = pattern(frames * channels as usize);
            let total_us = frames as u64 * 1_000_000 / rate as u64;
            for offset_us in [
                0,
                total_us / 2,
                total_us.saturating_sub(1),
                total_us,
                total_us * 3,
            ] {
                let actual: Vec<f32> =
                    AudioBuffer::from_parts(Arc::new(samples.clone()), channels, rate)
                        .source(offset_us, false)
                        .collect();
                let expected =
                    rodio_reference(&samples, channels, rate, offset_us, false, usize::MAX);
                assert_eq!(actual, expected, "{channels}ch {rate}Hz @{offset_us}us");
            }
        }
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn looped_sources_match_repeat_infinite_skip_at_all_offsets() {
        for (channels, rate, frames) in [(1u16, 44_100u32, 5_000), (2, 48_000, 3_731)] {
            let samples = pattern(frames * channels as usize);
            let total_us = frames as u64 * 1_000_000 / rate as u64;
            for offset_us in [0, total_us / 2, total_us + total_us / 3, total_us * 7] {
                let take = samples.len() * 3 + 7;
                let actual: Vec<f32> =
                    AudioBuffer::from_parts(Arc::new(samples.clone()), channels, rate)
                        .source(offset_us, true)
                        .take(take)
                        .collect();
                let expected = rodio_reference(&samples, channels, rate, offset_us, true, take);
                assert_eq!(actual, expected, "{channels}ch {rate}Hz @{offset_us}us");
            }
        }
    }

    #[test]
    fn looping_restarts_at_zero_and_finite_playback_ends() {
        let samples = pattern(8);
        let buffer = AudioBuffer::from_parts(Arc::new(samples.clone()), 2, 4);
        // Duration truncates to 1 s (1e9*8/4/2 ns), so 3.5 s wraps to 0.5 s,
        // i.e. sample 4 - the same index the rodio reference produces.
        let looped: Vec<f32> = buffer.source(3_500_000, true).take(10).collect();
        assert_eq!(&looped[..4], &samples[4..]);
        assert_eq!(&looped[4..], &samples[..6]);
        let finite: Vec<f32> = buffer.source(0, false).collect();
        assert_eq!(finite, samples);
        assert!(buffer.source(u64::MAX / 1_000, false).next().is_none());
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn source_reports_shape_and_remaining_duration() {
        use rodio::Source as _;
        let buffer = AudioBuffer::from_parts(Arc::new(pattern(1_000)), 2, 1_000);
        let mut source = buffer.source(0, false);
        assert_eq!(rodio::Source::channels(&source), 2);
        assert_eq!(rodio::Source::sample_rate(&source), 1_000);
        assert_eq!(source.current_span_len(), Some(1_000));
        assert_eq!(source.total_duration(), Some(Duration::from_millis(500)));
        for _ in 0..400 {
            source.next();
        }
        assert_eq!(source.current_span_len(), Some(600));
        assert_eq!(source.total_duration(), Some(Duration::from_millis(300)));
        let looping = buffer.source(0, true);
        assert_eq!(looping.current_span_len(), None);
        assert_eq!(looping.total_duration(), None);
    }
}
