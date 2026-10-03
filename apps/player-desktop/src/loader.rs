//! Worker pool for object reads and media decode off the owner thread.
//!
//! Up to four workers share one job channel; the receive mutex is held only
//! around `recv`, so one worker parks on the channel while the others decode.
//! A global byte budget bounds decode memory held concurrently: image and
//! audio jobs acquire the manifest-declared `decoded_bytes` before decoding,
//! and the owner thread releases the tokens when it drains the result, so
//! slow delivery applies backpressure to decoding instead of growing memory.
use crate::Bundle;
use anyhow::{anyhow, ensure, Result};
use nir_format::{Asset, AssetKind};
use rodio::{Decoder, Source};
use std::{
    io::Cursor,
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
};

/// Upper bound on decode memory held concurrently across all workers.
const DECODE_BUDGET_BYTES: u64 = 128 * 1024 * 1024;

pub(crate) enum Job {
    Content {
        request: u32,
        hashes: Vec<String>,
        limit: usize,
    },
    Asset {
        request: u32,
        id: String,
        descriptor: Asset,
    },
}
pub(crate) enum AssetData {
    /// Encoded object bytes (fonts, and audio pending owner admission);
    /// the owner thread re-verifies the digest before use.
    Bytes(Arc<[u8]>),
    /// A digest-verified, decoded and premultiplied image. The host attests
    /// verification; `Engine::resource_decoded` re-checks dimensions.
    Decoded {
        width: u32,
        height: u32,
        pixels: Arc<[u8]>,
    },
    /// Decoded audio samples plus the encoded bytes for owner admission.
    Audio {
        samples: Arc<Vec<f32>>,
        channels: u16,
        rate: u32,
        bytes: Arc<[u8]>,
    },
}
pub(crate) enum Loaded {
    Content {
        request: u32,
        data: std::result::Result<Vec<Vec<u8>>, String>,
    },
    Asset {
        request: u32,
        id: String,
        /// Budget tokens this result still holds; the owner releases them.
        tokens: u64,
        data: std::result::Result<AssetData, String>,
    },
}
struct Budget {
    total: u64,
    remaining: Mutex<u64>,
    available: Condvar,
}
impl Budget {
    fn new(total: u64) -> Self {
        Self {
            total,
            remaining: Mutex::new(total),
            available: Condvar::new(),
        }
    }
    fn clamp(&self, want: u64) -> u64 {
        want.min(self.total)
    }
    /// Blocks until `want` tokens are free. A single asset larger than the
    /// whole budget takes the entire budget instead of waiting forever.
    fn acquire(&self, want: u64) {
        let want = self.clamp(want);
        let mut remaining = self.remaining.lock().unwrap();
        while *remaining < want {
            remaining = self.available.wait(remaining).unwrap();
        }
        *remaining -= want;
    }
    fn release(&self, want: u64) {
        let mut remaining = self.remaining.lock().unwrap();
        *remaining = (*remaining + self.clamp(want)).min(self.total);
        drop(remaining);
        self.available.notify_all();
    }
}
pub(crate) struct Loader {
    send: mpsc::Sender<Job>,
    results: mpsc::Receiver<Loaded>,
    budget: Arc<Budget>,
    workers: usize,
    outstanding: usize,
}
impl Loader {
    pub(crate) fn new(bundle: Arc<Bundle>) -> Self {
        let workers = thread::available_parallelism()
            .map_or(2, |n| n.get())
            .clamp(2, 4);
        let (send, jobs) = mpsc::channel::<Job>();
        let (done, results) = mpsc::sync_channel::<Loaded>(2);
        let budget = Arc::new(Budget::new(DECODE_BUDGET_BYTES));
        let shared = Arc::new(Mutex::new(jobs));
        let mut spawned = 0;
        for _ in 0..workers {
            let worker = thread::Builder::new().name("nir-loader".into()).spawn({
                let bundle = bundle.clone();
                let jobs = shared.clone();
                let results = done.clone();
                let budget = budget.clone();
                move || run_worker(bundle, jobs, results, budget)
            });
            match worker {
                Ok(_) => spawned += 1,
                Err(_) => break,
            }
        }
        Self {
            send,
            results,
            budget,
            workers: spawned.max(1),
            outstanding: 0,
        }
    }
    #[cfg(test)]
    pub(crate) fn workers(&self) -> usize {
        self.workers
    }
    pub(crate) fn outstanding(&self) -> usize {
        self.outstanding
    }
    pub(crate) fn has_capacity(&self) -> bool {
        self.outstanding < self.workers
    }
    pub(crate) fn dispatch(&mut self, job: Job) -> Result<()> {
        self.send
            .send(job)
            .map_err(|_| anyhow!("E_WORKER_UNAVAILABLE"))?;
        self.outstanding += 1;
        Ok(())
    }
    /// Takes at most one finished result and releases its decode budget.
    /// One delivery per owner turn keeps ADR-007 admission pacing.
    pub(crate) fn drain(&mut self) -> Option<Loaded> {
        let loaded = self.results.try_recv().ok()?;
        self.outstanding -= 1;
        if let Loaded::Asset { tokens, .. } = &loaded {
            self.budget.release(*tokens);
        }
        Some(loaded)
    }
}
fn run_worker(
    bundle: Arc<Bundle>,
    jobs: Arc<Mutex<mpsc::Receiver<Job>>>,
    results: mpsc::SyncSender<Loaded>,
    budget: Arc<Budget>,
) {
    loop {
        // The lock guards only the recv; decoding runs without it.
        let job = jobs.lock().unwrap().recv();
        let Ok(job) = job else {
            return;
        };
        let loaded = match job {
            Job::Content {
                request,
                hashes,
                limit,
            } => Loaded::Content {
                request,
                data: load_content(&bundle, &hashes, limit).map_err(|e| e.to_string()),
            },
            Job::Asset {
                request,
                id,
                descriptor,
            } => {
                let data = load_asset(&bundle, &budget, &descriptor);
                match data {
                    Ok((data, tokens)) => Loaded::Asset {
                        request,
                        id,
                        tokens,
                        data: Ok(data),
                    },
                    Err(e) => Loaded::Asset {
                        request,
                        id,
                        tokens: 0,
                        data: Err(e.to_string()),
                    },
                }
            }
        };
        if results.send(loaded).is_err() {
            return;
        }
    }
}
fn load_content(bundle: &Bundle, hashes: &[String], limit: usize) -> Result<Vec<Vec<u8>>> {
    let mut used = 0usize;
    let mut data = Vec::new();
    ensure!(hashes.len() <= 128, "E_CONTENT_LIMIT");
    for hash in hashes {
        let size = bundle
            .manifest
            .objects
            .get(hash)
            .ok_or_else(|| anyhow!("E_OBJECT_REFERENCE"))?
            .bytes;
        ensure!(size <= limit.saturating_sub(used) as u64, "E_CONTENT_LIMIT");
        let bytes = bundle.object(hash)?;
        used += bytes.len();
        data.push(bytes);
    }
    Ok(data)
}
fn load_asset(bundle: &Bundle, budget: &Budget, descriptor: &Asset) -> Result<(AssetData, u64)> {
    let tokens = match descriptor.kind {
        AssetKind::Image | AssetKind::Audio => descriptor.decoded_bytes,
        AssetKind::Font => 0,
    };
    if tokens > 0 {
        budget.acquire(tokens);
    }
    // Any failure after acquiring must give the tokens back immediately;
    // success transfers them to the result for the owner to release.
    match decode_asset(bundle, descriptor) {
        Ok(data) => Ok((data, tokens)),
        Err(e) => {
            budget.release(tokens);
            Err(e)
        }
    }
}
/// Aligns a tag-unaware (whole-frames) MP3 decode back to the authored
/// window. The LAME tag's delay is relative to the fixed 528+1 MP3 decoder
/// delay — symphonia adds the same amount itself when it applies the tag
/// (demuxer.rs: `528 + 1 + trim`) — so the authored audio in an unaware
/// decode starts 529 samples per channel further in than the tag delay.
fn trim_unaware_mp3(samples: &mut Vec<f32>, delay: u32, channels: u16, authored: u64) {
    let skip = ((529 + delay as usize) * channels as usize).min(samples.len());
    samples.drain(..skip);
    samples.truncate(authored as usize);
}
fn decode_asset(bundle: &Bundle, descriptor: &Asset) -> Result<AssetData> {
    let bytes: Arc<[u8]> = bundle.object(&descriptor.object)?.into();
    match descriptor.kind {
        AssetKind::Image => {
            let (width, height, pixels) = nir_render_wgpu::decode_image(&bytes)?;
            // The compiler derived the descriptor dimensions from the same
            // digest-pinned bytes; a mismatch means a forged manifest, and
            // failing here keeps the pixels from ever reaching admission.
            ensure!(
                width == descriptor.width && height == descriptor.height,
                "E_IMAGE_DIMENSION"
            );
            Ok(AssetData::Decoded {
                width,
                height,
                pixels: pixels.into(),
            })
        }
        AssetKind::Audio => {
            let decoder = Decoder::try_from(Cursor::new(bytes.clone()))?;
            let channels = decoder.channels();
            let rate = decoder.sample_rate();
            // MP3 objects keep the WAV-derived decode budget: their LAME
            // gapless tag declares how many encoder delay/padding samples
            // surround the authored audio. Tag-aware decoders (symphonia
            // here, decodeAudioData in browsers) cut delay and padding
            // themselves and hand back exactly the authored count; a
            // tag-unaware decode instead yields whole frames, which always
            // exceeds the authored count by at least the encoder delay. Trim
            // only that shape back into alignment. Untagged WAV keeps the
            // exact manifest cap.
            let gapless = nir_format::lame::parse(&bytes);
            let authored = descriptor.decoded_bytes / 4;
            let cap = match &gapless {
                Some(tag) => {
                    authored + (tag.delay + tag.padding + 1152 * 2) as u64 * channels as u64
                }
                None => authored,
            };
            let mut samples: Vec<f32> = Vec::new();
            for sample in decoder {
                ensure!((samples.len() as u64) < cap, "E_AUDIO_SIZE");
                samples.push(sample);
            }
            if let Some(tag) = &gapless {
                ensure!(rate == tag.rate, "E_AUDIO_RATE");
                if samples.len() as u64 > authored {
                    trim_unaware_mp3(&mut samples, tag.delay, channels, authored);
                }
            }
            ensure!(samples.len() as u64 == authored, "E_AUDIO_SIZE");
            Ok(AssetData::Audio {
                samples: Arc::new(samples),
                channels,
                rate,
                bytes,
            })
        }
        AssetKind::Font => Ok(AssetData::Bytes(bytes)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nir_format::{Micros, NativeRelease, Object, MAX_INPUT_BYTES};
    use std::{
        collections::BTreeMap,
        fs,
        path::Path,
        time::{Duration, Instant},
    };

    /// A published bundle with one object per entry, keyed by content digest.
    fn bundle_with(objects: &[(Vec<u8>, &str)]) -> (tempfile::TempDir, Arc<Bundle>) {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("objects")).unwrap();
        fs::create_dir(temp.path().join("releases")).unwrap();
        let mut manifest = NativeRelease {
            format: 1,
            game_id: "game".into(),
            title: "title".into(),
            version: "1".into(),
            profile: "release".into(),
            engine_build: "a".repeat(64),
            player: "b".repeat(64),
            program: String::new(),
            objects: BTreeMap::new(),
        };
        for (bytes, extension) in objects {
            let hash = nir_content::digest(bytes);
            let path = format!("objects/{hash}.{extension}");
            fs::write(temp.path().join(&path), bytes).unwrap();
            manifest.objects.insert(
                hash,
                Object {
                    path,
                    bytes: bytes.len() as u64,
                    media_type: "application/octet-stream".into(),
                },
            );
        }
        manifest.program = manifest.objects.keys().next().unwrap().clone();
        let id = nir_content::digest(&serde_json::to_vec(&manifest).unwrap());
        fs::write(
            temp.path().join(format!("releases/{id}.json")),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(temp.path().join("release.txt"), &id).unwrap();
        let bundle = Arc::new(Bundle::open(temp.path()).unwrap());
        (temp, bundle)
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut encoded = Cursor::new(Vec::new());
        image::RgbaImage::new(width, height)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        encoded.into_inner()
    }

    fn drain_all(loader: &mut Loader) -> Vec<Loaded> {
        let mut drained = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while loader.outstanding() > 0 {
            assert!(Instant::now() < deadline, "loader results never arrived");
            if let Some(loaded) = loader.drain() {
                drained.push(loaded);
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        drained
    }

    #[test]
    fn budget_gates_and_returns_declared_decode_tokens() {
        let budget = Budget::new(100);
        budget.acquire(60);
        assert_eq!(*budget.remaining.lock().unwrap(), 40);
        budget.release(60);
        assert_eq!(*budget.remaining.lock().unwrap(), 100);
        // Oversized wants clamp to the total instead of waiting forever.
        budget.acquire(250);
        assert_eq!(*budget.remaining.lock().unwrap(), 0);
        budget.release(250);
        assert_eq!(*budget.remaining.lock().unwrap(), 100);
    }

    #[test]
    fn images_decode_off_thread_and_results_match_jobs() {
        let png = png_bytes(3, 2);
        let (_temp, bundle) = bundle_with(&[(png.clone(), "png")]);
        let descriptor = Asset {
            kind: AssetKind::Image,
            object: nir_content::digest(&png),
            bytes: png.len() as u64,
            width: 3,
            height: 2,
            duration_us: Micros(0),
            decoded_bytes: 3 * 2 * 4,
        };
        let mut loader = Loader::new(bundle);
        let workers = loader.workers();
        assert!((2..=4).contains(&workers));
        loader
            .dispatch(Job::Asset {
                request: 1,
                id: "bg".into(),
                descriptor: descriptor.clone(),
            })
            .unwrap();
        loader
            .dispatch(Job::Asset {
                request: 1,
                id: "broken".into(),
                descriptor: Asset {
                    object: nir_content::digest(b"not an image"),
                    bytes: 13,
                    ..descriptor
                },
            })
            .unwrap();
        assert_eq!(loader.outstanding(), 2);
        let drained = drain_all(&mut loader);
        assert_eq!(drained.len(), 2);
        assert_eq!(loader.outstanding(), 0);
        let mut decoded = None;
        let mut failed = None;
        for loaded in drained {
            let Loaded::Asset {
                id, tokens, data, ..
            } = loaded
            else {
                panic!("unexpected content result");
            };
            match (id.as_str(), data) {
                (
                    "bg",
                    Ok(AssetData::Decoded {
                        width,
                        height,
                        pixels,
                    }),
                ) => {
                    assert_eq!((width, height), (3, 2));
                    assert_eq!(pixels.len(), 3 * 2 * 4);
                    // A transparent source decodes to fully zeroed RGBA.
                    assert!(pixels.iter().all(|b| *b == 0));
                    decoded = Some(tokens);
                }
                ("broken", Err(_)) => failed = Some(tokens),
                (id, Ok(_)) => panic!("unexpected asset success for {id}"),
                (id, Err(_)) => panic!("unexpected asset failure for {id}"),
            }
        }
        // The failed decode returned its tokens on the worker; the drained
        // results carry none, so the whole budget is free again.
        assert_eq!(decoded.unwrap(), 3 * 2 * 4);
        assert_eq!(failed.unwrap(), 0);
    }

    /// Decodes audio bytes through the worker path exactly as a packaged
    /// asset with the given frame count would be decoded.
    fn decode_audio(bytes: &[u8], frames: u64) -> Vec<f32> {
        let (_temp, bundle) = bundle_with(&[(bytes.to_vec(), "bin")]);
        let descriptor = Asset {
            kind: AssetKind::Audio,
            object: nir_content::digest(bytes),
            bytes: bytes.len() as u64,
            width: 0,
            height: 0,
            duration_us: Micros(500_000),
            decoded_bytes: frames * 4,
        };
        match decode_asset(&bundle, &descriptor).unwrap() {
            AssetData::Audio { samples, .. } => samples.to_vec(),
            _ => panic!("unexpected asset data for audio bytes"),
        }
    }

    #[test]
    fn mp3_gapless_trim_matches_the_authored_wav_sample_for_sample() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let wav = fs::read(dir.join("gapless-44100-mono.wav")).unwrap();
        let mp3 = fs::read(dir.join("gapless-44100-mono.mp3")).unwrap();
        // The tag the trim follows, plus the raw count the decoder yields
        // before any loader trim — symphonia is tag-aware and already cut
        // delay and padding, so raw equals the authored count exactly.
        let gapless = nir_format::lame::parse(&mp3).unwrap();
        let raw: Vec<f32> = Decoder::try_from(Cursor::new(mp3.clone()))
            .unwrap()
            .collect();
        assert_eq!(raw.len(), 22_050, "tag-aware decode is pre-trimmed");
        eprintln!(
            "gapless: delay={} padding={} raw_decoded={} authored={}",
            gapless.delay,
            gapless.padding,
            raw.len(),
            22_050
        );
        let reference = decode_audio(&wav, 22_050);
        assert_eq!(reference.len(), 22_050);
        let converted = decode_audio(&mp3, 22_050);
        assert_eq!(
            converted.len(),
            22_050,
            "trimmed decode must return the authored sample count"
        );
        // A 0.5 s chirp tightens from ~200 to ~50 samples per period, so a
        // one-frame misalignment cannot hide behind a low error floor: slide
        // the converted stream against the reference and require the best
        // alignment at offset zero with encoding-level residual only.
        let window = 18_000usize;
        let skip = 2_000usize;
        let mut best = (f32::INFINITY, 0i32);
        for offset in -2400i32..=2400 {
            let start = skip as i64 + offset as i64;
            if start < 0 || (start + window as i64) as usize > converted.len() {
                continue;
            }
            let mut sum = 0.0f64;
            for i in 0..window {
                let delta = (reference[skip + i] - converted[start as usize + i]) as f64;
                sum += delta * delta;
            }
            let rms = (sum / window as f64).sqrt() as f32;
            if rms < best.0 {
                best = (rms, offset);
            }
        }
        eprintln!("alignment: best offset {} rms {}", best.1, best.0);
        assert_eq!(best.1, 0, "trimmed stream must start at the first sample");
        assert!(best.0 < 0.05, "residual beyond encoding noise: {}", best.0);
    }

    #[test]
    fn decoded_mp3_loops_without_inserting_delay_or_padding() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gapless-44100-mono.mp3");
        let samples = decode_audio(&fs::read(path).unwrap(), 22_050);
        let buffer =
            crate::audio_source::AudioBuffer::from_parts(Arc::new(samples.clone()), 1, 44_100);
        for (index, sample) in buffer.source(0, true).take(samples.len() * 100).enumerate() {
            assert_eq!(sample, samples[index % samples.len()]);
        }
    }

    #[test]
    fn short_audio_decode_fails_the_authored_sample_contract() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gapless-44100-mono.mp3");
        let bytes = fs::read(path).unwrap();
        let (_temp, bundle) = bundle_with(&[(bytes.clone(), "mp3")]);
        let descriptor = Asset {
            kind: AssetKind::Audio,
            object: nir_content::digest(&bytes),
            bytes: bytes.len() as u64,
            width: 0,
            height: 0,
            duration_us: Micros(500_000),
            decoded_bytes: 22_051 * 4,
        };
        assert!(decode_asset(&bundle, &descriptor)
            .err()
            .unwrap()
            .to_string()
            .contains("E_AUDIO_SIZE"));
    }

    #[test]
    fn unaware_decode_trim_skips_the_fixed_decoder_delay() {
        // A tag-unaware decode (whole frames) places the authored audio at
        // 529 + tag.delay samples per channel: the tag's delay is relative
        // to the fixed 528+1 MP3 decoder delay. Rebuild that shape around
        // the fixture's WAV and require the trim to recover it exactly.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let wav = fs::read(dir.join("gapless-44100-mono.wav")).unwrap();
        let mp3 = fs::read(dir.join("gapless-44100-mono.mp3")).unwrap();
        let gapless = nir_format::lame::parse(&mp3).unwrap();
        let authored = decode_audio(&wav, 22_050);
        let head = 529 + gapless.delay as usize;
        let total = nir_format::lame::untrimmed_samples(&gapless, 22_050, 1152) as usize;
        assert!(total >= head + 22_050);
        let mut unaware = vec![0.0f32; head];
        unaware.extend_from_slice(&authored);
        unaware.resize(total, 0.0);
        trim_unaware_mp3(&mut unaware, gapless.delay, 1, 22_050);
        assert_eq!(unaware.len(), 22_050);
        assert_eq!(unaware, authored);
    }

    #[test]
    fn content_results_carry_objects_and_missing_references_fail() {
        let payload = b"chapter-bytes".to_vec();
        let (_temp, bundle) = bundle_with(&[(payload.clone(), "json")]);
        let mut loader = Loader::new(bundle);
        loader
            .dispatch(Job::Content {
                request: 7,
                hashes: vec![nir_content::digest(&payload)],
                limit: MAX_INPUT_BYTES,
            })
            .unwrap();
        loader
            .dispatch(Job::Content {
                request: 7,
                hashes: vec![nir_content::digest(b"missing")],
                limit: MAX_INPUT_BYTES,
            })
            .unwrap();
        let drained = drain_all(&mut loader);
        let mut ok = 0;
        let mut failed = 0;
        for loaded in drained {
            let Loaded::Content { data, .. } = loaded else {
                panic!("unexpected asset result");
            };
            match data {
                Ok(objects) => {
                    assert_eq!(objects, vec![payload.clone()]);
                    ok += 1;
                }
                Err(message) => {
                    assert!(message.contains("E_OBJECT_REFERENCE"));
                    failed += 1;
                }
            }
        }
        assert_eq!((ok, failed), (1, 1));
    }
}
