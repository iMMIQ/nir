//! High-performance native audio host backed by `rodio` + `symphonia`:
//! - Zero main-thread decoding: BGM / long tracks stream from a background decoder thread (~256 KB RAM!)
//! - Background pre-decode LRU cache (32 MiB) for Voice & SFX honoring runtime lookahead
//! - Zero-copy `PakSliceReader` directly over `mmap` `.nirpak` (no compressed byte copy!)
//! - Reusable `SampleBuffer<f32>` (zero per-packet heap allocation) & zero-copy LAME gapless trimming

use crate::pak::PakArchive;
use nir_ir::{AssetId, AudioBus, AudioLoopRegion, ChannelId, Preferences};
use rodio::{OutputStream, OutputStreamBuilder, Sink, Source};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Compressed size threshold below which non-looped audio (SFX/Voice) is eligible for LRU caching.
const SHORT_AUDIO_MAX_COMPRESSED: usize = 256 * 1024;
/// Maximum RAM dedicated to decoded short audio (Voice / SFX) PCM cache.
const AUDIO_CACHE_LIMIT_BYTES: usize = 32 * 1024 * 1024;
/// Samples per streaming chunk sent from feeder thread to rodio mixer thread (~16 KiB per chunk).
const STREAM_CHUNK_SAMPLES: usize = 4096;
/// Bounded streaming queue depth (8 * 16 KiB ≈ 128 KiB peak per active streaming track!).
const STREAM_QUEUE_DEPTH: usize = 8;

/// Zero-copy `MediaSource` reading directly from a byte range inside `Arc<PakArchive>`.
struct PakSliceReader {
    pak: Arc<PakArchive>,
    start: usize,
    len: usize,
    pos: usize,
}

impl PakSliceReader {
    fn new(pak: Arc<PakArchive>, start: usize, len: usize) -> Self {
        Self {
            pak,
            start,
            len,
            pos: 0,
        }
    }

    #[inline]
    fn slice(&self) -> &[u8] {
        &self.pak.raw_slice()[self.start..self.start + self.len]
    }
}

impl Read for PakSliceReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let rem = self.len.saturating_sub(self.pos);
        if rem == 0 {
            return Ok(0);
        }
        let n = rem.min(buf.len());
        buf[..n].copy_from_slice(&self.slice()[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl Seek for PakSliceReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let next: i64 = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(delta) => self.pos as i64 + delta,
            SeekFrom::End(delta) => self.len as i64 + delta,
        };
        if next < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "negative seek position",
            ));
        }
        self.pos = (next as usize).min(self.len);
        Ok(self.pos as u64)
    }
}

impl MediaSource for PakSliceReader {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len as u64)
    }
}

struct DecodedPcm {
    samples: Vec<f32>,
    /// Zero-copy gapless LAME trim range inside `samples`.
    start: usize,
    end: usize,
    sample_rate: u32,
    channels: u16,
}

impl DecodedPcm {
    #[inline]
    fn bytes_len(&self) -> usize {
        self.samples.len() * std::mem::size_of::<f32>()
    }
}

struct AudioCache {
    entries: HashMap<AssetId, (Arc<DecodedPcm>, u64)>,
    inflight: HashSet<AssetId>,
    bytes: usize,
    clock: u64,
}

impl AudioCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            inflight: HashSet::new(),
            bytes: 0,
            clock: 0,
        }
    }

    fn get(&mut self, id: AssetId) -> Option<Arc<DecodedPcm>> {
        self.clock = self.clock.wrapping_add(1);
        let tick = self.clock;
        if let Some((pcm, last)) = self.entries.get_mut(&id) {
            *last = tick;
            return Some(Arc::clone(pcm));
        }
        None
    }

    fn insert(&mut self, id: AssetId, pcm: Arc<DecodedPcm>) {
        self.inflight.remove(&id);
        let added = pcm.bytes_len();
        if added > AUDIO_CACHE_LIMIT_BYTES / 2 {
            return;
        }
        self.clock = self.clock.wrapping_add(1);
        if let Some((old, _)) = self.entries.insert(id, (pcm, self.clock)) {
            self.bytes = self.bytes.saturating_sub(old.bytes_len());
        }
        self.bytes = self.bytes.saturating_add(added);
        self.evict_if_needed(id);
    }

    fn evict_if_needed(&mut self, keep: AssetId) {
        if self.bytes <= AUDIO_CACHE_LIMIT_BYTES {
            return;
        }
        let mut victims: Vec<(AssetId, u64)> = self
            .entries
            .iter()
            .filter(|(&id, _)| id != keep)
            .map(|(&id, &(_, age))| (id, age))
            .collect();
        victims.sort_by_key(|(_, age)| *age);
        for (id, _) in victims {
            if self.bytes <= AUDIO_CACHE_LIMIT_BYTES {
                break;
            }
            if let Some((old, _)) = self.entries.remove(&id) {
                self.bytes = self.bytes.saturating_sub(old.bytes_len());
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.inflight.clear();
        self.bytes = 0;
    }
}

struct PrefetchJob {
    pak: Arc<PakArchive>,
    id: AssetId,
    start: usize,
    len: usize,
    ext: &'static str,
}

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
    cache: Arc<Mutex<AudioCache>>,
    prefetch_tx: mpsc::Sender<PrefetchJob>,
}

