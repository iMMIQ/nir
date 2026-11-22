//! Native audio host backed by `rodio` + `symphonia` with gapless loop regions,
//! per-bus gain control (Master/BGM/Voice/SFX), and graceful headless degradation.

use nir_ir::{AudioBus, AudioLoopRegion, ChannelId, Preferences};
use rodio::{OutputStream, OutputStreamBuilder, Sink, Source};
use std::collections::HashMap;
use std::io::Cursor;
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

struct ActiveChannel {
    sink: Sink,
    bus: AudioBus,
    base_gain: f32,
}

pub struct AudioHost {
    stream: Option<OutputStream>,
    channels: HashMap<ChannelId, ActiveChannel>,
    bus_gains: [f32; 4], // [master, bgm, voice, sfx]
    bus_paused: [bool; 3],
}

impl AudioHost {
    pub fn new(prefs: &Preferences) -> Self {
        let stream = OutputStreamBuilder::open_default_stream().ok();
        let mut host = Self {
            stream,
            channels: HashMap::new(),
            bus_gains: [1.0; 4],
            bus_paused: [false; 3],
        };
        host.apply_preferences(prefs);
        host
    }

    pub fn apply_preferences(&mut self, prefs: &Preferences) {
        self.bus_gains = [
            prefs.master_volume.clamp(0.0, 1.0),
            prefs.bgm_volume.clamp(0.0, 1.0),
            prefs.voice_volume.clamp(0.0, 1.0),
            prefs.sfx_volume.clamp(0.0, 1.0),
        ];
        self.refresh_volumes();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn play(
        &mut self,
        channel: ChannelId,
        bus: AudioBus,
        bytes: &[u8],
        ext_hint: &str,
        gain: f32,
        looped: bool,
        fade_in_us: u64,
        loop_region: Option<AudioLoopRegion>,
    ) {
        self.stop(channel);
        let Some(stream) = &self.stream else {
            return;
        };
        let Some((samples, sample_rate, channels)) = decode_pcm_f32(bytes, ext_hint) else {
            return;
        };
        let sink = Sink::connect_new(stream.mixer());
        let src = PcmLoopSource::new(samples, sample_rate, channels, looped, loop_region);
        if fade_in_us > 0 {
            sink.append(src.fade_in(Duration::from_micros(fade_in_us)));
        } else {
            sink.append(src);
        }
        let vol = self.effective_gain(bus, gain);
        sink.set_volume(vol);
        if self.bus_paused[bus_index(bus)] {
            sink.pause();
        }
        self.channels.insert(
            channel,
            ActiveChannel {
                sink,
                bus,
                base_gain: gain,
            },
        );
    }

    pub fn stop(&mut self, channel: ChannelId) {
        if let Some(ch) = self.channels.remove(&channel) {
            ch.sink.stop();
        }
    }

    pub fn set_channel_gain(&mut self, channel: ChannelId, gain: f32) {
        let vol = self
            .channels
            .get(&channel)
            .map(|c| self.effective_gain(c.bus, gain));
        if let (Some(ch), Some(v)) = (self.channels.get_mut(&channel), vol) {
            ch.base_gain = gain;
            ch.sink.set_volume(v);
        }
    }

    pub fn set_bus_paused(&mut self, bus: AudioBus, paused: bool) {
        self.bus_paused[bus_index(bus)] = paused;
        for ch in self.channels.values() {
            if ch.bus == bus {
                if paused {
                    ch.sink.pause();
                } else {
                    ch.sink.play();
                }
            }
        }
    }

    /// Returns channel IDs that have naturally finished playing since last poll.
    pub fn poll_ended(&mut self) -> Vec<ChannelId> {
        let mut ended = Vec::new();
        self.channels.retain(|&id, ch| {
            if ch.sink.empty() {
                ended.push(id);
                false
            } else {
                true
            }
        });
        ended
    }

    fn effective_gain(&self, bus: AudioBus, base_gain: f32) -> f32 {
        let b = match bus {
            AudioBus::Bgm => self.bus_gains[1],
            AudioBus::Voice => self.bus_gains[2],
            AudioBus::Sfx => self.bus_gains[3],
        };
        (self.bus_gains[0] * b * base_gain).clamp(0.0, 2.0)
    }

    fn refresh_volumes(&self) {
        for ch in self.channels.values() {
            ch.sink.set_volume(self.effective_gain(ch.bus, ch.base_gain));
        }
    }
}

fn bus_index(bus: AudioBus) -> usize {
    match bus {
        AudioBus::Bgm => 0,
        AudioBus::Voice => 1,
        AudioBus::Sfx => 2,
    }
}

/// Decodes MP3 / OGG Vorbis / WAV / FLAC to interleaved f32 PCM using Symphonia,
/// trimming LAME encoder delay/padding for gapless MP3 loops.
fn decode_pcm_f32(bytes: &[u8], ext_hint: &str) -> Option<(Vec<f32>, u32, u16)> {
    let mut hint = Hint::new();
    if !ext_hint.is_empty() {
        hint.with_extension(ext_hint);
    }
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let mut format = probed.format;
    let track = format.default_track()?;
    let sample_rate = track.codec_params.sample_rate?;
    let channels = track.codec_params.channels?.count() as u16;
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .ok()?;

    let mut pcm: Vec<f32> = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else {
            continue;
        };
        let mut sbuf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        sbuf.copy_interleaved_ref(decoded);
        pcm.extend_from_slice(sbuf.samples());
    }

    // Gapless MP3 LAME trim
    if ext_hint == "mp3" {
        if let Some(info) = nir_ir::lame::parse(bytes) {
            let lead = (info.delay as usize + 529) * channels as usize;
            let trail = (info.padding as usize).saturating_sub(529) * channels as usize;
            if lead + trail < pcm.len() {
                pcm = pcm[lead..pcm.len() - trail].to_vec();
            }
        }
    }

    if pcm.is_empty() {
        return None;
    }
    Some((pcm, sample_rate, channels))
}

/// Sample-accurate looping PCM source supporting optional `AudioLoopRegion`.
struct PcmLoopSource {
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u16,
    cursor: usize,
    looped: bool,
    loop_start: usize,
    loop_end: usize,
}

impl PcmLoopSource {
    fn new(
        samples: Vec<f32>,
        sample_rate: u32,
        channels: u16,
        looped: bool,
        region: Option<AudioLoopRegion>,
    ) -> Self {
        let total = samples.len();
        let ch = channels.max(1) as usize;
        let (loop_start, loop_end) = if let Some(r) = region {
            let s = ((r.start_us as u128 * sample_rate as u128) / 1_000_000) as usize * ch;
            let e = ((r.end_us as u128 * sample_rate as u128) / 1_000_000) as usize * ch;
            if s < e && e <= total {
                (s, e)
            } else {
                (0, total)
            }
        } else {
            (0, total)
        };
        Self {
            samples,
            sample_rate,
            channels: channels.max(1),
            cursor: 0,
            looped,
            loop_start,
            loop_end,
        }
    }
}

impl Iterator for PcmLoopSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.cursor >= self.loop_end {
            if self.looped && self.loop_start < self.loop_end {
                self.cursor = self.loop_start;
            } else if self.cursor >= self.samples.len() {
                return None;
            }
        }
        let sample = *self.samples.get(self.cursor)?;
        self.cursor += 1;
        Some(sample)
    }
}

impl Source for PcmLoopSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        if self.looped {
            None
        } else {
            let frames = self.samples.len() / self.channels as usize;
            Some(Duration::from_secs_f64(
                frames as f64 / self.sample_rate as f64,
            ))
        }
    }
}
