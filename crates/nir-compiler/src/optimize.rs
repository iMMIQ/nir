//! Package-time media optimization.
//!
//! Authoring sources stay PNG/WAV (the formats `load_project` admits); this
//! pass re-encodes the bytes that actually ship — lossy WebP for images and
//! MP3 for audio by default — while keeping the descriptor contract intact:
//! `Asset.object`/`bytes` are rewritten from the converted bytes, dimensions
//! and authored `duration_us`/`decoded_bytes` are preserved exactly, and every
//! automatic MP3 conversion fails explicitly if its contract cannot be met.
//!
//! Looping and non-looping audio share the same gapless MP3 path. Unsupported
//! rates, invalid bitrate/rate pairs and missing gapless metadata are errors;
//! even short audio that grows after encoding stays MP3. Explicit WAV settings
//! and `optimize = "lossless" | "none"` remain author-controlled opt-outs.
//! Image conversions that do not shrink the object keep the original bytes.
//!
//! MP3 files always carry a LAME gapless tag; the native loader trims the
//! recorded delay/padding so decoded sample counts match the authored WAV and
//! the manifest's `decoded_bytes` cap stays exact. Browsers trim the same tag
//! inside `decodeAudioData`. Converted outputs are cached under
//! `.nir/cache/optimize` keyed by source digest plus parameters and tool
//! identity, so repeated dev-preview builds only encode what changed.

use crate::LoadedProject;
use anyhow::{bail, Context, Result};
use nir_format::AssetKind;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

/// Bumped whenever encoder dependencies or parameter semantics change, so
/// cached conversions from an older tool never leak into a new release.
/// The lossless WebP path goes through the `image` crate, so it is named too.
pub const OPTIMIZE_TOOL: &str =
    "nir-media-optimize/3:webp-0.3.1+libwebp,image-0.25.10,mp3lame-encoder-0.2.5+lame-3.100";

/// Container choice for packaged images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ImageFormat {
    Png,
    Webp,
    WebpLossless,
}

/// Container choice for packaged audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AudioFormat {
    Wav,
    Mp3,
}

/// Global packaging optimization settings (CLI-provided).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct OptimizeOptions {
    pub image_format: ImageFormat,
    /// Lossy WebP quality, 1-100; only meaningful for [`ImageFormat::Webp`].
    pub image_quality: u8,
    pub audio_format: AudioFormat,
    /// MP3 CBR bitrate in kbps; only meaningful for [`AudioFormat::Mp3`].
    pub audio_bitrate_kbps: u16,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            image_format: ImageFormat::Webp,
            image_quality: 92,
            audio_format: AudioFormat::Mp3,
            audio_bitrate_kbps: 160,
        }
    }
}

impl OptimizeOptions {
    /// Disables every conversion; packages the original PNG/WAV bytes.
    pub fn none() -> Self {
        Self {
            image_format: ImageFormat::Png,
            image_quality: 92,
            audio_format: AudioFormat::Wav,
            audio_bitrate_kbps: 160,
        }
    }
}

/// Per-asset override from the asset catalog (`optimize = "..."` field).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum AssetPolicy {
    /// Follow the global CLI settings.
    #[default]
    Auto,
    /// Images: lossless WebP. Audio: keep WAV (lossless source).
    Lossless,
    /// Package the original bytes untouched.
    None,
}

