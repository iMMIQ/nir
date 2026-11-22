//! Full Auto-Optimizer Pipeline:
//! - Alpha Auto-Trim for sprites (crops transparent borders, records exact `SpriteTrim`)
//! - Lossy/Lossless WebP transcoding with content-hash caching
//! - Gapless MP3 CBR transcoding for WAV + zero-reencode passthrough for MP3/OGG
//! - Fast HarfBuzz font subsetting directly from `NirProgram::collect_all_chars()`
use anyhow::{bail, Context, Result};
use nir_ir::{AssetKind, AudioLoopRegion, MediaCodec, SpriteTrim};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub const OPTIMIZER_VERSION: &str = "nir-opt/v2";

#[derive(Debug, Clone, Copy)]
pub struct OptimizeConfig {
    pub enabled: bool,
    pub webp_quality: u8,
    pub lossless_images: bool,
    pub auto_trim_sprites: bool,
    pub mp3_bitrate_kbps: u16,
    pub subset_fonts: bool,
}

impl Default for OptimizeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            webp_quality: 92,
            lossless_images: false,
            auto_trim_sprites: true,
            mp3_bitrate_kbps: 160,
            subset_fonts: true,
        }
    }
}

pub struct OptimizedAsset {
    pub bytes: Vec<u8>,
    pub codec: MediaCodec,
    pub width: u32,
    pub height: u32,
    pub trim: Option<SpriteTrim>,
    pub duration_us: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub loop_region: Option<AudioLoopRegion>,
    pub saved_bytes: i64,
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Inspects an image's alpha channel and returns a trimmed RGBA sub-image + `SpriteTrim`
/// if trimming removes at least 15% of pixels (typical for Galgame standing sprites!).
fn try_auto_trim(rgba: &image::RgbaImage) -> Option<(image::RgbaImage, SpriteTrim)> {
    let (w, h) = rgba.dimensions();
    if w < 16 || h < 16 {
        return None;
    }
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any_opaque = false;

    for (x, y, px) in rgba.enumerate_pixels() {
        if px[3] > 0 {
            any_opaque = true;
            if x < min_x {
                min_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if x > max_x {
                max_x = x;
            }
            if y > max_y {
                max_y = y;
            }
        }
    }
    if !any_opaque {
        return None;
    }
    let tw = max_x - min_x + 1;
    let th = max_y - min_y + 1;
    // Only trim when it saves >= 15% area (avoids trimming opaque backgrounds).
    let orig_area = w as u64 * h as u64;
    let trim_area = tw as u64 * th as u64;
    if trim_area * 100 >= orig_area * 85 {
        return None;
    }
    let cropped = image::imageops::crop_imm(rgba, min_x, min_y, tw, th).to_image();
    Some((
        cropped,
        SpriteTrim {
            x: min_x,
            y: min_y,
            w: tw,
            h: th,
            orig_w: w,
            orig_h: h,
        },
    ))
}

pub fn optimize_image(
    raw: &[u8],
    source_codec: MediaCodec,
    cfg: &OptimizeConfig,
    cache_dir: &Path,
) -> Result<OptimizedAsset> {
    let orig_len = raw.len() as i64;
    let img = image::load_from_memory(raw).context("E_OPT_IMAGE: decode failed")?;
    let (orig_w, orig_h) = (img.width(), img.height());

    if !cfg.enabled {
        return Ok(OptimizedAsset {
            bytes: raw.to_vec(),
            codec: source_codec,
            width: orig_w,
            height: orig_h,
            trim: None,
            duration_us: 0,
            sample_rate: 0,
            channels: 0,
            loop_region: None,
            saved_bytes: 0,
        });
    }

    // Already WebP or JPEG without alpha trim opportunity -> keep as-is without re-encode loss!
    if matches!(source_codec, MediaCodec::Jpeg) {
        return Ok(OptimizedAsset {
            bytes: raw.to_vec(),
            codec: MediaCodec::Jpeg,
            width: orig_w,
            height: orig_h,
            trim: None,
            duration_us: 0,
            sample_rate: 0,
            channels: 0,
            loop_region: None,
            saved_bytes: 0,
        });
    }

    let rgba = img.to_rgba8();
    let (final_rgba, trim) = if cfg.auto_trim_sprites {
        match try_auto_trim(&rgba) {
            Some((cropped, trim)) => (cropped, Some(trim)),
            None => (rgba, None),
        }
    } else {
        (rgba, None)
    };
    let (w, h) = final_rgba.dimensions();

    // If source was already WebP and wasn't trimmed, pass through untouched.
    if source_codec == MediaCodec::Webp && trim.is_none() {
        return Ok(OptimizedAsset {
            bytes: raw.to_vec(),
            codec: MediaCodec::Webp,
            width: orig_w,
            height: orig_h,
            trim: None,
            duration_us: 0,
            sample_rate: 0,
            channels: 0,
            loop_region: None,
            saved_bytes: 0,
        });
    }

    let cache_key = sha256_hex(
        format!(
            "{OPTIMIZER_VERSION}:img:{}:{}:{}:{}",
            sha256_hex(raw),
            cfg.webp_quality,
            cfg.lossless_images,
            cfg.auto_trim_sprites
        )
        .as_bytes(),
    );
    let cache_file = cache_dir.join(format!("{cache_key}.webp"));
    let encoded = if let Ok(hit) = fs::read(&cache_file) {
        hit
    } else {
        let bytes = if cfg.lossless_images {
            let mut out = Vec::new();
            image::codecs::webp::WebPEncoder::new_lossless(&mut out)
                .encode(final_rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
                .context("E_OPT_IMAGE: lossless webp")?;
            out
        } else {
            webp::Encoder::from_rgba(final_rgba.as_raw(), w, h)
                .encode(cfg.webp_quality as f32)
                .to_vec()
        };
        let _ = fs::create_dir_all(cache_dir);
        let _ = fs::write(&cache_file, &bytes);
        bytes
    };

    // Keep original PNG only if not trimmed and WebP grew larger.
    if trim.is_none() && encoded.len() >= raw.len() {
        return Ok(OptimizedAsset {
            bytes: raw.to_vec(),
            codec: source_codec,
            width: orig_w,
            height: orig_h,
            trim: None,
            duration_us: 0,
            sample_rate: 0,
            channels: 0,
            loop_region: None,
            saved_bytes: 0,
        });
    }

    Ok(OptimizedAsset {
        saved_bytes: orig_len - encoded.len() as i64,
        bytes: encoded,
        codec: MediaCodec::Webp,
        width: w,
        height: h,
        trim,
        duration_us: 0,
        sample_rate: 0,
        channels: 0,
        loop_region: None,
    })
}

/// Probes duration/rate/channels via symphonia and optimizes WAV → gapless MP3 CBR.
/// MP3 & OGG sources pass through untouched (zero quality loss!).
pub fn optimize_audio(
    raw: &[u8],
    source_codec: MediaCodec,
    cfg: &OptimizeConfig,
    cache_dir: &Path,
) -> Result<OptimizedAsset> {
    let orig_len = raw.len() as i64;

    // Fast path: non-WAV compressed formats pass through directly after probing metadata!
    if source_codec != MediaCodec::Wav || !cfg.enabled {
        let (rate, channels, duration_us) = probe_audio_meta(raw, source_codec.ext());
        return Ok(OptimizedAsset {
            bytes: raw.to_vec(),
            codec: source_codec,
            width: 0,
            height: 0,
            trim: None,
            duration_us,
            sample_rate: rate,
            channels,
            loop_region: None,
            saved_bytes: 0,
        });
    }

    // Parse PCM16 WAV and encode gapless MP3 CBR with LAME tag!
    let Some(wav) = parse_wav_pcm16(raw) else {
        let (rate, channels, duration_us) = probe_audio_meta(raw, "wav");
        return Ok(OptimizedAsset {
            bytes: raw.to_vec(),
            codec: MediaCodec::Wav,
            width: 0,
            height: 0,
            trim: None,
            duration_us,
            sample_rate: rate,
            channels,
            loop_region: None,
            saved_bytes: 0,
        });
    };
    let frames = wav.samples.len() as u64 / wav.channels.max(1) as u64;
    let duration_us = frames.saturating_mul(1_000_000) / wav.rate.max(1) as u64;

    if !mp3_rate_supported(wav.rate) {
        return Ok(OptimizedAsset {
            bytes: raw.to_vec(),
            codec: MediaCodec::Wav,
            width: 0,
            height: 0,
            trim: None,
            duration_us,
            sample_rate: wav.rate,
            channels: wav.channels,
            loop_region: None,
            saved_bytes: 0,
        });
    }

    let cache_key = sha256_hex(
        format!(
            "{OPTIMIZER_VERSION}:mp3:{}:{}",
            sha256_hex(raw),
            cfg.mp3_bitrate_kbps
        )
        .as_bytes(),
    );
    let cache_file = cache_dir.join(format!("{cache_key}.mp3"));
    let mp3 = if let Ok(hit) = fs::read(&cache_file) {
        hit
    } else {
        match encode_gapless_mp3(&wav, cfg.mp3_bitrate_kbps) {
            Ok(bytes) => {
                let _ = fs::create_dir_all(cache_dir);
                let _ = fs::write(&cache_file, &bytes);
                bytes
            }
            Err(_) => {
                return Ok(OptimizedAsset {
                    bytes: raw.to_vec(),
                    codec: MediaCodec::Wav,
                    width: 0,
                    height: 0,
                    trim: None,
                    duration_us,
                    sample_rate: wav.rate,
                    channels: wav.channels,
                    loop_region: None,
                    saved_bytes: 0,
                });
            }
        }
    };

    Ok(OptimizedAsset {
        saved_bytes: orig_len - mp3.len() as i64,
        bytes: mp3,
        codec: MediaCodec::Mp3,
        width: 0,
        height: 0,
        trim: None,
        duration_us,
        sample_rate: wav.rate,
        channels: wav.channels,
        loop_region: None,
    })
}

/// Subsets an OpenType/TrueType font via HarfBuzz to keep only glyphs actually used by the game.
pub fn subset_font(
    raw: &[u8],
    chars: &BTreeSet<char>,
    cfg: &OptimizeConfig,
    cache_dir: &Path,
) -> Result<Vec<u8>> {
    if !cfg.enabled || !cfg.subset_fonts {
        return Ok(raw.to_vec());
    }
    let Ok(face) = ttf_parser::Face::parse(raw, 0) else {
        return Ok(raw.to_vec());
    };
    if face.is_variable() {
        return Ok(raw.to_vec());
    }

    let mut wanted = chars.clone();
    let mut decomposed = BTreeSet::new();
    for &c in &wanted {
        unicode_normalization::char::decompose_canonical(c, |d| {
            decomposed.insert(d);
        });
    }
    wanted.extend(&decomposed);
    wanted.retain(|c| face.glyph_index(*c).is_some());
    if wanted.is_empty() {
        return Ok(raw.to_vec());
    }

    let chars_str: String = wanted.iter().collect();
    let cache_key = sha256_hex(
        format!(
            "{OPTIMIZER_VERSION}:font:{}:{}",
            sha256_hex(raw),
            sha256_hex(chars_str.as_bytes())
        )
        .as_bytes(),
    );
    let cache_file = cache_dir.join(format!("{cache_key}.font"));
    if let Ok(hit) = fs::read(&cache_file) {
        return Ok(hit);
    }

    let source = hb_subset::FontFace::new_with_index(hb_subset::Blob::from_bytes(raw)?, 0)?;
    let mut input = hb_subset::SubsetInput::new().context("E_FONT_SUBSET")?;
    input.keep_everything();
    input
        .flags()
        .remove_unrecognized_tables()
        .retain_legacy_names();
    input.glyph_set().clear();
    input.unicode_set().clear();
    for &c in &wanted {
        input.unicode_set().insert(c);
    }
    let subset = input
        .subset_font(&source)
        .context("E_FONT_SUBSET: harfbuzz subset failed")?;
    let out = subset.underlying_blob().to_vec();
    let _ = fs::create_dir_all(cache_dir);
    let _ = fs::write(&cache_file, &out);
    Ok(out)
}

pub fn detect_asset_kind_and_codec(path: &Path) -> Option<(AssetKind, MediaCodec)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => (AssetKind::Image, MediaCodec::Png),
        "webp" => (AssetKind::Image, MediaCodec::Webp),
        "jpg" | "jpeg" => (AssetKind::Image, MediaCodec::Jpeg),
        "ktx2" => (AssetKind::Image, MediaCodec::Ktx2),
        "wav" => (AssetKind::Audio, MediaCodec::Wav),
        "mp3" => (AssetKind::Audio, MediaCodec::Mp3),
        "ogg" | "oga" => (AssetKind::Audio, MediaCodec::Ogg),
        "flac" => (AssetKind::Audio, MediaCodec::Flac),
        "ttf" | "ttc" => (AssetKind::Font, MediaCodec::Ttf),
        "otf" => (AssetKind::Font, MediaCodec::Otf),
        "mp4" => (AssetKind::Video, MediaCodec::Mp4),
        "webm" => (AssetKind::Video, MediaCodec::Webm),
        _ => return None,
    })
}

