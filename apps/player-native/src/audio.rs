//! High-performance native audio host backed by `rodio` + `symphonia` + `opus-static-sys`:
//! - Zero main-thread decoding: BGM / long tracks stream from a reusable 6-thread worker pool (~128 KiB RAM/stream)
//! - Background pre-decode LRU cache (32 MiB) storing compact `Vec<i16>` PCM (2x effective clip capacity!)
//! - Full Ogg Opus decoding (RFC 7845 via `ogg` + official Xiph `libopus 1.5`) alongside MP3, OGG Vorbis, WAV, FLAC
//! - Zero-copy `PakSliceReader` directly over `mmap` `.nirpak` (no compressed byte copy!)
//! - Reusable `SampleBuffer<i16>` & zero-copy LAME gapless trimming

use crate::pak::PakArchive;
use nir_ir::{AssetId, AudioBus, AudioLoopRegion, ChannelId, Preferences};
use ogg::reading::PacketReader;
use rodio::{OutputStream, OutputStreamBuilder, Sink, Source};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Compressed size threshold below which non-looped audio (SFX/Voice) is eligible for LRU caching.
const SHORT_AUDIO_MAX_COMPRESSED: usize = 256 * 1024;
/// Maximum RAM dedicated to decoded short audio (Voice / SFX) PCM16 cache (~2x capacity vs f32!).
#[cfg(target_os = "android")]
const AUDIO_CACHE_LIMIT_BYTES: usize = 16 * 1024 * 1024;
#[cfg(not(target_os = "android"))]
const AUDIO_CACHE_LIMIT_BYTES: usize = 32 * 1024 * 1024;
/// Samples per streaming chunk sent from feeder thread to rodio mixer thread (~16 KiB per chunk).
const STREAM_CHUNK_SAMPLES: usize = 4096;
/// Bounded streaming queue depth (8 * 16 KiB ≈ 128 KiB peak per active streaming track).
const STREAM_QUEUE_DEPTH: usize = 8;
/// Persistent streaming decoder worker threads (lean count covers BGM + Voice + SFX concurrently
/// while minimizing idle stack memory and OS thread overhead).
#[cfg(target_os = "android")]
const STREAM_POOL_WORKERS: usize = 2;
#[cfg(not(target_os = "android"))]
const STREAM_POOL_WORKERS: usize = 3;

const INV_I16_SCALE: f32 = 1.0 / 32768.0;

#[inline]
fn f32_to_i16(s: f32) -> i16 {
    (s * 32767.0).round().clamp(-32768.0, 32767.0) as i16
}

/// Safe RAII wrapper around official Xiph `libopus` C decoder (`opus-static-sys`).
struct LibOpusDecoder {
    raw: *mut opus_static_sys::OpusDecoder,
    channels: usize,
}

// SAFETY: `OpusDecoder` holds self-contained per-stream state with no thread-local storage.
unsafe impl Send for LibOpusDecoder {}

impl LibOpusDecoder {
    fn new(channels: u16) -> Option<Self> {
        let ch = channels.clamp(1, 2) as i32;
        let mut err: core::ffi::c_int = 0;
        let raw = unsafe { opus_static_sys::opus_decoder_create(48_000, ch, &mut err) };
        if raw.is_null() || err != opus_static_sys::OPUS_OK as i32 {
            if !raw.is_null() {
                unsafe { opus_static_sys::opus_decoder_destroy(raw) };
            }
            return None;
        }
        Some(Self {
            raw,
            channels: ch as usize,
        })
    }

    fn decode_i16(&mut self, packet: &[u8], out_pcm: &mut [i16]) -> Option<usize> {
        let max_frames = (out_pcm.len() / self.channels) as i32;
        let frames = unsafe {
            opus_static_sys::opus_decode(
                self.raw,
                packet.as_ptr(),
                packet.len() as i32,
                out_pcm.as_mut_ptr(),
                max_frames,
                0,
            )
        };
        if frames < 0 {
            None
        } else {
            Some(frames as usize * self.channels)
        }
    }
}

impl Drop for LibOpusDecoder {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { opus_static_sys::opus_decoder_destroy(self.raw) };
        }
    }
}

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