/// Actual container stored for one asset after the pass ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoredMedia {
    Png,
    Webp,
    Wav,
    Mp3,
}
impl StoredMedia {
    pub(crate) fn ext(self) -> &'static str {
        match self {
            StoredMedia::Png => "png",
            StoredMedia::Webp => "webp",
            StoredMedia::Wav => "wav",
            StoredMedia::Mp3 => "mp3",
        }
    }
    pub(crate) fn mime(self) -> &'static str {
        match self {
            StoredMedia::Png => "image/png",
            StoredMedia::Webp => "image/webp",
            StoredMedia::Wav => "audio/wav",
            StoredMedia::Mp3 => "audio/mpeg",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OptimizeReport {
    pub tool: String,
    pub images: ClassReport,
    pub audio: ClassReport,
    pub assets: Vec<AssetOutcome>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ClassReport {
    pub converted: usize,
    pub kept: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetOutcome {
    pub id: String,
    pub kind: &'static str,
    /// `converted` or `kept`.
    pub action: &'static str,
    pub from: &'static str,
    pub to: &'static str,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub reason: Option<String>,
    pub cache_hit: bool,
}

/// Sample rates the MP3 container can carry without resampling.
const MP3_RATES: [u32; 9] = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];

fn mp3lame_bitrate(kbps: u16) -> Option<mp3lame_encoder::Bitrate> {
    use mp3lame_encoder::Bitrate::*;
    Some(match kbps {
        8 => Kbps8,
        16 => Kbps16,
        24 => Kbps24,
        32 => Kbps32,
        40 => Kbps40,
        48 => Kbps48,
        64 => Kbps64,
        80 => Kbps80,
        96 => Kbps96,
        112 => Kbps112,
        128 => Kbps128,
        160 => Kbps160,
        192 => Kbps192,
        224 => Kbps224,
        256 => Kbps256,
        320 => Kbps320,
        _ => return None,
    })
}

/// Every CBR bitrate LAME accepts, for CLI validation messages.
pub const MP3_BITRATES: [u16; 16] = [
    8, 16, 24, 32, 40, 48, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
];

/// MPEG CBR only pairs certain bitrate ranges with certain sample rates:
/// MPEG-1 (32-48 kHz) starts at 32 kbps, MPEG-2/2.5 (8-24 kHz) tops out at
/// 160 kbps. Outside these pairs LAME silently resamples or writes no gapless
/// tag, so they never reach the encoder.
fn mp3_cbr_valid(rate: u32, kbps: u16) -> bool {
    match rate {
        32000 | 44100 | 48000 => (32..=320).contains(&kbps),
        8000 | 11025 | 12000 | 16000 | 22050 | 24000 => (8..=160).contains(&kbps),
        _ => false,
    }
}

/// LAME writes the gapless tag only when the first frame can hold it
/// (`VbrTag.c` `InitVbrTag`): the CBR frame must be at least
/// `sideinfo_len + 156` bytes (VBRHEADERSIZE 120 + 36 bytes of LAME
/// extension) and at most 2880, or the tag is silently dropped. This mirrors
/// that admission with the same truncating integer math, so the guard
/// rejects exactly the pairs whose tag would not exist.
fn mp3_cbr_tag_fits(rate: u32, kbps: u16, channels: u16) -> bool {
    let mpeg1 = matches!(rate, 32000 | 44100 | 48000);
    let unit = if mpeg1 { 144_000 } else { 72_000 };
    let frame = unit * kbps as u32 / rate;
    let sideinfo = match (mpeg1, channels) {
        (true, 2) => 32,
        (true, _) => 17,
        (false, 2) => 17,
        (false, _) => 9,
    };
    (sideinfo + 156..=2880).contains(&frame)
}

/// Parsed PCM WAV payload ready for encoding.
struct WavPcm {
    rate: u32,
    channels: u16,
    samples: Vec<i16>,
}

fn parse_wav_pcm16(b: &[u8]) -> Result<WavPcm> {
    if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        bail!("E_OPTIMIZE_AUDIO: expected PCM WAV");
    }
    let mut pos = 12;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= b.len() {
        let n = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        let end = start
            .checked_add(n)
            .context("E_OPTIMIZE_AUDIO: chunk size")?;
        if end > b.len() {
            bail!("E_OPTIMIZE_AUDIO: truncated chunk");
        }
        match &b[pos..pos + 4] {
            b"fmt " => {
                if n < 16 {
                    bail!("E_OPTIMIZE_AUDIO: fmt chunk too short");
                }
                let format = u16::from_le_bytes(b[start..start + 2].try_into().unwrap());
                channels = u16::from_le_bytes(b[start + 2..start + 4].try_into().unwrap());
                rate = u32::from_le_bytes(b[start + 4..start + 8].try_into().unwrap());
                let bits = u16::from_le_bytes(b[start + 14..start + 16].try_into().unwrap());
                if format != 1 || bits != 16 || !(1..=2).contains(&channels) {
                    bail!("E_OPTIMIZE_AUDIO: expected mono/stereo 16-bit PCM");
                }
            }
            b"data" => data = Some(&b[start..end]),
            _ => {}
        }
        pos = end + (n % 2);
    }
    let data = data.context("E_OPTIMIZE_AUDIO: missing data chunk")?;
    if rate == 0 {
        bail!("E_OPTIMIZE_AUDIO: missing fmt chunk");
    }
    let mut samples: Vec<i16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| i16::from_le_bytes([p[0], p[1]]))
        .collect();
    // A truncated data chunk can leave a lone half-sample at the end of a
    // stereo stream; drop it so interleaved channel pairs stay whole.
    samples.truncate(samples.len() - samples.len() % channels as usize);
    Ok(WavPcm {
        rate,
        channels,
        samples,
    })
}

fn encode_webp_lossy(source: &[u8], quality: u8) -> Result<Vec<u8>> {
    let img = image::load_from_memory(source).context("E_OPTIMIZE_IMAGE: decode")?;
    let memory = if img.color() == image::ColorType::Rgb8 {
        let rgb = img.to_rgb8();
        webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height()).encode(quality as f32)
    } else {
        let rgba = img.to_rgba8();
        webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height()).encode(quality as f32)
    };
    Ok(memory.to_vec())
}