struct WavPcm {
    rate: u32,
    channels: u16,
    samples: Vec<i16>,
}

fn parse_wav_pcm16(b: &[u8]) -> Option<WavPcm> {
    if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= b.len() {
        let n = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().ok()?) as usize;
        let start = pos + 8;
        let end = start.checked_add(n)?;
        if end > b.len() {
            return None;
        }
        match &b[pos..pos + 4] {
            b"fmt " if n >= 16 => {
                let fmt = u16::from_le_bytes(b[start..start + 2].try_into().ok()?);
                channels = u16::from_le_bytes(b[start + 2..start + 4].try_into().ok()?);
                rate = u32::from_le_bytes(b[start + 4..start + 8].try_into().ok()?);
                let bits = u16::from_le_bytes(b[start + 14..start + 16].try_into().ok()?);
                if fmt != 1 || bits != 16 || !(1..=2).contains(&channels) {
                    return None;
                }
            }
            b"data" => data = Some(&b[start..end]),
            _ => {}
        }
        pos = end + (n % 2);
    }
    let data = data?;
    if rate == 0 {
        return None;
    }
    let mut samples: Vec<i16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| i16::from_le_bytes(*p))
        .collect();
    samples.truncate(samples.len() - samples.len() % channels as usize);
    Some(WavPcm {
        rate,
        channels,
        samples,
    })
}