/// Compact PCM16 audio cache entry (half the RAM of `Vec<f32>` -> 2x LRU clip capacity!).
struct DecodedPcm {
    samples: Vec<i16>,
    /// Zero-copy gapless LAME/Opus trim range inside `samples`.
    start: usize,
    end: usize,
    sample_rate: u32,
    channels: u16,
}

impl DecodedPcm {
    #[inline]
    fn bytes_len(&self) -> usize {
        self.samples.len() * std::mem::size_of::<i16>()
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

struct StreamJob {
    pak: Arc<PakArchive>,
    start: usize,
    len: usize,
    ext_hint: &'static str,
    looped: bool,
    loop_region: Option<AudioLoopRegion>,
    tx: mpsc::SyncSender<Vec<f32>>,
    meta_tx: Option<mpsc::SyncSender<Option<(u32, u16)>>>,
    cache_short: Option<(Arc<Mutex<AudioCache>>, AssetId)>,
}

/// Suspend idle ALSA/PipeWire/WASAPI/CoreAudio/AAudio output stream after 5s of silence (0% audio CPU!).
const AUDIO_IDLE_SUSPEND_DELAY: Duration = Duration::from_secs(5);
/// Minimum backoff between audio device reopen attempts after transient failure / disconnect.
const AUDIO_REOPEN_COOLDOWN: Duration = Duration::from_secs(2);
/// Fade-out volume update cadence (~60 Hz while a channel is fading out).
const FADE_TICK_US: u64 = 16_000;

/// Lightweight descriptor retained for active channels so looping BGM can cleanly release
/// the Android AAudio stream on suspend (0% background drain) and recover from route disconnects!
#[derive(Clone)]
struct ChannelSpec {
    pak: Arc<PakArchive>,
    asset: AssetId,
    bus: AudioBus,
    ext_hint: &'static str,
    meta_sample_rate: u32,
    meta_channels: u16,
    gain: f32,
    looped: bool,
    loop_region: Option<AudioLoopRegion>,
}

struct ActiveChannel {
    sink: Sink,
    bus: AudioBus,
    base_gain: f32,
    spec: ChannelSpec,
}

struct FadingChannel {
    sink: Sink,
    bus: AudioBus,
    start_gain: f32,
    started_at: Instant,
    duration: Duration,
}

pub struct AudioHost {
    stream: Option<OutputStream>,
    stream_failed: Arc<AtomicBool>,
    last_stream_fail: Option<Instant>,
    idle_since: Option<Instant>,
    channels: HashMap<ChannelId, ActiveChannel>,
    suspended_loops: HashMap<ChannelId, ChannelSpec>,
    fading_out: Vec<FadingChannel>,
    video_sink: Option<Sink>,
    bus_gains: [f32; 4], // [master, bgm, voice, sfx]
    bus_paused: [bool; 3],
    host_paused: bool,
    cache: Arc<Mutex<AudioCache>>,
    prefetch_tx: mpsc::Sender<PrefetchJob>,
    stream_tx: mpsc::Sender<StreamJob>,
}

impl AudioHost {
    pub fn new(prefs: &Preferences) -> Self {
        let cache = Arc::new(Mutex::new(AudioCache::new()));
        let (prefetch_tx, prefetch_rx) = mpsc::channel::<PrefetchJob>();
        let worker_cache = Arc::clone(&cache);
        let _ = std::thread::Builder::new()
            .name("nir-audio-prefetch".into())
            .spawn(move || {
                while let Ok(job) = prefetch_rx.recv() {
                    let decoded = decode_pcm_i16_from_pak(job.pak, job.start, job.len, job.ext);
                    let mut guard = worker_cache.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(pcm) = decoded {
                        guard.insert(job.id, Arc::new(pcm));
                    } else {
                        guard.inflight.remove(&job.id);
                    }
                }
            });

        // Reusable streaming decoder worker pool (eliminates per-clip thread spawn jitter!)
        let (stream_tx, stream_rx) = mpsc::channel::<StreamJob>();
        let shared_rx = Arc::new(Mutex::new(stream_rx));
        for idx in 0..STREAM_POOL_WORKERS {
            let rx = Arc::clone(&shared_rx);
            let _ = std::thread::Builder::new()
                .name(format!("nir-audio-stream-{idx}"))
                .spawn(move || loop {
                    let job = {
                        let guard = rx.lock().unwrap_or_else(|e| e.into_inner());
                        guard.recv()
                    };
                    let Ok(job) = job else {
                        return;
                    };
                    run_stream_job(job);
                });
        }

        let mut host = Self {
            // Open lazily on first `play()` so silent title screens / idle periods use 0 audio CPU!
            stream: None,
            stream_failed: Arc::new(AtomicBool::new(false)),
            last_stream_fail: None,
            idle_since: None,
            channels: HashMap::new(),
            suspended_loops: HashMap::new(),
            fading_out: Vec::new(),
            video_sink: None,
            bus_gains: [1.0; 4],
            bus_paused: [false; 3],
            host_paused: false,
            cache,
            prefetch_tx,
            stream_tx,
        };
        host.apply_preferences(prefs);
        host
    }

    fn ensure_stream(&mut self) -> Option<&OutputStream> {
        if self.stream.is_some() && self.stream_failed.load(Ordering::Acquire) {
            self.drop_broken_stream();
        }
        if self.stream.is_none() {
            if let Some(prev) = self.last_stream_fail {
                if prev.elapsed() < AUDIO_REOPEN_COOLDOWN {
                    return None;
                }
            }
            let failed_flag = Arc::clone(&self.stream_failed);
            failed_flag.store(false, Ordering::Release);
            let opened = OutputStreamBuilder::from_default_device().and_then(|builder| {
                builder
                    .with_error_callback(move |_err| {
                        failed_flag.store(true, Ordering::Release);
                    })
                    .open_stream()
            });
            match opened {
                Ok(mut s) => {
                    s.log_on_drop(false);
                    self.stream = Some(s);
                    self.last_stream_fail = None;
                    self.idle_since = None;
                }
                Err(_) => {
                    self.last_stream_fail = Some(Instant::now());
                }
            }
        }
        self.stream.as_ref()
    }

    fn drop_broken_stream(&mut self) {
        self.stream_failed.store(false, Ordering::Release);
        for (id, ch) in self.channels.drain() {
            ch.sink.stop();
            if ch.spec.looped {
                self.suspended_loops.insert(id, ch.spec);
            }
        }
        for f in self.fading_out.drain(..) {
            f.sink.stop();
        }
        if let Some(vs) = self.video_sink.take() {
            vs.stop();
        }
        self.stream = None;
        self.idle_since = None;
    }

    /// Stops every active channel, suspended loop, fade-out, and video audio stream (e.g. returning to Shelf).
    pub fn stop_all(&mut self) {
        self.suspended_loops.clear();
        for (_, ch) in self.channels.drain() {
            ch.sink.stop();
        }
        for f in self.fading_out.drain(..) {
            f.sink.stop();
        }
        if let Some(vs) = self.video_sink.take() {
            vs.stop();
        }
    }

    /// Attaches a streaming f32 PCM channel (used by native video audio decoding) to the output mixer.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn start_video_pcm_stream(
        &mut self,
        sample_rate: u32,
        channels: u16,
        rx: mpsc::Receiver<Vec<f32>>,
    ) -> bool {
        self.stop_video_audio();
        let Some(stream) = self.ensure_stream() else {
            return false;
        };
        let sink = Sink::connect_new(stream.mixer());
        sink.set_volume(self.bus_gains[0]);
        sink.append(StreamingAudioSource {
            rx,
            current: Vec::new(),
            pos: 0,
            sample_rate: sample_rate.max(8_000),
            channels: channels.clamp(1, 8),
        });
        if self.host_paused {
            sink.pause();
        }
        self.video_sink = Some(sink);
        self.idle_since = None;
        true
    }

    /// Stops any active video PCM stream.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn stop_video_audio(&mut self) {
        if let Some(vs) = self.video_sink.take() {
            vs.stop();
        }
    }