fn encode_webp_lossless(source: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(source).context("E_OPTIMIZE_IMAGE: decode")?;
    let mut out = Vec::new();
    let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
    let (bytes, color) = if img.color() == image::ColorType::Rgb8 {
        let rgb = img.to_rgb8();
        (rgb.into_raw(), image::ExtendedColorType::Rgb8)
    } else {
        let rgba = img.to_rgba8();
        (rgba.into_raw(), image::ExtendedColorType::Rgba8)
    };
    encoder
        .encode(&bytes, img.width(), img.height(), color)
        .context("E_OPTIMIZE_IMAGE: lossless webp encode")?;
    Ok(out)
}

fn encode_mp3(wav: &WavPcm, bitrate: mp3lame_encoder::Bitrate) -> Result<Vec<u8>> {
    use mp3lame_encoder::{FlushGap, InterleavedPcm, MonoPcm};
    let mut builder = mp3lame_encoder::Builder::new().context("E_OPTIMIZE_AUDIO: LAME init")?;
    builder = builder
        .with_num_channels(wav.channels as u8)
        .and_then(|b| b.with_sample_rate(wav.rate))
        // Pin the output rate: LAME otherwise auto-picks a lower one from a
        // bitrate-derived lowpass (mono gets 1.5x), silently resampling e.g.
        // 32 kHz/40 kbps down to 24 kHz and breaking the sample-rate contract.
        .and_then(|b| b.with_output_sample_rate(std::num::NonZeroU32::new(wav.rate)))
        .and_then(|b| b.with_brate(bitrate))
        .and_then(|b| b.with_to_write_vbr_tag(true))
        .map_err(|e| anyhow::anyhow!("E_OPTIMIZE_AUDIO: LAME setup: {e:?}"))?;
    let mut encoder = builder
        .build()
        .map_err(|e| anyhow::anyhow!("E_OPTIMIZE_AUDIO: LAME build: {e:?}"))?;
    let mut out = Vec::new();
    let stride = wav.channels as usize;
    for block in wav.samples.chunks(1152 * 100 * stride) {
        // encode_to_vec writes into spare capacity only; LAME crashes on an
        // undersized output buffer, so reserve the documented maximum first.
        out.reserve(mp3lame_encoder::max_required_buffer_size(block.len()));
        let encoded = if wav.channels == 2 {
            encoder.encode_to_vec(InterleavedPcm(block), &mut out)
        } else {
            encoder.encode_to_vec(MonoPcm(block), &mut out)
        };
        encoded.map_err(|e| anyhow::anyhow!("E_OPTIMIZE_AUDIO: encode: {e:?}"))?;
    }
    out.reserve(mp3lame_encoder::max_required_buffer_size(0));
    // FlushGap pads the final frame with silence and records the padding in
    // the gapless tag; flush_nogap is for stream concatenation and drops the
    // tail samples, which would break the sample-exact decode contract.
    encoder
        .flush_to_vec::<FlushGap>(&mut out)
        .map_err(|e| anyhow::anyhow!("E_OPTIMIZE_AUDIO: flush: {e:?}"))?;
    // With the VBR tag enabled the encoder emits a placeholder silence frame
    // where the tag belongs; replace it in place (an inserted tag would leave
    // the placeholder behind and shift every decode by one frame).
    let boundary = encoder.id3v2_tag_size();
    let mut tag = Vec::new();
    let tag_size = encoder.lame_tag_size();
    if tag_size > 0 {
        tag.reserve(tag_size);
        encoder
            .lame_tag_encode_to_vec(&mut tag)
            .context("E_OPTIMIZE_AUDIO: gapless tag")?;
    }
    let mut file = Vec::with_capacity(out.len());
    file.extend_from_slice(&out[..boundary]);
    file.extend_from_slice(&tag);
    file.extend_from_slice(&out[boundary + tag_size..]);
    Ok(file)
}

/// Decode one frame at a time to verify the gapless contract without retaining
/// another full PCM copy. This runs for fresh encodes and cache hits alike.
fn validate_mp3(bytes: &[u8], wav: &WavPcm) -> Result<()> {
    use symphonia::core::{
        errors::Error, formats::FormatOptions, io::MediaSourceStream, probe::Hint,
    };
    anyhow::ensure!(
        nir_format::lame::parse(bytes).is_some_and(|g| g.rate == wav.rate),
        "E_OPTIMIZE_AUDIO: encoder output lacks a usable gapless tag at {} Hz",
        wav.rate
    );
    let source = MediaSourceStream::new(
        Box::new(std::io::Cursor::new(bytes.to_vec())),
        Default::default(),
    );
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let mut format = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &Default::default(),
        )
        .context("E_OPTIMIZE_AUDIO: verification probe")?
        .format;
    let track = format
        .default_track()
        .context("E_OPTIMIZE_AUDIO: missing audio track")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())
        .context("E_OPTIMIZE_AUDIO: verification decoder")?;
    let expected = wav.samples.len() / wav.channels as usize;
    let mut frames = 0;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e).context("E_OPTIMIZE_AUDIO: verification packet"),
        };
        anyhow::ensure!(
            packet.track_id() == track_id,
            "E_OPTIMIZE_AUDIO: unexpected audio track"
        );
        let decoded = decoder
            .decode(&packet)
            .context("E_OPTIMIZE_AUDIO: verification decode")?;
        anyhow::ensure!(
            decoded.spec().rate == wav.rate
                && decoded.spec().channels.count() == wav.channels as usize,
            "E_OPTIMIZE_AUDIO: decoded rate or channels changed"
        );
        frames += decoded.frames();
        anyhow::ensure!(
            frames <= expected,
            "E_OPTIMIZE_AUDIO: decoded audio exceeds authored sample count"
        );
    }
    anyhow::ensure!(
        frames == expected,
        "E_OPTIMIZE_AUDIO: decoded {frames} frames, expected {expected}"
    );
    Ok(())
}

