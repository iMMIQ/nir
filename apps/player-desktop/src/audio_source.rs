//! Shared decoded audio with O(1) offset seeks.
//!
//! Offset seeks compute a frame index on the owner thread and share one Arc.
//! Whole-track loops use the same rounded frame timeline as authored loop
//! regions; seeking never splits a stereo frame or accumulates per-sample
//! nanosecond rounding. Finite voices keep their existing floor-frame seek.
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
    pub fn source_with_region(
        &self,
        offset_us: u64,
        looped: bool,
        region: Option<nir_format::AudioLoopRegion>,
    ) -> Result<BufferSource, &'static str> {
        let Some(region) = region else {
            return Ok(self.source(offset_us, looped));
        };
        if !looped {
            return Err("E_AUDIO_LOOP: region on a finite voice");
        }
        let frames = (self.samples.len() / self.channels as usize) as u64;
        let (start, end) = region
            .frame_bounds(self.rate, frames)
            .ok_or("E_AUDIO_LOOP: invalid decoded loop frames")?;
        let position = region
            .playback_frame(offset_us, self.rate, frames)
            .ok_or("E_AUDIO_LOOP: invalid playhead")?;
        Ok(BufferSource {
            samples: self.samples.clone(),
            channels: self.channels,
            rate: self.rate,
            pos: position as usize * self.channels as usize,
            looped: true,
            loop_start: start as usize * self.channels as usize,
            loop_end: end as usize * self.channels as usize,
        })
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
    loop_start: usize,
    loop_end: usize,
}
impl BufferSource {
    /// Keep one converter for the voice lifetime. Sink queues and mixers can
    /// divide Sources into spans; resampling inside those spans would restart
    /// fractional phase at every span or authored loop boundary.
    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
    pub fn at_device_rate(self, rate: u32) -> DeviceSource {
        use rodio::Source;
        let channels = self.channels;
        let source_rate = self.rate;
        let duration = self.total_duration();
        let rate = rate.max(1);
        DeviceSource {
            input: rodio::conversions::SampleRateConverter::new(self, source_rate, rate, channels),
            channels,
            rate,
            duration,
        }
    }
    /// Loop offsets select a complete nearest frame and wrap in integer
    /// frames. Finite offsets retain the unchecked floor-frame seek policy.
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
                let frames = len / channels as usize;
                if frames > 0 {
                    let frame = (offset_us as u128 * rate as u128 + 500_000) / 1_000_000;
                    pos = (frame % frames as u128) as usize * channels as usize;
                }
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
            loop_start: 0,
            loop_end: len,
        }
    }
}

#[cfg(any(windows, target_os = "linux", target_os = "android"))]
pub struct DeviceSource {
    input: rodio::conversions::SampleRateConverter<BufferSource>,
    channels: u16,
    rate: u32,
    duration: Option<Duration>,
}
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
impl Iterator for DeviceSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.input.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.input.size_hint()
    }
}
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
impl rodio::Source for DeviceSource {
    // Output format stays fixed across every intro and loop boundary. Any
    // outer queue span conversion is now at an identical rate (a passthrough).
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
}