    fn resume_suspended_loops(&mut self) {
        if self.suspended_loops.is_empty() || self.host_paused {
            return;
        }
        let pending: Vec<(ChannelId, ChannelSpec)> = self.suspended_loops.drain().collect();
        for (channel, spec) in pending {
            self.play(
                &spec.pak,
                spec.asset,
                channel,
                spec.bus,
                spec.ext_hint,
                spec.meta_sample_rate,
                spec.meta_channels,
                spec.gain,
                spec.looped,
                120_000, // Gentle 120ms anti-click fade-in when resuming BGM after suspend/route change
                spec.loop_region,
            );
        }
    }

    /// Forces a clean audio stream retry (e.g. after returning from Android background).
    pub fn reset_stream_backoff(&mut self) {
        self.last_stream_fail = None;
    }

    pub fn is_stream_open(&self) -> bool {
        self.stream.is_some()
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

    /// Asynchronously pre-decodes short Voice/SFX assets into the PCM16 LRU cache (lookahead!).
    pub fn prefetch(&self, pak: &Arc<PakArchive>, id: AssetId, ext: &'static str) {
        let Some((start, len)) = pak.asset_range(id) else {
            return;
        };
        if len == 0 || len > SHORT_AUDIO_MAX_COMPRESSED {
            return;
        }
        let should_queue = {
            let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            !guard.entries.contains_key(&id) && guard.inflight.insert(id)
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
        self.stop(channel, 0);
        let Some((start, len)) = pak.asset_range(asset) else {
            return;
        };
        if len == 0 {
            return;
        }
        let spec = ChannelSpec {
            pak: Arc::clone(pak),
            asset,
            bus,
            ext_hint,
            meta_sample_rate,
            meta_channels,
            gain,
            looped,
            loop_region,
        };
        if self.host_paused && cfg!(target_os = "android") {
            if looped {
                self.suspended_loops.insert(channel, spec);
            }
            return;
        }
        let vol = self.effective_gain(bus, gain);
        let paused = self.host_paused || self.bus_paused[bus_index(bus)];
        let Some(stream) = self.ensure_stream() else {
            if looped {
                self.suspended_loops.insert(channel, spec);
            }
            return;
        };

        let sink = Sink::connect_new(stream.mixer());
        sink.set_volume(vol);
        if paused {
            sink.pause();
        }
        self.idle_since = None;

        // Fast path 1: Already decoded in PCM16 LRU cache (0ms startup, zero-copy Arc share!)
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
                    spec,
                },
            );
            return;
        }