/// Cache entry metadata; the payload sits in the sibling `.<ext>` file.
#[derive(serde::Serialize, serde::Deserialize)]
struct CacheMeta {
    source: String,
    params: String,
    output: String,
}

struct Cache<'a> {
    root: &'a Path,
}
impl Cache<'_> {
    fn key(&self, source: &[u8], params: &str) -> String {
        nir_content::digest(
            &serde_json::to_vec(&(OPTIMIZE_TOOL, nir_content::digest(source), params)).unwrap(),
        )
    }
    fn load(&self, key: &str, ext: &str) -> Option<Vec<u8>> {
        let meta: CacheMeta =
            serde_json::from_str(&fs::read_to_string(self.root.join(format!("{key}.json"))).ok()?)
                .ok()?;
        let bytes = fs::read(self.root.join(format!("{key}.{ext}"))).ok()?;
        if nir_content::digest(&bytes) != meta.output {
            return None;
        }
        Some(bytes)
    }
    fn store(
        &self,
        key: &str,
        ext: &str,
        bytes: &[u8],
        source: &str,
        params: &str,
    ) -> Result<PathBuf> {
        fs::create_dir_all(self.root)?;
        let path = self.root.join(format!("{key}.{ext}"));
        let temporary = self.root.join(format!("{key}.{ext}.next"));
        fs::write(&temporary, bytes)?;
        fs::rename(&temporary, &path)?;
        let meta = CacheMeta {
            source: source.into(),
            params: params.into(),
            output: nir_content::digest(bytes),
        };
        let meta_path = self.root.join(format!("{key}.json"));
        let temporary = self.root.join(format!("{key}.json.next"));
        fs::write(&temporary, serde_json::to_vec(&meta)?)?;
        fs::rename(&temporary, &meta_path)?;
        Ok(path)
    }
}

fn is_webp(b: &[u8]) -> bool {
    b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP"
}