impl AudioHost {
    pub fn new(prefs: &Preferences) -> Self {
        let stream = OutputStreamBuilder::open_default_stream().ok();
        let cache = Arc::new(Mutex::new(AudioCache::new()));
        let (prefetch_tx, prefetch_rx) = mpsc::channel::<PrefetchJob>();
        let worker_cache = Arc::clone(&cache);
        let _ = std::thread::Builder::new()
            .name("nir-audio-prefetch".into())
            .spawn(move || {
                while let Ok(job) = prefetch_rx.recv() {
                    let decoded = decode_pcm_f32_from_pak(job.pak, job.start, job.len, job.ext);
                    let mut guard = worker_cache.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(pcm) = decoded {
                        guard.insert(job.id, Arc::new(pcm));
                    } else {
                        guard.inflight.remove(&job.id);
                    }
                }
            });

        let mut host = Self {
            stream,
            channels: HashMap::new(),
            bus_gains: [1.0; 4],
            bus_paused: [false; 3],
            cache,
            prefetch_tx,
        };
        host.apply_preferences(prefs);
        host
    }

    pub fn clear_cache(&mut self) {
        if let Ok(mut guard) = self.cache.lock() {
            guard.clear();
        }
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

    /// Asynchronously pre-decodes short Voice/SFX assets into the LRU cache (lookahead!).
    pub fn prefetch(&self, pak: &Arc<PakArchive>, id: AssetId, ext: &'static str) {
        if self.stream.is_none() {
            return;
        }
        let Some((start, len)) = pak.asset_range(id) else {
            return;
        };
        if len == 0 || len > SHORT_AUDIO_MAX_COMPRESSED {
            return;
        }
        let should_queue = {
            let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if guard.entries.contains_key(&id) || !guard.inflight.insert(id) {
                false
            } else {
                true
            }
        };
        if should_queue {
            let _ = self.prefetch_tx.send(PrefetchJob {
                pak: Arc::clone(pak),
                id,
                start,
                len,
                ext,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn play(
        &mut self,
        pak: &Arc<PakArchive>,
        asset: AssetId,
        channel: ChannelId,
        bus: AudioBus,
        ext_hint: &'static str,
        meta_sample_rate: u32,
        meta_channels: u16,
        gain: f32,
        looped: bool,
        fade_in_us: u64,
        loop_region: Option<AudioLoopRegion>,
    ) {
        self.stop(channel);
        let Some(stream) = &self.stream else {
            return;
        };
        let Some((start, len)) = pak.asset_range(asset) else {
            return;
        };
        if len == 0 {
            return;
        }

        let sink = Sink::connect_new(stream.mixer());
        let vol = self.effective_gain(bus, gain);
        sink.set_volume(vol);
        if self.bus_paused[bus_index(bus)] {
            sink.pause();
        }

        // Fast path 1: Already decoded in LRU cache (0ms startup, zero-copy Arc share!)
        let cached = {
            let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            guard.get(asset)
        };
        if let Some(pcm) = cached {
            let src = PcmLoopSource::new(pcm, looped, loop_region);
            if fade_in_us > 0 {
                sink.append(src.fade_in(Duration::from_micros(fade_in_us)));
            } else {
                sink.append(src);
            }
            self.channels.insert(
                channel,
                ActiveChannel {
                    sink,
                    bus,
                    base_gain: gain,
                },
            );
            return;
        }

        // Fast path 2: Background streaming decoder (NEVER blocks the main thread, ~256 KB RAM!).
        // If it's a short unlooped clip, also schedule it into the LRU cache for instant replays.
        let cache_if_short = (!looped && len <= SHORT_AUDIO_MAX_COMPRESSED)
            .then(|| (Arc::clone(&self.cache), asset));

        let Some(stream_src) = StreamingAudioSource::spawn(
            Arc::clone(pak),
            start,
            len,
            ext_hint,
            meta_sample_rate,
            meta_channels,
            looped,
            loop_region,
            cache_if_short,
        ) else {
            return;
        };

        if fade_in_us > 0 {
            sink.append(stream_src.fade_in(Duration::from_micros(fade_in_us)));
        } else {
            sink.append(stream_src);
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

fn lame_trim_samples(pak: &PakArchive, start: usize, len: usize, ext: &str, channels: u16) -> (usize, usize) {
    if ext != "mp3" {
        return (0, 0);
    }
    let slice = &pak.raw_slice()[start..start + len];
    if let Some(info) = nir_ir::lame::parse(slice) {
        let ch = channels.max(1) as usize;
        let lead = (info.delay as usize + 529) * ch;
        let trail = (info.padding as usize).saturating_sub(529) * ch;
        (lead, trail)
    } else {
        (0, 0)
    }
}

/// Decodes a short audio asset directly from `Arc<PakArchive>` (zero input copy,
/// reusable `SampleBuffer`, zero-copy LAME gapless slice indices!).
fn decode_pcm_f32_from_pak(
    pak: Arc<PakArchive>,
    start: usize,
    len: usize,
    ext_hint: &str,
) -> Option<DecodedPcm> {
    let mut hint = Hint::new();
    if !ext_hint.is_empty() {
        hint.with_extension(ext_hint);
    }
    let (lead_hint, trail_hint) = (Arc::clone(&pak), (start, len));
    let reader = PakSliceReader::new(pak, start, len);
    let mss = MediaSourceStream::new(Box::new(reader), Default::default());
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

    let est_samples = track
        .codec_params
        .n_frames
        .map(|f| (f as usize).saturating_mul(channels as usize))
        .unwrap_or(0)
        .min(8 * 1024 * 1024);
    let mut pcm: Vec<f32> = Vec::with_capacity(est_samples);
    let mut sbuf: Option<SampleBuffer<f32>> = None;

    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else {
            continue;
        };
        let cap = decoded.capacity() as u64;
        let spec = *decoded.spec();
        let buf = match &mut sbuf {
            Some(b) if b.capacity() >= decoded.capacity() => b,
            slot => slot.insert(SampleBuffer::<f32>::new(cap, spec)),
        };
        buf.copy_interleaved_ref(decoded);
        pcm.extend_from_slice(buf.samples());
    }

    if pcm.is_empty() {
        return None;
    }

    let (lead, trail) = lame_trim_samples(&lead_hint, trail_hint.0, trail_hint.1, ext_hint, channels);
    let (trim_start, trim_end) = if lead + trail < pcm.len() {
        (lead, pcm.len() - trail)
    } else {
        (0, pcm.len())
    };

    Some(DecodedPcm {
        samples: pcm,
        start: trim_start,
        end: trim_end,
        sample_rate,
        channels: channels.max(1),
    })
}

/// Low-memory background streaming source: decodes chunk-by-chunk on a feeder thread
/// with lookbehind LAME gapless trimming and bounded ~128 KiB RAM footprint!
struct StreamingAudioSource {
    rx: mpsc::Receiver<Vec<f32>>,
    current: Vec<f32>,
    pos: usize,
    sample_rate: u32,
    channels: u16,
}

impl StreamingAudioSource {
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        pak: Arc<PakArchive>,
        start: usize,
        len: usize,
        ext_hint: &'static str,
        meta_sample_rate: u32,
        meta_channels: u16,
        looped: bool,
        loop_region: Option<AudioLoopRegion>,
        cache_short: Option<(Arc<Mutex<AudioCache>>, AssetId)>,
    ) -> Option<Self> {
        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<Vec<f32>>(STREAM_QUEUE_DEPTH);
        let need_probe_sync = meta_sample_rate == 0 || meta_channels == 0;
        let (meta_tx, meta_rx) = if need_probe_sync {
            let (tx, rx) = mpsc::sync_channel::<Option<(u32, u16)>>(1);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        std::thread::Builder::new()
            .name("nir-audio-stream".into())
            .spawn(move || {
                stream_decode_loop(
                    pak,
                    start,
                    len,
                    ext_hint,
                    looped,
                    loop_region,
                    chunk_tx,
                    meta_tx,
                    cache_short,
                );
            })
            .ok()?;

        let (sample_rate, channels) = if let Some(rx) = meta_rx {
            rx.recv().ok()??
        } else {
            (meta_sample_rate, meta_channels.max(1))
        };

        Some(Self {
            rx: chunk_rx,
            current: Vec::new(),
            pos: 0,
            sample_rate: sample_rate.max(8000),
            channels: channels.max(1),
        })
    }
}

impl Iterator for StreamingAudioSource {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        while self.pos >= self.current.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.current = chunk;
                    self.pos = 0;
                }
                Err(_) => return None,
            }
        }
        let s = self.current[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl Source for StreamingAudioSource {
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
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_decode_loop(
    pak: Arc<PakArchive>,
    start: usize,
    len: usize,
    ext_hint: &'static str,
    looped: bool,
    loop_region: Option<AudioLoopRegion>,
    tx: mpsc::SyncSender<Vec<f32>>,
    mut meta_tx: Option<mpsc::SyncSender<Option<(u32, u16)>>>,
    mut cache_short: Option<(Arc<Mutex<AudioCache>>, AssetId)>,
) {
    let mut first_pass = true;
    let mut out_chunk = Vec::with_capacity(STREAM_CHUNK_SAMPLES);
    let mut cache_accum: Option<Vec<f32>> = cache_short.as_ref().map(|_| Vec::new());

    loop {
        let mut hint = Hint::new();
        if !ext_hint.is_empty() {
            hint.with_extension(ext_hint);
        }
        let reader = PakSliceReader::new(Arc::clone(&pak), start, len);
        let mss = MediaSourceStream::new(Box::new(reader), Default::default());
        let Ok(probed) = symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        ) else {
            if let Some(m) = meta_tx.take() {
                let _ = m.send(None);
            }
            return;
        };
        let mut format = probed.format;
        let Some(track) = format.default_track() else {
            if let Some(m) = meta_tx.take() {
                let _ = m.send(None);
            }
            return;
        };
        let Some(sample_rate) = track.codec_params.sample_rate else {
            if let Some(m) = meta_tx.take() {
                let _ = m.send(None);
            }
            return;
        };
        let Some(ch) = track.codec_params.channels.map(|c| c.count() as u16) else {
            if let Some(m) = meta_tx.take() {
                let _ = m.send(None);
            }
            return;
        };
        let track_id = track.id;
        if let Some(m) = meta_tx.take() {
            let _ = m.send(Some((sample_rate, ch.max(1))));
        }

        let Ok(mut decoder) = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
        else {
            return;
        };

        let (lead, trail) = lame_trim_samples(&pak, start, len, ext_hint, ch);
        let ch_usize = ch.max(1) as usize;
        let (loop_start_sample, loop_end_sample) = if let Some(r) = loop_region {
            let ls = ((r.start_us as u128 * sample_rate as u128) / 1_000_000) as usize * ch_usize;
            let le = ((r.end_us as u128 * sample_rate as u128) / 1_000_000) as usize * ch_usize;
            if ls < le {
                (ls, Some(le))
            } else {
                (0, None)
            }
        } else {
            (0, None)
        };

        let skip_after_lead = if first_pass { 0 } else { loop_start_sample };
        let mut total_skip = lead + skip_after_lead;
        let mut emitted: usize = skip_after_lead;
        let mut tail_hold: VecDeque<f32> = VecDeque::with_capacity(trail + 256);
        let mut sbuf: Option<SampleBuffer<f32>> = None;
        let mut hit_loop_end = false;

        let emit_sample = |s: f32,
                           out_chunk: &mut Vec<f32>,
                           cache_accum: &mut Option<Vec<f32>>|
         -> bool {
            if let Some(acc) = cache_accum.as_mut() {
                acc.push(s);
            }
            out_chunk.push(s);
            if out_chunk.len() >= STREAM_CHUNK_SAMPLES {
                let full = std::mem::replace(out_chunk, Vec::with_capacity(STREAM_CHUNK_SAMPLES));
                if tx.send(full).is_err() {
                    return false;
                }
            }
            true
        };

        while let Ok(packet) = format.next_packet() {
            if packet.track_id() != track_id {
                continue;
            }
            let Ok(decoded) = decoder.decode(&packet) else {
                continue;
            };
            let cap = decoded.capacity() as u64;
            let spec = *decoded.spec();
            let buf = match &mut sbuf {
                Some(b) if b.capacity() >= decoded.capacity() => b,
                slot => slot.insert(SampleBuffer::<f32>::new(cap, spec)),
            };
            buf.copy_interleaved_ref(decoded);
            let mut slice = buf.samples();

            if total_skip > 0 {
                if total_skip >= slice.len() {
                    total_skip -= slice.len();
                    continue;
                }
                slice = &slice[total_skip..];
                total_skip = 0;
            }

            if trail == 0 {
                for &s in slice {
                    if let Some(end_at) = loop_end_sample {
                        if emitted >= end_at {
                            hit_loop_end = true;
                            break;
                        }
                    }
                    emitted += 1;
                    if !emit_sample(s, &mut out_chunk, &mut cache_accum) {
                        return;
                    }
                }
            } else {
                for &s in slice {
                    tail_hold.push_back(s);
                    if tail_hold.len() > trail {
                        let ready = tail_hold.pop_front().unwrap();
                        if let Some(end_at) = loop_end_sample {
                            if emitted >= end_at {
                                hit_loop_end = true;
                                break;
                            }
                        }
                        emitted += 1;
                        if !emit_sample(ready, &mut out_chunk, &mut cache_accum) {
                            return;
                        }
                    }
                }
            }

            if hit_loop_end {
                break;
            }
        }

        // If looping ended early at an explicit loop_region end, drain non-padding samples up to loop_end.
        // If EOF was reached naturally, `tail_hold` holds the exact LAME trailing padding and is dropped!
        if !looped {
            if !out_chunk.is_empty() {
                let _ = tx.send(out_chunk);
            }
            if let (Some((cache, id)), Some(samples)) = (cache_short.take(), cache_accum.take()) {
                if !samples.is_empty() {
                    let end = samples.len();
                    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
                    guard.insert(
                        id,
                        Arc::new(DecodedPcm {
                            samples,
                            start: 0,
                            end,
                            sample_rate,
                            channels: ch.max(1),
                        }),
                    );
                }
            }
            return;
        }
        first_pass = false;
        cache_accum = None;
        cache_short = None;
    }
}

/// Zero-copy sample-accurate looping PCM source backed by shared `Arc<DecodedPcm>`.
struct PcmLoopSource {
    pcm: Arc<DecodedPcm>,
    cursor: usize,
    looped: bool,
    loop_start: usize,
    loop_end: usize,
}

impl PcmLoopSource {
    fn new(pcm: Arc<DecodedPcm>, looped: bool, region: Option<AudioLoopRegion>) -> Self {
        let base = pcm.start;
        let total = pcm.end.saturating_sub(base);
        let ch = pcm.channels.max(1) as usize;
        let (loop_start, loop_end) = if let Some(r) = region {
            let s = ((r.start_us as u128 * pcm.sample_rate as u128) / 1_000_000) as usize * ch;
            let e = ((r.end_us as u128 * pcm.sample_rate as u128) / 1_000_000) as usize * ch;
            if s < e && e <= total {
                (base + s, base + e)
            } else {
                (base, pcm.end)
            }
        } else {
            (base, pcm.end)
        };
        Self {
            pcm,
            cursor: base,
            looped,
            loop_start,
            loop_end,
        }
    }
}

impl Iterator for PcmLoopSource {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        if self.cursor >= self.loop_end {
            if self.looped && self.loop_start < self.loop_end {
                self.cursor = self.loop_start;
            } else if self.cursor >= self.pcm.end {
                return None;
            }
        }
        let sample = *self.pcm.samples.get(self.cursor)?;
        self.cursor += 1;
        Some(sample)
    }
}

impl Source for PcmLoopSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.pcm.channels
    }
    fn sample_rate(&self) -> u32 {
        self.pcm.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        if self.looped {
            None
        } else {
            let frames = self.pcm.end.saturating_sub(self.pcm.start) / self.pcm.channels as usize;
            Some(Duration::from_secs_f64(
                frames as f64 / self.pcm.sample_rate as f64,
            ))
        }
    }
}