        // Fast path 2: Dispatch to reusable streaming worker pool (zero thread spawn!).
        let cache_if_short = (!looped && len <= SHORT_AUDIO_MAX_COMPRESSED)
            .then(|| (Arc::clone(&self.cache), asset));

        let Some(stream_src) = StreamingAudioSource::dispatch(
            &self.stream_tx,
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
                spec,
            },
        );
    }

    pub fn stop(&mut self, channel: ChannelId, fade_out_us: u64) {
        self.suspended_loops.remove(&channel);
        if let Some(ch) = self.channels.remove(&channel) {
            if fade_out_us > 0 && !ch.sink.empty() && !self.host_paused {
                self.fading_out.push(FadingChannel {
                    sink: ch.sink,
                    bus: ch.bus,
                    start_gain: ch.base_gain,
                    started_at: Instant::now(),
                    duration: Duration::from_micros(fade_out_us),
                });
            } else {
                ch.sink.stop();
            }
        }
    }

    pub fn set_channel_gain(&mut self, channel: ChannelId, gain: f32) {
        if let Some(spec) = self.suspended_loops.get_mut(&channel) {
            spec.gain = gain;
        }
        let vol = self
            .channels
            .get(&channel)
            .map(|c| self.effective_gain(c.bus, gain));
        if let (Some(ch), Some(v)) = (self.channels.get_mut(&channel), vol) {
            ch.base_gain = gain;
            ch.spec.gain = gain;
            ch.sink.set_volume(v);
        }
    }

    pub fn set_bus_paused(&mut self, bus: AudioBus, paused: bool) {
        self.bus_paused[bus_index(bus)] = paused;
        let eff_paused = self.host_paused || paused;
        for ch in self.channels.values() {
            if ch.bus == bus {
                if eff_paused {
                    ch.sink.pause();
                } else {
                    ch.sink.play();
                }
            }
        }
        for f in &self.fading_out {
            if f.bus == bus {
                if eff_paused {
                    f.sink.pause();
                } else {
                    f.sink.play();
                }
            }
        }
    }

    /// Global lifecycle pause/resume (e.g. Android `suspended` / focus lost) preserving per-bus pauses.
    /// On Android, fully releases the AAudio output stream while backgrounded (0% audio hardware
    /// drain) and seamlessly resumes looping BGM upon return!
    pub fn set_host_paused(&mut self, paused: bool) {
        if self.host_paused == paused {
            return;
        }
        self.host_paused = paused;
        if paused {
            // Immediately finalize any in-flight fade-outs when backgrounded.
            for f in self.fading_out.drain(..) {
                f.sink.stop();
            }
            if cfg!(target_os = "android") {
                for (id, ch) in self.channels.drain() {
                    ch.sink.stop();
                    if ch.spec.looped {
                        self.suspended_loops.insert(id, ch.spec);
                    }
                }
                if let Some(vs) = &self.video_sink {
                    vs.pause();
                } else {
                    self.stream = None;
                    self.idle_since = None;
                }
                return;
            }
        } else {
            self.last_stream_fail = None;
            if let Some(vs) = &self.video_sink {
                vs.play();
            }
        }
        for ch in self.channels.values() {
            if paused || self.bus_paused[bus_index(ch.bus)] {
                ch.sink.pause();
            } else {
                ch.sink.play();
            }
        }
        if paused && self.channels.is_empty() && self.fading_out.is_empty() && self.video_sink.is_none() {
            self.stream = None;
            self.idle_since = None;
        } else if !paused {
            self.resume_suspended_loops();
        }
    }

    /// Pauses or resumes game audio channels while keeping the audio output stream alive for
    /// embedded video playback (`video_sink`).
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn set_game_paused_for_video(&mut self, paused: bool) {
        for ch in self.channels.values() {
            if paused || self.host_paused || self.bus_paused[bus_index(ch.bus)] {
                ch.sink.pause();
            } else {
                ch.sink.play();
            }
        }
    }

    /// Advances active fade-outs, recovers from AAudio/device disconnects, suspends idle OS output
    /// streams after silence, and returns channel IDs that naturally finished playing.
    pub fn poll_ended(&mut self) -> Vec<ChannelId> {
        if self.stream_failed.load(Ordering::Acquire) {
            self.drop_broken_stream();
        }
        if !self.suspended_loops.is_empty() && !self.host_paused {
            self.resume_suspended_loops();
        }

        let now = Instant::now();
        let bus_gains = self.bus_gains;
        self.fading_out.retain(|f| {
            if f.sink.empty() {
                return false;
            }
            let elapsed = now.saturating_duration_since(f.started_at);
            if elapsed >= f.duration {
                f.sink.stop();
                return false;
            }
            let frac = (1.0 - elapsed.as_secs_f32() / f.duration.as_secs_f32()).clamp(0.0, 1.0);
            let vol = effective_gain_with(&bus_gains, f.bus, f.start_gain * frac);
            f.sink.set_volume(vol);
            true
        });

        let mut ended = Vec::new();
        self.channels.retain(|&id, ch| {
            if ch.sink.empty() {
                ended.push(id);
                false
            } else {
                true
            }
        });

        if let Some(vs) = &self.video_sink {
            if vs.empty() {
                self.video_sink = None;
            }
        }

        if self.channels.is_empty() && self.fading_out.is_empty() && self.video_sink.is_none() {
            if self.stream.is_some() {
                match self.idle_since {
                    None => self.idle_since = Some(now),
                    Some(since)
                        if now.saturating_duration_since(since) >= AUDIO_IDLE_SUSPEND_DELAY =>
                    {
                        self.stream = None;
                        self.idle_since = None;
                    }
                    Some(_) => {}
                }
            }
        } else {
            self.idle_since = None;
        }
        ended
    }

    /// Exact microseconds until the audio host next needs a wakeup (fade-out step or idle auto-suspend).
    pub fn next_wakeup_delay_us(&self) -> Option<u64> {
        if !self.fading_out.is_empty() {
            return Some(FADE_TICK_US);
        }
        if self.channels.is_empty() && self.stream.is_some() {
            let now = Instant::now();
            if let Some(since) = self.idle_since {
                let elapsed = now.saturating_duration_since(since);
                let rem = AUDIO_IDLE_SUSPEND_DELAY.saturating_sub(elapsed);
                return Some(rem.as_micros().max(1_000) as u64);
            }
            return Some(AUDIO_IDLE_SUSPEND_DELAY.as_micros() as u64);
        }
        None
    }

    fn effective_gain(&self, bus: AudioBus, base_gain: f32) -> f32 {
        effective_gain_with(&self.bus_gains, bus, base_gain)
    }

    fn refresh_volumes(&self) {
        let now = Instant::now();
        for ch in self.channels.values() {
            ch.sink
                .set_volume(self.effective_gain(ch.bus, ch.base_gain));
        }
        for f in &self.fading_out {
            let elapsed = now.saturating_duration_since(f.started_at);
            let frac = if f.duration.is_zero() {
                0.0
            } else {
                (1.0 - elapsed.as_secs_f32() / f.duration.as_secs_f32()).clamp(0.0, 1.0)
            };
            f.sink
                .set_volume(self.effective_gain(f.bus, f.start_gain * frac));
        }
    }
}