/// Re-encodes the media of every packaged asset according to `options`.
///
/// Mutates `p.media` and the `p.program.assets` descriptors of converted
/// assets in place; returns the per-asset storage formats the release writer
/// must use, plus the report embedded in `BuildReport`.
pub(crate) fn optimize_media(
    p: &mut LoadedProject,
    roots: &BTreeSet<String>,
    options: &OptimizeOptions,
    cache_root: &Path,
) -> Result<(OptimizeReport, BTreeMap<String, StoredMedia>)> {
    let cache = Cache { root: cache_root };
    let mut report = OptimizeReport {
        tool: OPTIMIZE_TOOL.into(),
        ..Default::default()
    };
    let mut stored = BTreeMap::new();
    for id in roots {
        let Some(bytes) = p.media.get(id) else {
            bail!("E_OPTIMIZE: packaged asset {id} has no media bytes");
        };
        let asset = &p.program.assets[id];
        let policy = p
            .asset_optimize
            .get(id)
            .copied()
            .unwrap_or(AssetPolicy::Auto);
        let mut outcome = AssetOutcome {
            id: id.clone(),
            kind: match asset.kind {
                AssetKind::Image => "image",
                AssetKind::Audio => "audio",
                AssetKind::Font => continue, // fonts have their own pipeline
            },
            action: "kept",
            from: "",
            to: "",
            bytes_before: bytes.len() as u64,
            bytes_after: bytes.len() as u64,
            reason: None,
            cache_hit: false,
        };
        match asset.kind {
            AssetKind::Image => {
                let source_is_webp = is_webp(bytes);
                let lossless = options.image_format == ImageFormat::WebpLossless
                    || policy == AssetPolicy::Lossless;
                let convert = options.image_format != ImageFormat::Png
                    && policy != AssetPolicy::None
                    && !source_is_webp;
                if !convert {
                    let media = if source_is_webp {
                        StoredMedia::Webp
                    } else {
                        StoredMedia::Png
                    };
                    outcome.from = media.ext();
                    outcome.to = media.ext();
                    outcome.reason = Some(if source_is_webp {
                        "already webp".into()
                    } else if policy == AssetPolicy::None {
                        "asset opt-out".into()
                    } else {
                        "image conversion disabled".into()
                    });
                    stored.insert(id.clone(), media);
                    report.images.kept += 1;
                    report.images.bytes_before += bytes.len() as u64;
                    report.images.bytes_after += bytes.len() as u64;
                    report.assets.push(outcome);
                    continue;
                }
                let params = format!(
                    "image:{}:lossless={lossless}:quality={}",
                    options.image_format == ImageFormat::WebpLossless
                        || policy == AssetPolicy::Lossless,
                    if lossless { 0 } else { options.image_quality }
                );
                let key = cache.key(bytes, &params);
                let (converted, cache_hit) = if let Some(hit) = cache.load(&key, "webp") {
                    (hit, true)
                } else {
                    let fresh = if lossless {
                        encode_webp_lossless(bytes)?
                    } else {
                        encode_webp_lossy(bytes, options.image_quality)?
                    };
                    // The descriptor's dimensions are cross-checked by every
                    // player against the decoded object; re-verify on encode.
                    let decoded = image::load_from_memory(&fresh)
                        .context("E_OPTIMIZE_IMAGE: webp verification decode")?;
                    anyhow::ensure!(
                        decoded.width() == asset.width && decoded.height() == asset.height,
                        "E_OPTIMIZE_IMAGE: webp dimensions changed for {id}"
                    );
                    cache.store(&key, "webp", &fresh, &nir_content::digest(bytes), &params)?;
                    (fresh, false)
                };
                if converted.len() >= bytes.len() {
                    outcome.from = "png";
                    outcome.to = "png";
                    outcome.reason = Some("conversion did not shrink the object".into());
                    outcome.cache_hit = cache_hit;
                    stored.insert(id.clone(), StoredMedia::Png);
                    report.images.kept += 1;
                    report.images.bytes_before += bytes.len() as u64;
                    report.images.bytes_after += bytes.len() as u64;
                    report.assets.push(outcome);
                    continue;
                }
                outcome.action = "converted";
                outcome.from = "png";
                outcome.to = "webp";
                outcome.bytes_after = converted.len() as u64;
                outcome.cache_hit = cache_hit;
                report.images.converted += 1;
                report.images.bytes_before += bytes.len() as u64;
                report.images.bytes_after += converted.len() as u64;
                let asset = p.program.assets.get_mut(id).unwrap();
                asset.object = nir_content::digest(&converted);
                asset.bytes = converted.len() as u64;
                p.media.insert(id.clone(), converted);
                stored.insert(id.clone(), StoredMedia::Webp);
            }
            AssetKind::Audio => {
                let reason_kept = |why: &str| -> String { why.to_owned() };
                let convert =
                    options.audio_format == AudioFormat::Mp3 && policy != AssetPolicy::None;
                let guarded = if !convert {
                    Some(if policy == AssetPolicy::None {
                        "asset opt-out"
                    } else {
                        "audio conversion disabled"
                    })
                } else if policy == AssetPolicy::Lossless {
                    Some("lossless policy keeps WAV")
                } else {
                    None
                };
                if let Some(reason) = guarded {
                    outcome.from = "wav";
                    outcome.to = "wav";
                    outcome.reason = Some(reason_kept(reason));
                    stored.insert(id.clone(), StoredMedia::Wav);
                    report.audio.kept += 1;
                    report.audio.bytes_before += bytes.len() as u64;
                    report.audio.bytes_after += bytes.len() as u64;
                    report.assets.push(outcome);
                    continue;
                }
                let wav = parse_wav_pcm16(bytes)
                    .with_context(|| format!("E_OPTIMIZE_AUDIO: asset {id}"))?;
                anyhow::ensure!(
                    MP3_RATES.contains(&wav.rate),
                    "E_OPTIMIZE_AUDIO: asset {id}: {} Hz is not representable in MP3",
                    wav.rate
                );
                anyhow::ensure!(
                    mp3_cbr_valid(wav.rate, options.audio_bitrate_kbps),
                    "E_OPTIMIZE_AUDIO: asset {id}: {} kbps CBR is not valid at {} Hz in MP3",
                    options.audio_bitrate_kbps,
                    wav.rate
                );
                anyhow::ensure!(
                    mp3_cbr_tag_fits(wav.rate, options.audio_bitrate_kbps, wav.channels),
                    "E_OPTIMIZE_AUDIO: asset {id}: {} kbps at {} Hz with {} channels leaves no room for a gapless tag",
                    options.audio_bitrate_kbps, wav.rate, wav.channels
                );
                let bitrate = mp3lame_bitrate(options.audio_bitrate_kbps)
                    .context("E_OPTIMIZE_AUDIO: bitrate")?;
                let params = format!("audio:mp3:cbr:{}:{}", options.audio_bitrate_kbps, wav.rate);
                let key = cache.key(bytes, &params);
                let (converted, cache_hit) = if let Some(hit) = cache.load(&key, "mp3") {
                    validate_mp3(&hit, &wav)
                        .with_context(|| format!("E_OPTIMIZE_AUDIO: cached asset {id}"))?;
                    (hit, true)
                } else {
                    let fresh = encode_mp3(&wav, bitrate)
                        .with_context(|| format!("E_OPTIMIZE_AUDIO: asset {id}"))?;
                    // Validate before publishing a cache entry. A tagged file
                    // alone is insufficient: decoding must also return every
                    // authored sample at the original rate and channel count.
                    validate_mp3(&fresh, &wav)
                        .with_context(|| format!("E_OPTIMIZE_AUDIO: asset {id}"))?;
                    cache.store(&key, "mp3", &fresh, &nir_content::digest(bytes), &params)?;
                    (fresh, false)
                };
                outcome.action = "converted";
                outcome.from = "wav";
                outcome.to = "mp3";
                outcome.bytes_after = converted.len() as u64;
                outcome.cache_hit = cache_hit;
                report.audio.converted += 1;
                report.audio.bytes_before += bytes.len() as u64;
                report.audio.bytes_after += converted.len() as u64;
                // duration_us and decoded_bytes stay authored: players trim the
                // LAME tag, so decoded sample counts match the source WAV.
                let asset = p.program.assets.get_mut(id).unwrap();
                asset.object = nir_content::digest(&converted);
                asset.bytes = converted.len() as u64;
                p.media.insert(id.clone(), converted);
                stored.insert(id.clone(), StoredMedia::Mp3);
            }
            AssetKind::Font => unreachable!("fonts continue above"),
        }
        report.assets.push(outcome);
    }
    Ok((report, stored))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rgba_png(w: u32, h: u32) -> Vec<u8> {
        let mut image = image::RgbaImage::new(w, h);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, (x * 7 % 256) as u8]);
        }
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn wav_pcm(rate: u32, channels: u16, frames: usize) -> Vec<u8> {
        let mut samples = Vec::with_capacity(frames * channels as usize * 2);
        for i in 0..frames * channels as usize {
            samples.extend_from_slice(&(i as i16 % 1000).to_le_bytes());
        }
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        b.extend_from_slice(&(channels * 2).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        b.extend_from_slice(&samples);
        b
    }

    #[test]
    fn lossy_webp_preserves_dimensions_and_alpha_plane() {
        let png = rgba_png(257, 131);
        let webp = encode_webp_lossy(&png, 92).unwrap();
        assert!(is_webp(&webp));
        assert!(!webp.is_empty());
        let decoded = image::load_from_memory(&webp).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (257, 131));
        // libwebp keeps the alpha plane lossless (ALPH chunk): the alpha row
        // gradient must survive bit-exact while RGB is lossy.
        let original = image::load_from_memory(&png).unwrap().to_rgba8();
        let converted = decoded.to_rgba8();
        for (a, b) in original.pixels().zip(converted.pixels()) {
            assert_eq!(a.0[3], b.0[3], "alpha plane must stay lossless");
        }
    }

    #[test]
    fn lossless_webp_roundtrips_pixels_exactly() {
        let png = rgba_png(64, 64);
        let webp = encode_webp_lossless(&png).unwrap();
        let original = image::load_from_memory(&png).unwrap().to_rgba8();
        let decoded = image::load_from_memory(&webp).unwrap().to_rgba8();
        assert_eq!(original.as_raw(), decoded.as_raw());
    }

    #[test]
    fn mp3_carries_parseable_gapless_tag_and_shrinks_speech() {
        // One second of 44.1kHz stereo at 160kbps CBR: 20 kB MP3 vs 176 kB WAV.
        let wav_bytes = wav_pcm(44100, 2, 44100);
        let wav = parse_wav_pcm16(&wav_bytes).unwrap();
        assert_eq!(wav.samples.len() / wav.channels as usize, 44100);
        let mp3 = encode_mp3(&wav, mp3lame_bitrate(160).unwrap()).unwrap();
        assert!(mp3.len() < wav_bytes.len() / 4);
        let gapless = nir_format::lame::parse(&mp3).expect("encoder writes gapless tag");
        assert!(gapless.delay <= 3000, "delay {}", gapless.delay);
        assert!(gapless.padding <= 3000, "padding {}", gapless.padding);
        // Untrimmed decode covers whole frames around the authored audio.
        let frames = (wav.samples.len() / wav.channels as usize) as u64;
        let untrimmed = nir_format::lame::untrimmed_samples(&gapless, frames, 1152);
        assert!(untrimmed >= frames);
        assert_eq!(untrimmed % 1152, 0);
    }

    /// Locks the bitrate/rate guard against the actual encoder: every pair
    /// the guards admit must encode to an MP3 whose gapless tag names the
    /// authored rate, and every other pair must be rejected before encoding.
    /// Both channel counts are covered because the tag's room depends on the
    /// side-info size.
    #[test]
    fn every_admitted_rate_bitrate_pair_encodes_tagged_or_is_guarded() {
        let mut converted = 0;
        let mut guarded = 0;
        for &rate in MP3_RATES.iter() {
            for channels in [1u16, 2] {
                for &kbps in MP3_BITRATES.iter() {
                    if !mp3_cbr_valid(rate, kbps) || !mp3_cbr_tag_fits(rate, kbps, channels) {
                        guarded += 1;
                        continue;
                    }
                    let wav = parse_wav_pcm16(&wav_pcm(rate, channels, 1200)).unwrap();
                    let mp3 = encode_mp3(&wav, mp3lame_bitrate(kbps).unwrap())
                        .unwrap_or_else(|e| panic!("{kbps} kbps at {rate} Hz: {e}"));
                    let gapless = nir_format::lame::parse(&mp3).unwrap_or_else(|| {
                        panic!("{kbps} kbps at {rate} Hz ch{channels} lost the gapless tag")
                    });
                    assert_eq!(
                        gapless.rate, rate,
                        "{kbps} kbps at {rate} Hz ch{channels} resampled"
                    );
                    validate_mp3(&mp3, &wav)
                        .unwrap_or_else(|e| panic!("{kbps} kbps at {rate} Hz ch{channels}: {e:#}"));
                    converted += 1;
                }
            }
        }
        assert!(
            converted > 60,
            "suspiciously few admitted pairs: {converted}"
        );
        assert!(guarded > 20, "suspiciously few guarded pairs: {guarded}");
    }

    fn audio_project(rate: u32, channels: u16, frames: usize) -> LoadedProject {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/rain-letters");
        let mut project = crate::load_project(&root).unwrap();
        // audio.bgm is actually referenced by a looped cue in this project.
        assert!(project.program.cues.values().flat_map(|c| &c.effects).any(|d| matches!(
            &d.effect, nir_format::Effect::Audio { asset, looped: true, .. } if asset == "audio.bgm"
        )));
        let bytes = wav_pcm(rate, channels, frames);
        let asset = project.program.assets.get_mut("audio.bgm").unwrap();
        asset.object = nir_content::digest(&bytes);
        asset.bytes = bytes.len() as u64;
        asset.duration_us = nir_format::Micros(frames as u64 * 1_000_000 / rate as u64);
        asset.decoded_bytes = frames as u64 * channels as u64 * 4;
        project.media.insert("audio.bgm".into(), bytes);
        project
    }

    #[test]
    fn looped_bgm_and_short_audio_ship_mp3_including_cache_hits() {
        let cache = tempfile::tempdir().unwrap();
        let roots = BTreeSet::from(["audio.bgm".into()]);
        for frames in [1, 44101] {
            for cache_hit in [false, true] {
                let mut p = audio_project(44100, 2, frames);
                let authored = p.program.assets["audio.bgm"].clone();
                let (report, stored) =
                    optimize_media(&mut p, &roots, &OptimizeOptions::default(), cache.path())
                        .unwrap();
                assert_eq!(stored["audio.bgm"], StoredMedia::Mp3);
                assert_eq!((report.audio.converted, report.audio.kept), (1, 0));
                assert_eq!(report.assets[0].cache_hit, cache_hit);
                assert_eq!(
                    p.program.assets["audio.bgm"].duration_us,
                    authored.duration_us
                );
                assert_eq!(
                    p.program.assets["audio.bgm"].decoded_bytes,
                    authored.decoded_bytes
                );
                if frames == 1 {
                    assert!(report.audio.bytes_after > report.audio.bytes_before);
                }
            }
        }
    }

    #[test]
    fn unsupported_mp3_parameters_fail_without_wav_fallback() {
        let cache = tempfile::tempdir().unwrap();
        let roots = BTreeSet::from(["audio.bgm".into()]);
        for (rate, kbps, reason) in [
            (44056, 160, "not representable"),
            (44100, 8, "not valid"),
            (44100, 32, "no room"),
        ] {
            let mut p = audio_project(rate, 2, 1200);
            let before = p.media["audio.bgm"].clone();
            let options = OptimizeOptions {
                audio_bitrate_kbps: kbps,
                ..Default::default()
            };
            let error = optimize_media(&mut p, &roots, &options, cache.path()).unwrap_err();
            let message = format!("{error:#}");
            assert!(
                message.contains("E_OPTIMIZE_AUDIO")
                    && message.contains("audio.bgm")
                    && message.contains(reason),
                "{message}"
            );
            assert_eq!(p.media["audio.bgm"], before);
        }
    }

    #[test]
    fn invalid_cached_mp3_is_rejected_and_explicit_wav_policies_are_honored() {
        let directory = tempfile::tempdir().unwrap();
        let cache = Cache {
            root: directory.path(),
        };
        let roots = BTreeSet::from(["audio.bgm".into()]);
        let mut p = audio_project(44100, 2, 1200);
        let source = &p.media["audio.bgm"];
        let params = "audio:mp3:cbr:160:44100";
        let key = cache.key(source, params);
        cache
            .store(
                &key,
                "mp3",
                b"invalid",
                &nir_content::digest(source),
                params,
            )
            .unwrap();
        let error = optimize_media(
            &mut p,
            &roots,
            &OptimizeOptions::default(),
            directory.path(),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("gapless tag"));
        for policy in [AssetPolicy::None, AssetPolicy::Lossless] {
            p.asset_optimize.insert("audio.bgm".into(), policy);
            let (report, stored) = optimize_media(
                &mut p,
                &roots,
                &OptimizeOptions::default(),
                directory.path(),
            )
            .unwrap();
            assert_eq!(stored["audio.bgm"], StoredMedia::Wav);
            assert_eq!(report.audio.kept, 1);
        }
        p.asset_optimize.clear();
        let (_, stored) =
            optimize_media(&mut p, &roots, &OptimizeOptions::none(), directory.path()).unwrap();
        assert_eq!(stored["audio.bgm"], StoredMedia::Wav);
    }

    #[test]
    fn verification_rejects_wrong_sample_counts_and_truncated_mp3() {
        for (rate, channels, kbps) in [(44100, 1, 160), (48000, 2, 160), (22050, 2, 64)] {
            let mut wav = parse_wav_pcm16(&wav_pcm(rate, channels, 1201)).unwrap();
            let mp3 = encode_mp3(&wav, mp3lame_bitrate(kbps).unwrap()).unwrap();
            validate_mp3(&mp3, &wav).unwrap();
            assert!(validate_mp3(&mp3[..mp3.len() / 2], &wav).is_err());
            wav.samples
                .extend(std::iter::repeat_n(0, channels as usize));
            assert!(validate_mp3(&mp3, &wav).is_err());
        }
    }

    #[test]
    fn truncated_stereo_tail_sample_is_dropped_not_panicked() {
        let mut wav_bytes = wav_pcm(24000, 2, 100);
        // Declare and append one stray byte: half of a final sample pair.
        wav_bytes.push(0x10);
        let parsed = parse_wav_pcm16(&wav_bytes).unwrap();
        assert_eq!(parsed.samples.len(), 200);
        let mp3 = encode_mp3(&parsed, mp3lame_bitrate(64).unwrap());
        assert!(
            mp3.is_ok(),
            "odd interleaved tail must not reach the encoder"
        );
    }

    #[test]
    fn short_fmt_chunk_is_rejected_not_read_past() {
        let mut wav_bytes = wav_pcm(24000, 1, 10);
        // Rewrite the fmt chunk header to declare only 4 bytes of payload.
        wav_bytes[16..20].copy_from_slice(&4u32.to_le_bytes());
        assert!(parse_wav_pcm16(&wav_bytes).is_err());
    }

    #[test]
    fn odd_mp3_rates_are_rejected_by_the_guard() {
        assert!(!MP3_RATES.contains(&44056));
        assert!(MP3_RATES.contains(&44100));
    }

    // One-off fixture generator for the desktop loader gapless test; writes
    // only when NOIR_FIXTURE_DIR is set (CI never sets it).
    #[test]
    fn export_loader_fixture() {
        let Ok(dir) = std::env::var("NOIR_FIXTURE_DIR") else {
            return;
        };
        let rate = 44100u32;
        let frames = rate / 2; // 0.5 s chirp, alignment-sensitive
        let samples: Vec<i16> = (0..frames)
            .map(|i| {
                let t = i as f32 / rate as f32;
                let f = 220.0 + 660.0 * t;
                (f32::sin(2.0 * std::f32::consts::PI * f * t) * 12000.0) as i16
            })
            .collect();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&((36 + samples.len() * 2) as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&((samples.len() * 2) as u32).to_le_bytes());
        for s in &samples {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        let parsed = parse_wav_pcm16(&wav).unwrap();
        let mp3 = encode_mp3(&parsed, mp3lame_bitrate(96).unwrap()).unwrap();
        let tag = nir_format::lame::parse(&mp3).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("gapless-44100-mono.wav"),
            &wav,
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("gapless-44100-mono.mp3"),
            &mp3,
        )
        .unwrap();
        println!(
            "fixture: frames={} mp3_bytes={} delay={} padding={}",
            frames,
            mp3.len(),
            tag.delay,
            tag.padding
        );
    }
}
