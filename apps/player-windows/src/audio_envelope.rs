//! Sample-clock envelope. Paused sinks do not consume samples or envelope time.
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct Ramp {
    from: f32,
    to: f32,
    duration_us: u64,
    frames: u64,
    owner: Option<u32>,
    base_us: u64,
    sampled_us: u64,
}
impl Default for Ramp {
    fn default() -> Self {
        Self {
            from: 1.,
            to: 1.,
            duration_us: 0,
            frames: 0,
            owner: None,
            base_us: 0,
            sampled_us: 0,
        }
    }
}
impl Ramp {
    pub fn set(&mut self, from: f32, to: f32, duration_us: u64) {
        self.set_owned(None, 0, from, to, duration_us);
    }
    pub fn set_owned(
        &mut self,
        owner: Option<u32>,
        base_us: u64,
        from: f32,
        to: f32,
        duration_us: u64,
    ) {
        self.owner = owner;
        self.base_us = base_us;
        self.sampled_us = 0;
        self.from = from;
        self.to = to;
        self.duration_us = duration_us;
        self.frames = 0;
    }
    pub fn observation(&self) -> Option<nir_format::AudioEnvelopePosition> {
        self.owner.map(|owner| nir_format::AudioEnvelopePosition {
            owner,
            elapsed_us: nir_format::Micros(
                self.base_us
                    .saturating_add(self.sampled_us.min(self.duration_us)),
            ),
        })
    }
    fn next_frame(&mut self, rate: u32) -> f32 {
        self.sampled_us = ((self.frames as u128 * 1_000_000) / rate.max(1) as u128)
            .min(self.duration_us as u128) as u64;
        let p = if self.duration_us == 0 {
            1.
        } else {
            (self.frames as f64 * 1_000_000. / (self.duration_us as f64 * rate as f64)).min(1.)
                as f32
        };
        self.frames = self.frames.saturating_add(1);
        nir_format::interpolate(self.from, self.to, p, nir_format::Easing::Linear)
    }
}
pub type Envelope = Arc<Mutex<Ramp>>;
pub struct EnvelopeSamples<I> {
    input: I,
    envelope: Envelope,
    channels: u16,
    rate: u32,
    channel: u16,
    value: f32,
}
impl<I> EnvelopeSamples<I> {
    pub fn new(input: I, envelope: Envelope, channels: u16, rate: u32) -> Self {
        Self {
            input,
            envelope,
            channels: channels.max(1),
            rate: rate.max(1),
            channel: 0,
            value: 1.,
        }
    }
}
impl<I: Iterator<Item = f32>> Iterator for EnvelopeSamples<I> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let sample = self.input.next()?;
        if self.channel == 0 {
            self.value = self
                .envelope
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .next_frame(self.rate);
        }
        self.channel = (self.channel + 1) % self.channels;
        Some(sample * self.value)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.input.size_hint()
    }
}
#[cfg(windows)]
impl<I: rodio::Source> rodio::Source for EnvelopeSamples<I> {
    fn current_span_len(&self) -> Option<usize> {
        self.input.current_span_len()
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        self.input.total_duration()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observation_tracks_restored_owned_ramp_and_constant_replacement_clears_owner() {
        let state = Arc::new(Mutex::new(Ramp::default()));
        state
            .lock()
            .unwrap()
            .set_owned(Some(7), 250_000, 0.75, 0., 750_000);
        let mut source = EnvelopeSamples::new(std::iter::repeat(1.), state.clone(), 2, 4);
        assert_eq!(source.next(), Some(0.75));
        assert_eq!(source.next(), Some(0.75));
        assert_eq!(
            state.lock().unwrap().observation().unwrap().elapsed_us.0,
            250_000
        );
        assert_eq!(source.next(), Some(0.5));
        let observed = state.lock().unwrap().observation().unwrap();
        assert_eq!(observed.owner, 7);
        assert_eq!(observed.elapsed_us.0, 500_000);
        assert_eq!(state.lock().unwrap().observation(), Some(observed));
        state.lock().unwrap().set(0.5, 0.5, 0);
        assert_eq!(state.lock().unwrap().observation(), None);
    }
    #[test]
    fn envelope_uses_sample_frames_and_preserves_stereo() {
        let state = Arc::new(Mutex::new(Ramp::default()));
        state.lock().unwrap().set(1., 0., 500_000);
        let actual: Vec<_> =
            EnvelopeSamples::new(std::iter::repeat_n(1., 6), state, 2, 4).collect();
        assert_eq!(actual, [1., 1., 0.5, 0.5, 0., 0.]);
    }
    #[test]
    fn no_consumption_means_no_clock_and_replacement_starts_from_explicit_value() {
        let state = Arc::new(Mutex::new(Ramp::default()));
        state.lock().unwrap().set(1., 0., 1_000_000);
        let mut source = EnvelopeSamples::new(std::iter::repeat(1.), state.clone(), 1, 4);
        assert_eq!(source.next(), Some(1.));
        assert_eq!(source.next(), Some(0.75));
        state.lock().unwrap().set(0.5, 0.5, 0);
        assert_eq!(source.next(), Some(0.5));
        state.lock().unwrap().set(0.5, 0., 500_000);
        assert_eq!(source.next(), Some(0.5));
        assert_eq!(source.next(), Some(0.25));
        assert_eq!(source.next(), Some(0.));
    }
}