fn effective_gain_with(bus_gains: &[f32; 4], bus: AudioBus, base_gain: f32) -> f32 {
    let b = match bus {
        AudioBus::Bgm => bus_gains[1],
        AudioBus::Voice => bus_gains[2],
        AudioBus::Sfx => bus_gains[3],
    };
    (bus_gains[0] * b * base_gain).clamp(0.0, 2.0)
}

fn bus_index(bus: AudioBus) -> usize {
    match bus {
        AudioBus::Bgm => 0,
        AudioBus::Voice => 1,
        AudioBus::Sfx => 2,
    }
}

fn is_opus_stream(pak: &PakArchive, start: usize, len: usize, ext: &str) -> bool {
    if ext == "opus" {
        return true;
    }
    let slice = &pak.raw_slice()[start..start + len.min(128)];
    slice.windows(8).any(|w| w == b"OpusHead")
}

fn lame_trim_samples(
    pak: &PakArchive,
    start: usize,
    len: usize,
    ext: &str,
    channels: u16,
) -> (usize, usize) {
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

fn decode_ogg_opus_i16_from_pak(
    pak: Arc<PakArchive>,
    start: usize,
    len: usize,
) -> Option<DecodedPcm> {
    let reader = PakSliceReader::new(pak, start, len);
    let mut pr = PacketReader::new(reader);
    let head = pr.read_packet().ok()??;
    if !head.data.starts_with(b"OpusHead") || head.data.len() < 19 {
        return None;
    }
    let channels = (head.data[9] as u16).clamp(1, 2);
    let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]) as usize;
    let _tags = pr.read_packet().ok()??;

    let mut dec = LibOpusDecoder::new(channels)?;
    let ch = channels as usize;
    let mut pcm: Vec<i16> = Vec::new();
    let mut frame_buf = vec![0i16; 5760 * ch];

    while let Ok(Some(pkt)) = pr.read_packet() {
        if pkt.data.is_empty() {
            continue;
        }
        if let Some(n) = dec.decode_i16(&pkt.data, &mut frame_buf) {
            pcm.extend_from_slice(&frame_buf[..n]);
        }
    }
    if pcm.is_empty() {
        return None;
    }
    let lead = (pre_skip * ch).min(pcm.len());
    let end = pcm.len();
    Some(DecodedPcm {
        samples: pcm,
        start: lead,
        end,
        sample_rate: 48_000,
        channels,
    })
}