/// The sink's queue survives an output-device replacement. The old stream is
/// dropped by the output worker before a new mixer can pull this same queue;
/// its cursor, resampler, envelope and Sink position never restart.
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
#[derive(Clone)]
pub struct SharedVoiceQueue {
    input: Arc<std::sync::Mutex<FixedVoiceQueue>>,
    channels: u16,
    rate: u32,
}
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
impl SharedVoiceQueue {
    pub fn new(input: rodio::queue::SourcesQueueOutput, channels: u16, rate: u32) -> Self {
        Self {
            input: Arc::new(std::sync::Mutex::new(FixedVoiceQueue::new(
                input, channels, rate,
            ))),
            channels,
            rate,
        }
    }
}
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
impl Iterator for SharedVoiceQueue {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.input
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .next()
    }
}
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
impl rodio::Source for SharedVoiceQueue {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Each NIR sink owns one voice of a known, fixed format. Announce that
/// format before the queue's initially empty source has yielded any audio;
/// otherwise the mixer bootstraps with rodio's empty-source 48 kHz mono
/// metadata and can prefetch silence or convert the first span incorrectly.
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
pub struct FixedVoiceQueue {
    input: rodio::queue::SourcesQueueOutput,
    channels: u16,
    rate: u32,
}
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
impl FixedVoiceQueue {
    pub fn new(input: rodio::queue::SourcesQueueOutput, channels: u16, rate: u32) -> Self {
        Self {
            input,
            channels,
            rate,
        }
    }
}
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
impl Iterator for FixedVoiceQueue {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.input.next()
    }
}
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
impl rodio::Source for FixedVoiceQueue {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
impl Iterator for BufferSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos < self.loop_end {
            let sample = self.samples[self.pos];
            self.pos += 1;
            Some(sample)
        } else if self.looped && self.loop_start < self.loop_end {
            // Intro is traversed once; subsequent cycles start at the region.
            self.pos = self.loop_start + 1;
            Some(self.samples[self.loop_start])
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
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
impl rodio::Source for BufferSource {
    fn current_span_len(&self) -> Option<usize> {
        // Direct consumers can inspect the contiguous run up to the loop
        // boundary. Playback resolves rate conversion continuously through
        // DeviceSource before the Sink's queue can split this run into spans.
        if self.pos >= self.loop_end {
            Some(self.loop_end - self.loop_start)
        } else {
            Some(self.loop_end - self.pos)
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

    #[test]
    fn region_repeats_body_without_intro_tail_or_channel_phase_errors() {
        let buffer = AudioBuffer::from_parts(Arc::new((0..16).map(|i| i as f32).collect()), 2, 10);
        let region = nir_format::AudioLoopRegion {
            start_us: nir_format::Micros(200_000),
            end_us: nir_format::Micros(600_000),
        };
        let actual: Vec<f32> = buffer
            .source_with_region(0, true, Some(region))
            .unwrap()
            .take(28)
            .collect();
        let expected: Vec<f32> = (0..12)
            .chain(4..12)
            .chain(4..12)
            .map(|i| i as f32)
            .collect();
        assert_eq!(actual, expected);
        for (offset, expected) in [
            (600_000, vec![4., 5., 6., 7., 8., 9.]),
            (900_000, vec![10., 11., 4., 5., 6., 7.]),
            (1_400_000, vec![4., 5., 6., 7., 8., 9.]),
            (u64::MAX, vec![8., 9., 10., 11., 4., 5.]),
        ] {
            assert_eq!(
                buffer
                    .source_with_region(offset, true, Some(region))
                    .unwrap()
                    .take(6)
                    .collect::<Vec<_>>(),
                expected
            );
        }
        assert!(buffer.source_with_region(0, false, Some(region)).is_err());
        let too_long = nir_format::AudioLoopRegion {
            end_us: nir_format::Micros(900_000),
            ..region
        };
        assert!(buffer.source_with_region(0, true, Some(too_long)).is_err());
        let zero_frames = nir_format::AudioLoopRegion {
            start_us: nir_format::Micros(200_001),
            end_us: nir_format::Micros(200_002),
        };
        assert!(buffer
            .source_with_region(0, true, Some(zero_frames))
            .is_err());
    }

    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
    #[test]
    fn region_spans_end_at_the_loop_boundary_and_then_exclude_intro() {
        use rodio::Source as _;
        let buffer = AudioBuffer::from_parts(Arc::new(vec![0.; 16]), 2, 10);
        let region = nir_format::AudioLoopRegion {
            start_us: nir_format::Micros(200_000),
            end_us: nir_format::Micros(600_000),
        };
        let mut source = buffer.source_with_region(0, true, Some(region)).unwrap();
        assert_eq!(source.current_span_len(), Some(12));
        assert_eq!(source.total_duration(), None);
        assert_eq!(source.by_ref().take(12).count(), 12);
        assert_eq!(source.current_span_len(), Some(8));
        source.next();
        assert_eq!(source.current_span_len(), Some(7));
    }

    fn pattern(len: usize) -> Vec<f32> {
        (0..len).map(|i| i as f32 * 0.5 - 3.).collect()
    }

    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
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

    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
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

    #[test]
    fn whole_track_loop_seek_matches_an_explicit_full_region() {
        for (channels, rate, len) in [(1, 8_000, 53), (2, 44_100, 4096), (2, 48_000, 1536)] {
            let samples = Arc::new(pattern(len));
            let buffer = AudioBuffer::from_parts(samples, channels, rate);
            let frames = (len / channels as usize) as u64;
            let duration_us = (frames * 1_000_000 + u64::from(rate) / 2) / u64::from(rate);
            let region = nir_format::AudioLoopRegion {
                start_us: nir_format::Micros(0),
                end_us: nir_format::Micros(duration_us),
            };
            for offset in [
                0,
                duration_us / 2,
                duration_us.saturating_sub(1),
                duration_us,
                duration_us * 7,
                u64::MAX,
            ] {
                let actual = buffer
                    .source(offset, true)
                    .take(len * 3)
                    .collect::<Vec<_>>();
                let expected = buffer
                    .source_with_region(offset, true, Some(region))
                    .unwrap()
                    .take(len * 3)
                    .collect::<Vec<_>>();
                assert_eq!(actual, expected, "{channels}ch {rate}Hz @{offset}us");
            }
        }
    }
    #[test]
    fn looping_stereo_seek_preserves_channel_order_and_long_track_position() {
        // These two saved positions previously started on a right sample,
        // or drifted hundreds of samples ahead through nanosecond truncation.
        for (rate, seconds, offset, frame) in [
            (44_100, 2, 347_000, 15_303),
            (48_000, 120, 109_483_355, 5_255_201),
        ] {
            let samples = (0..rate * seconds)
                .flat_map(|i| [i as f32, -(i as f32) - 0.5])
                .collect();
            let buffer = AudioBuffer::from_parts(Arc::new(samples), 2, rate);
            assert_eq!(
                buffer.source(offset, true).take(4).collect::<Vec<_>>(),
                [
                    frame as f32,
                    -(frame as f32) - 0.5,
                    (frame + 1) as f32,
                    -((frame + 1) as f32) - 0.5
                ]
            );
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

    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
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
        // Looped sources report spans to the cycle end (and a fresh cycle
        // once exhausted) so rodio's converter chunks stay long.
        assert_eq!(looping.current_span_len(), Some(1_000));
        assert_eq!(looping.total_duration(), None);
    }
}

#[cfg(all(test, any(windows, target_os = "linux", target_os = "android")))]
mod recovery_tests {
    use super::*;
    use rodio::{mixer::mixer, source::UniformSourceIterator, Sink};

    #[test]
    fn output_mixer_replacement_keeps_the_same_voice_cursor_and_sink() {
        let buffer = AudioBuffer::from_parts(Arc::new(vec![1., 2., 3., 4.]), 1, 4);
        let (sink, queue) = Sink::new();
        sink.append(buffer.source(0, true).at_device_rate(4));
        let queue = SharedVoiceQueue::new(queue, 1, 4);
        let (old, mut output) = mixer(1, 4);
        old.add(queue.clone());
        assert_eq!(output.by_ref().take(3).collect::<Vec<_>>(), [1., 2., 3.]);
        drop(output);
        drop(old);
        let (new, mut output) = mixer(1, 4);
        new.add(queue);
        assert_eq!(
            output.by_ref().take(5).collect::<Vec<_>>(),
            [4., 1., 2., 3., 4.]
        );
        assert!(!sink.empty());
    }
    #[test]
    fn output_replacement_inside_body_keeps_intro_once_and_the_current_loop_frame() {
        let buffer = AudioBuffer::from_parts(
            Arc::new(vec![-10., -9., 1., 2., 3., 4., 5., 6., 7., 8.]),
            1,
            10,
        );
        let region = nir_format::AudioLoopRegion {
            start_us: nir_format::Micros(200_000),
            end_us: nir_format::Micros(600_000),
        };
        let (sink, queue) = Sink::new();
        sink.append(
            buffer
                .source_with_region(0, true, Some(region))
                .unwrap()
                .at_device_rate(10),
        );
        let queue = SharedVoiceQueue::new(queue, 1, 10);
        let (old, mut output) = mixer(1, 10);
        old.add(queue.clone());
        assert_eq!(
            output.by_ref().take(7).collect::<Vec<_>>(),
            [-10., -9., 1., 2., 3., 4., 1.]
        );
        drop(output);
        drop(old);
        let (new, output) = mixer(1, 10);
        new.add(queue);
        let expected = buffer
            .source_with_region(700_000, true, Some(region))
            .unwrap()
            .take(32)
            .collect::<Vec<_>>();
        assert_eq!(output.take(32).collect::<Vec<_>>(), expected);
        assert!(!sink.empty());
    }

    #[test]
    fn replacement_device_rate_converts_the_remaining_queue_without_replaying_intro() {
        let buffer = AudioBuffer::from_parts(Arc::new(vec![1., 2., 3., 4.]), 1, 4);
        let (sink, queue) = Sink::new();
        sink.append(buffer.source(0, true).at_device_rate(4));
        let queue = SharedVoiceQueue::new(queue, 1, 4);
        let (old, mut output) = mixer(1, 4);
        old.add(queue.clone());
        assert_eq!(output.by_ref().take(3).collect::<Vec<_>>(), [1., 2., 3.]);
        drop(output);
        drop(old);
        let (new, output) = mixer(1, 8);
        new.add(queue);
        let expected =
            UniformSourceIterator::new(buffer.source(750_000, true).at_device_rate(4), 1, 8)
                .take(32)
                .collect::<Vec<_>>();
        assert_eq!(output.take(32).collect::<Vec<_>>(), expected);
        assert!(!sink.empty());
    }

    #[test]
    fn output_replacement_keeps_the_owned_fade_and_stereo_frame_phase() {
        use crate::audio_envelope::{EnvelopeSamples, Ramp};
        use std::sync::Mutex;
        let buffer = AudioBuffer::from_parts(Arc::new(vec![1.; 16]), 2, 4);
        let envelope = Arc::new(Mutex::new(Ramp::default()));
        envelope
            .lock()
            .unwrap()
            .set_owned(Some(7), 250_000, 1., 0., 1_000_000);
        let (sink, queue) = Sink::new();
        sink.append(EnvelopeSamples::new(
            buffer.source(0, true).at_device_rate(4),
            envelope.clone(),
            2,
            4,
        ));
        let queue = SharedVoiceQueue::new(queue, 2, 4);
        let (old, mut output) = mixer(2, 4);
        old.add(queue.clone());
        assert_eq!(
            output.by_ref().take(4).collect::<Vec<_>>(),
            [1., 1., 0.75, 0.75]
        );
        let observed = envelope.lock().unwrap().observation().unwrap();
        assert_eq!(observed.owner, 7);
        assert_eq!(observed.elapsed_us.0, 500_000);
        drop(output);
        drop(old);
        // No callback is consuming the queue while the output is unavailable.
        assert_eq!(envelope.lock().unwrap().observation(), Some(observed));
        let (new, output) = mixer(2, 4);
        new.add(queue);
        assert_eq!(
            output.take(6).collect::<Vec<_>>(),
            [0.5, 0.5, 0.25, 0.25, 0., 0.]
        );
        let restored = envelope.lock().unwrap().observation().unwrap();
        assert_eq!(restored.owner, 7);
        assert_eq!(restored.elapsed_us.0, 1_250_000);
        assert!(!sink.empty());
    }
}