fn mp3_rate_supported(rate: u32) -> bool {
    matches!(
        rate,
        8000 | 11025 | 12000 | 16000 | 22050 | 24000 | 32000 | 44100 | 48000
    )
}

fn encode_gapless_mp3(wav: &WavPcm, kbps: u16) -> Result<Vec<u8>> {
    use mp3lame_encoder::{Bitrate, FlushGap, InterleavedPcm, MonoPcm};
    let bitrate = match kbps {
        64 => Bitrate::Kbps64,
        96 => Bitrate::Kbps96,
        128 => Bitrate::Kbps128,
        160 => Bitrate::Kbps160,
        192 => Bitrate::Kbps192,
        256 => Bitrate::Kbps256,
        320 => Bitrate::Kbps320,
        _ => Bitrate::Kbps160,
    };
    let mut builder = mp3lame_encoder::Builder::new().context("LAME init")?;
    builder = builder
        .with_num_channels(wav.channels as u8)
        .and_then(|b| b.with_sample_rate(wav.rate))
        .and_then(|b| b.with_output_sample_rate(std::num::NonZeroU32::new(wav.rate)))
        .and_then(|b| b.with_brate(bitrate))
        .and_then(|b| b.with_to_write_vbr_tag(true))
        .map_err(|e| anyhow::anyhow!("LAME setup: {e:?}"))?;
    let mut enc = builder
        .build()
        .map_err(|e| anyhow::anyhow!("LAME build: {e:?}"))?;
    let mut out = Vec::new();
    let stride = wav.channels as usize;
    for block in wav.samples.chunks(1152 * 100 * stride) {
        out.reserve(mp3lame_encoder::max_required_buffer_size(block.len()));
        let res = if wav.channels == 2 {
            enc.encode_to_vec(InterleavedPcm(block), &mut out)
        } else {
            enc.encode_to_vec(MonoPcm(block), &mut out)
        };
        res.map_err(|e| anyhow::anyhow!("LAME encode: {e:?}"))?;
    }
    out.reserve(mp3lame_encoder::max_required_buffer_size(0));
    enc.flush_to_vec::<FlushGap>(&mut out)
        .map_err(|e| anyhow::anyhow!("LAME flush: {e:?}"))?;

    let boundary = enc.id3v2_tag_size();
    let tag_size = enc.lame_tag_size();
    let mut tag = Vec::new();
    if tag_size > 0 {
        tag.reserve(tag_size);
        enc.lame_tag_encode_to_vec(&mut tag)
            .ok_or_else(|| anyhow::anyhow!("LAME tag encode buffer too small"))?;
    }
    let mut file = Vec::with_capacity(out.len());
    file.extend_from_slice(&out[..boundary]);
    file.extend_from_slice(&tag);
    file.extend_from_slice(&out[boundary + tag_size..]);
    if nir_ir::lame::parse(&file).is_none() {
        bail!("encoded MP3 lacks gapless tag");
    }
    Ok(file)
}

fn probe_audio_meta(bytes: &[u8], ext: &str) -> (u32, u16, u64) {
    use symphonia::core::{formats::FormatOptions, io::MediaSourceStream, probe::Hint};
    let mut hint = Hint::new();
    hint.with_extension(ext);
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes.to_vec())), Default::default());
    let Ok(probed) = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions {
            enable_gapless: true,
            ..Default::default()
        },
        &Default::default(),
    ) else {
        return (44100, 2, 0);
    };
    let Some(track) = probed.format.default_track() else {
        return (44100, 2, 0);
    };
    let rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);
    let duration_us = track
        .codec_params
        .n_frames
        .map(|f| f.saturating_mul(1_000_000) / rate.max(1) as u64)
        .unwrap_or(0);
    (rate, channels, duration_us)
}