/// Decodes a short audio asset directly from `Arc<PakArchive>` into compact PCM16.
fn decode_pcm_i16_from_pak(
    pak: Arc<PakArchive>,
    start: usize,
    len: usize,
    ext_hint: &str,
) -> Option<DecodedPcm> {
    if is_opus_stream(&pak, start, len, ext_hint) {
        return decode_ogg_opus_i16_from_pak(pak, start, len);
    }

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
    let mut pcm: Vec<i16> = Vec::with_capacity(est_samples);
    let mut sbuf: Option<SampleBuffer<i16>> = None;

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
            slot => slot.insert(SampleBuffer::<i16>::new(cap, spec)),
        };
        buf.copy_interleaved_ref(decoded);
        pcm.extend_from_slice(buf.samples());
    }

    if pcm.is_empty() {
        return None;
    }

    let (lead, trail) =
        lame_trim_samples(&lead_hint, trail_hint.0, trail_hint.1, ext_hint, channels);
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

/// Low-memory background streaming source backed by the reusable streaming decoder pool.
struct StreamingAudioSource {
    rx: mpsc::Receiver<Vec<f32>>,
    current: Vec<f32>,
    pos: usize,
    sample_rate: u32,
    channels: u16,
}

impl StreamingAudioSource {
    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        pool_tx: &mpsc::Sender<StreamJob>,
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

        pool_tx
            .send(StreamJob {
                pak,
                start,
                len,
                ext_hint,
                looped,
                loop_region,
                tx: chunk_tx,
                meta_tx,
                cache_short,
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

fn run_stream_job(job: StreamJob) {
    if is_opus_stream(&job.pak, job.start, job.len, job.ext_hint) {
        stream_opus_decode_loop(job);
    } else {
        stream_symphonia_decode_loop(job);
    }
}

fn stream_opus_decode_loop(mut job: StreamJob) {
    let mut first_pass = true;
    let mut out_chunk = Vec::with_capacity(STREAM_CHUNK_SAMPLES);
    let mut cache_accum: Option<Vec<i16>> = job.cache_short.as_ref().map(|_| Vec::new());

    loop {
        let reader = PakSliceReader::new(Arc::clone(&job.pak), job.start, job.len);
        let mut pr = PacketReader::new(reader);
        let Ok(Some(head)) = pr.read_packet() else {
            if let Some(m) = job.meta_tx.take() {
                let _ = m.send(None);
            }
            return;
        };
        if !head.data.starts_with(b"OpusHead") || head.data.len() < 19 {
            if let Some(m) = job.meta_tx.take() {
                let _ = m.send(None);
            }
            return;
        }
        let ch = (head.data[9] as u16).clamp(1, 2);
        let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]) as usize;
        let _ = pr.read_packet(); // OpusTags
        if let Some(m) = job.meta_tx.take() {
            let _ = m.send(Some((48_000, ch)));
        }

        let Some(mut dec) = LibOpusDecoder::new(ch) else {
            return;
        };
        let ch_usize = ch as usize;
        let (loop_start_sample, loop_end_sample) = if let Some(r) = job.loop_region {
            let ls = ((r.start_us as u128 * 48_000u128) / 1_000_000) as usize * ch_usize;
            let le = ((r.end_us as u128 * 48_000u128) / 1_000_000) as usize * ch_usize;
            if ls < le {
                (ls, Some(le))
            } else {
                (0, None)
            }
        } else {
            (0, None)
        };

        let skip_after_lead = if first_pass { 0 } else { loop_start_sample };
        let mut total_skip = pre_skip * ch_usize + skip_after_lead;
        let mut emitted = skip_after_lead;
        let mut frame_buf = vec![0i16; 5760 * ch_usize];
        let mut hit_loop_end = false;

        while let Ok(Some(pkt)) = pr.read_packet() {
            if pkt.data.is_empty() {
                continue;
            }
            let Some(n) = dec.decode_i16(&pkt.data, &mut frame_buf) else {
                continue;
            };
            let mut slice = &frame_buf[..n];
            if total_skip > 0 {
                if total_skip >= slice.len() {
                    total_skip -= slice.len();
                    continue;
                }
                slice = &slice[total_skip..];
                total_skip = 0;
            }
            for &s_i16 in slice {
                if let Some(end_at) = loop_end_sample {
                    if emitted >= end_at {
                        hit_loop_end = true;
                        break;
                    }
                }
                emitted += 1;
                if let Some(acc) = cache_accum.as_mut() {
                    acc.push(s_i16);
                }
                out_chunk.push(s_i16 as f32 * INV_I16_SCALE);
                if out_chunk.len() >= STREAM_CHUNK_SAMPLES {
                    let full =
                        std::mem::replace(&mut out_chunk, Vec::with_capacity(STREAM_CHUNK_SAMPLES));
                    if job.tx.send(full).is_err() {
                        return;
                    }
                }
            }
            if hit_loop_end {
                break;
            }
        }

        if !job.looped {
            if !out_chunk.is_empty() {
                let _ = job.tx.send(out_chunk);
            }
            if let (Some((cache, id)), Some(samples)) = (job.cache_short.take(), cache_accum.take())
            {
                if !samples.is_empty() {
                    let end = samples.len();
                    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
                    guard.insert(
                        id,
                        Arc::new(DecodedPcm {
                            samples,
                            start: 0,
                            end,
                            sample_rate: 48_000,
                            channels: ch,
                        }),
                    );
                }
            }
            return;
        }
        first_pass = false;
        cache_accum = None;
        job.cache_short = None;
    }
}

fn stream_symphonia_decode_loop(mut job: StreamJob) {
    let mut first_pass = true;
    let mut out_chunk = Vec::with_capacity(STREAM_CHUNK_SAMPLES);
    let mut cache_accum: Option<Vec<i16>> = job.cache_short.as_ref().map(|_| Vec::new());

    loop {
        let mut hint = Hint::new();
        if !job.ext_hint.is_empty() {
            hint.with_extension(job.ext_hint);
        }
        let reader = PakSliceReader::new(Arc::clone(&job.pak), job.start, job.len);
        let mss = MediaSourceStream::new(Box::new(reader), Default::default());
        let Ok(probed) = symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        ) else {
            if let Some(m) = job.meta_tx.take() {
                let _ = m.send(None);
            }
            return;
        };
        let mut format = probed.format;
        let Some(track) = format.default_track() else {
            if let Some(m) = job.meta_tx.take() {
                let _ = m.send(None);
            }
            return;
        };
        let Some(sample_rate) = track.codec_params.sample_rate else {
            if let Some(m) = job.meta_tx.take() {
                let _ = m.send(None);
            }
            return;
        };
        let Some(ch) = track.codec_params.channels.map(|c| c.count() as u16) else {
            if let Some(m) = job.meta_tx.take() {
                let _ = m.send(None);
            }
            return;
        };
        let track_id = track.id;
        if let Some(m) = job.meta_tx.take() {
            let _ = m.send(Some((sample_rate, ch.max(1))));
        }

        let Ok(mut decoder) =
            symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())
        else {
            return;
        };

        let (lead, trail) = lame_trim_samples(&job.pak, job.start, job.len, job.ext_hint, ch);
        let ch_usize = ch.max(1) as usize;
        let (loop_start_sample, loop_end_sample) = if let Some(r) = job.loop_region {
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

        let emit_sample =
            |s: f32, out_chunk: &mut Vec<f32>, cache_accum: &mut Option<Vec<i16>>| -> bool {
                if let Some(acc) = cache_accum.as_mut() {
                    acc.push(f32_to_i16(s));
                }
                out_chunk.push(s);
                if out_chunk.len() >= STREAM_CHUNK_SAMPLES {
                    let full =
                        std::mem::replace(out_chunk, Vec::with_capacity(STREAM_CHUNK_SAMPLES));
                    if job.tx.send(full).is_err() {
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

        if !job.looped {
            if !out_chunk.is_empty() {
                let _ = job.tx.send(out_chunk);
            }
            if let (Some((cache, id)), Some(samples)) = (job.cache_short.take(), cache_accum.take())
            {
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
        job.cache_short = None;
    }
}

/// Zero-copy sample-accurate looping PCM16 source backed by shared `Arc<DecodedPcm>`.
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
        Some(sample as f32 * INV_I16_SCALE)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libopus_decoder_initializes_and_decodes_silence_frame() {
        let mut dec = LibOpusDecoder::new(1).expect("mono opus decoder");
        // Standard Opus TOC-only 1-byte DTX/silence packet (0xF8 = CELT FB 20ms mono, 1 frame)
        let mut out = vec![0i16; 5760];
        let decoded = dec.decode_i16(&[0xF8, 0xFF, 0xFE], &mut out);
        assert!(decoded.unwrap_or(0) > 0);
    }

    #[test]
    fn pcm16_audio_lru_cache_accounts_bytes_and_replaces() {
        let mut cache = AudioCache::new();
        let mk = |n: usize| {
            Arc::new(DecodedPcm {
                samples: vec![0i16; n],
                start: 0,
                end: n,
                sample_rate: 48_000,
                channels: 1,
            })
        };
        cache.insert(AssetId(1), mk(20)); // 40 bytes (PCM16 = 2 bytes/sample)
        assert_eq!(cache.bytes, 40);
        cache.insert(AssetId(2), mk(30)); // 60 bytes
        assert_eq!(cache.bytes, 100);
        cache.insert(AssetId(1), mk(10)); // replace 40 -> 20 bytes
        assert_eq!(cache.bytes, 80);
        assert!(cache.get(AssetId(1)).is_some());
        assert!(cache.get(AssetId(2)).is_some());
    }

    #[test]
    fn audio_host_starts_lazy_with_no_open_stream_or_wakeup() {
        let host = AudioHost::new(&Preferences::default());
        assert!(!host.is_stream_open());
        assert!(host.next_wakeup_delay_us().is_none());
    }
}
