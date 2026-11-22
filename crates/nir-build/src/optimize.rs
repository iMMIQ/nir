//! Full Auto-Optimizer Pipeline:
//! - Fast header-only dimension probing for passthrough/dev builds
//! - Metadata-backed content-hash disk cache checked BEFORE decoding
//! - Edge-inward Alpha Auto-Trim for sprites (crops transparent borders, records exact `SpriteTrim`)
//! - Transparent-pixel RGB zeroing + RGB-only WebP encoding for opaque images
//! - Gapless MP3 CBR transcoding for WAV (16/24/32-bit/float) & FLAC + zero-reencode passthrough for MP3/OGG
//! - Fast HarfBuzz font subsetting directly from `NirProgram` characters
use anyhow::{bail, Context, Result};
use nir_ir::{AssetKind, AudioLoopRegion, MediaCodec, SpriteTrim};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::Path;

pub const OPTIMIZER_VERSION: &str = "nir-opt/v3";
const IMG_CACHE_MAGIC: &[u8; 4] = b"NIOC";
const AUDIO_CACHE_MAGIC: &[u8; 4] = b"NIAC";

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

fn codec_to_u8(c: MediaCodec) -> u8 {
    c as u8
}

fn codec_from_u8(v: u8) -> Option<MediaCodec> {
    Some(match v {
        0 => MediaCodec::Png,
        1 => MediaCodec::Webp,
        2 => MediaCodec::Jpeg,
        3 => MediaCodec::Ktx2,
        4 => MediaCodec::Wav,
        5 => MediaCodec::Mp3,
        6 => MediaCodec::Ogg,
        7 => MediaCodec::Flac,
        8 => MediaCodec::Ttf,
        9 => MediaCodec::Otf,
        10 => MediaCodec::Mp4,
        11 => MediaCodec::Webm,
        12 => MediaCodec::Raw,
        _ => return None,
    })
}

/// Fast header-only dimension probe — reads only the image header (~24 bytes),
/// avoiding full pixel decompression for dev builds, `novelc check`, and JPEG passthrough!
fn probe_image_dimensions(raw: &[u8], source_codec: MediaCodec) -> Result<(u32, u32)> {
    if source_codec == MediaCodec::Webp {
        if let Some(f) = webp::BitstreamFeatures::new(raw) {
            return Ok((f.width(), f.height()));
        }
    }
    let reader = image::ImageReader::new(Cursor::new(raw))
        .with_guessed_format()
        .context("E_OPT_IMAGE: format probe failed")?;
    reader
        .into_dimensions()
        .context("E_OPT_IMAGE: dimension probe failed")
}

/// Decodes a cached image asset + metadata (codec, width, height, optional trim)
/// without re-decoding source pixels.
fn read_image_cache(path: &Path, orig_len: i64) -> Option<OptimizedAsset> {
    let data = fs::read(path).ok()?;
    if data.len() < 14 || &data[0..4] != IMG_CACHE_MAGIC {
        return None;
    }
    let codec = codec_from_u8(data[4])?;
    let width = u32::from_le_bytes(data[5..9].try_into().ok()?);
    let height = u32::from_le_bytes(data[9..13].try_into().ok()?);
    let has_trim = data[13];
    let (trim, payload_start) = if has_trim == 1 {
        if data.len() < 14 + 24 {
            return None;
        }
        let u = |off: usize| u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
        (
            Some(SpriteTrim {
                x: u(14),
                y: u(18),
                w: u(22),
                h: u(26),
                orig_w: u(30),
                orig_h: u(34),
            }),
            14 + 24,
        )
    } else if has_trim == 0 {
        (None, 14)
    } else {
        return None;
    };
    let bytes = data[payload_start..].to_vec();
    let saved_bytes = orig_len - bytes.len() as i64;
    Some(OptimizedAsset {
        bytes,
        codec,
        width,
        height,
        trim,
        duration_us: 0,
        sample_rate: 0,
        channels: 0,
        loop_region: None,
        saved_bytes,
    })
}

fn write_image_cache(
    path: &Path,
    codec: MediaCodec,
    width: u32,
    height: u32,
    trim: Option<SpriteTrim>,
    bytes: &[u8],
) {
    let mut buf = Vec::with_capacity(14 + if trim.is_some() { 24 } else { 0 } + bytes.len());
    buf.extend_from_slice(IMG_CACHE_MAGIC);
    buf.push(codec_to_u8(codec));
    buf.extend_from_slice(&width.to_le_bytes());
    buf.extend_from_slice(&height.to_le_bytes());
    if let Some(t) = trim {
        buf.push(1);
        buf.extend_from_slice(&t.x.to_le_bytes());
        buf.extend_from_slice(&t.y.to_le_bytes());
        buf.extend_from_slice(&t.w.to_le_bytes());
        buf.extend_from_slice(&t.h.to_le_bytes());
        buf.extend_from_slice(&t.orig_w.to_le_bytes());
        buf.extend_from_slice(&t.orig_h.to_le_bytes());
    } else {
        buf.push(0);
    }
    buf.extend_from_slice(bytes);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("nioc.tmp");
    if fs::write(&tmp, &buf).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn read_audio_cache(path: &Path, orig_len: i64) -> Option<OptimizedAsset> {
    let data = fs::read(path).ok()?;
    if data.len() < 19 || &data[0..4] != AUDIO_CACHE_MAGIC {
        return None;
    }
    let codec = codec_from_u8(data[4])?;
    let duration_us = u64::from_le_bytes(data[5..13].try_into().ok()?);
    let sample_rate = u32::from_le_bytes(data[13..17].try_into().ok()?);
    let channels = u16::from_le_bytes(data[17..19].try_into().ok()?);
    let bytes = data[19..].to_vec();
    let saved_bytes = orig_len - bytes.len() as i64;
    Some(OptimizedAsset {
        bytes,
        codec,
        width: 0,
        height: 0,
        trim: None,
        duration_us,
        sample_rate,
        channels,
        loop_region: None,
        saved_bytes,
    })
}

fn write_audio_cache(
    path: &Path,
    codec: MediaCodec,
    duration_us: u64,
    sample_rate: u32,
    channels: u16,
    bytes: &[u8],
) {
    let mut buf = Vec::with_capacity(19 + bytes.len());
    buf.extend_from_slice(AUDIO_CACHE_MAGIC);
    buf.push(codec_to_u8(codec));
    buf.extend_from_slice(&duration_us.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(bytes);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("niac.tmp");
    if fs::write(&tmp, &buf).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Edge-inward alpha trim: scans outer borders inward and rejects opaque images in O(1)!
/// Returns a trimmed RGBA sub-image + `SpriteTrim` if trimming removes at least 15% of pixels.
fn try_auto_trim(rgba: &image::RgbaImage) -> Option<(image::RgbaImage, SpriteTrim)> {
    let (w, h) = rgba.dimensions();
    if w < 16 || h < 16 {
        return None;
    }
    let raw = rgba.as_raw();
    let row_stride = w as usize * 4;

    // Fast corner check: if opposite corners are both non-transparent, bounding box is full image.
    let alpha_at = |x: u32, y: u32| -> u8 { raw[y as usize * row_stride + x as usize * 4 + 3] };
    if (alpha_at(0, 0) > 0 && alpha_at(w - 1, h - 1) > 0)
        || (alpha_at(w - 1, 0) > 0 && alpha_at(0, h - 1) > 0)
    {
        return None;
    }

    // 1. Find min_y scanning top-down
    let mut min_y = None;
    'top: for y in 0..h {
        let row = &raw[y as usize * row_stride..(y as usize + 1) * row_stride];
        for px in row.as_chunks::<4>().0 {
            if px[3] > 0 {
                min_y = Some(y);
                break 'top;
            }
        }
    }
    let min_y = min_y?;

    // 2. Find max_y scanning bottom-up
    let mut max_y = min_y;
    'bottom: for y in (min_y..h).rev() {
        let row = &raw[y as usize * row_stride..(y as usize + 1) * row_stride];
        for px in row.as_chunks::<4>().0 {
            if px[3] > 0 {
                max_y = y;
                break 'bottom;
            }
        }
    }

    // 3. Find min_x scanning left-to-right within min_y..=max_y
    let mut min_x = w - 1;
    'left: for x in 0..w {
        let col_off = x as usize * 4 + 3;
        for y in min_y..=max_y {
            if raw[y as usize * row_stride + col_off] > 0 {
                min_x = x;
                break 'left;
            }
        }
    }

    // 4. Find max_x scanning right-to-left within min_y..=max_y
    let mut max_x = min_x;
    'right: for x in (min_x..w).rev() {
        let col_off = x as usize * 4 + 3;
        for y in min_y..=max_y {
            if raw[y as usize * row_stride + col_off] > 0 {
                max_x = x;
                break 'right;
            }
        }
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

fn encode_webp_rgb(rgb: &[u8], w: u32, h: u32, cfg: &OptimizeConfig) -> Vec<u8> {
    let enc = webp::Encoder::from_rgb(rgb, w, h);
    if cfg.lossless_images {
        enc.encode_lossless().to_vec()
    } else {
        enc.encode(cfg.webp_quality as f32).to_vec()
    }
}

fn encode_webp_rgba(rgba: &[u8], w: u32, h: u32, cfg: &OptimizeConfig) -> Vec<u8> {
    let enc = webp::Encoder::from_rgba(rgba, w, h);
    if cfg.lossless_images {
        enc.encode_lossless().to_vec()
    } else {
        enc.encode(cfg.webp_quality as f32).to_vec()
    }
}

pub fn optimize_image(
    raw: &[u8],
    source_codec: MediaCodec,
    cfg: &OptimizeConfig,
    cache_dir: &Path,
) -> Result<OptimizedAsset> {
    let orig_len = raw.len() as i64;

    // Fast header-only path for disabled optimizer, JPEG, or KTX2 (zero pixel decoding!).
    if !cfg.enabled || matches!(source_codec, MediaCodec::Jpeg | MediaCodec::Ktx2) {
        let (orig_w, orig_h) = if source_codec == MediaCodec::Ktx2 {
            (0, 0)
        } else {
            probe_image_dimensions(raw, source_codec)?
        };
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

    // Fast path for WebP without alpha channel: never needs trimming or re-encoding!
    if source_codec == MediaCodec::Webp {
        if let Some(feat) = webp::BitstreamFeatures::new(raw) {
            if !feat.has_alpha() || !cfg.auto_trim_sprites {
                return Ok(OptimizedAsset {
                    bytes: raw.to_vec(),
                    codec: MediaCodec::Webp,
                    width: feat.width(),
                    height: feat.height(),
                    trim: None,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                    saved_bytes: 0,
                });
            }
        }
    }

    // Check metadata-backed cache BEFORE decoding pixels!
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
    let cache_file = cache_dir.join(format!("{cache_key}.nioc"));
    if let Some(hit) = read_image_cache(&cache_file, orig_len) {
        return Ok(hit);
    }

    // Decode source image (SIMD libwebp fast path for WebP, image crate for PNG)
    let img = if source_codec == MediaCodec::Webp {
        if let Some(decoded) = webp::Decoder::new(raw).decode() {
            decoded.to_image()
        } else {
            image::load_from_memory(raw).context("E_OPT_IMAGE: decode failed")?
        }
    } else {
        image::load_from_memory(raw).context("E_OPT_IMAGE: decode failed")?
    };
    let (orig_w, orig_h) = (img.width(), img.height());

    // Opaque source (e.g. Rgb8 PNG): encode directly as RGB WebP without RGBA expansion!
    if !img.color().has_alpha() {
        let rgb = img.to_rgb8();
        let encoded = encode_webp_rgb(rgb.as_raw(), orig_w, orig_h, cfg);
        let (final_bytes, final_codec) = if encoded.len() >= raw.len() {
            (raw.to_vec(), source_codec)
        } else {
            (encoded, MediaCodec::Webp)
        };
        write_image_cache(&cache_file, final_codec, orig_w, orig_h, None, &final_bytes);
        return Ok(OptimizedAsset {
            saved_bytes: orig_len - final_bytes.len() as i64,
            bytes: final_bytes,
            codec: final_codec,
            width: orig_w,
            height: orig_h,
            trim: None,
            duration_us: 0,
            sample_rate: 0,
            channels: 0,
            loop_region: None,
        });
    }

    let rgba = img.to_rgba8();
    let (mut final_rgba, trim) = if cfg.auto_trim_sprites {
        match try_auto_trim(&rgba) {
            Some((cropped, trim)) => (cropped, Some(trim)),
            None => (rgba, None),
        }
    } else {
        (rgba, None)
    };
    let (w, h) = final_rgba.dimensions();

    // If source was already WebP and wasn't trimmed, pass through untouched (and cache result).
    if source_codec == MediaCodec::Webp && trim.is_none() {
        write_image_cache(&cache_file, MediaCodec::Webp, orig_w, orig_h, None, raw);
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

    // Single pass: check if any pixel actually uses alpha < 255, and zero RGB on fully transparent pixels!
    let mut has_transparency = false;
    for px in final_rgba.as_mut().as_chunks_mut::<4>().0 {
        if px[3] == 0 {
            has_transparency = true;
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if px[3] < 255 {
            has_transparency = true;
        }
    }

    let encoded = if has_transparency {
        encode_webp_rgba(final_rgba.as_raw(), w, h, cfg)
    } else {
        let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
        for px in final_rgba.as_raw().as_chunks::<4>().0 {
            rgb.extend_from_slice(&px[..3]);
        }
        encode_webp_rgb(&rgb, w, h, cfg)
    };

    // Keep original PNG only if not trimmed and WebP grew larger.
    let (final_bytes, final_codec, final_w, final_h, final_trim) =
        if trim.is_none() && encoded.len() >= raw.len() {
            (raw.to_vec(), source_codec, orig_w, orig_h, None)
        } else {
            (encoded, MediaCodec::Webp, w, h, trim)
        };

    write_image_cache(
        &cache_file,
        final_codec,
        final_w,
        final_h,
        final_trim,
        &final_bytes,
    );

    Ok(OptimizedAsset {
        saved_bytes: orig_len - final_bytes.len() as i64,
        bytes: final_bytes,
        codec: final_codec,
        width: final_w,
        height: final_h,
        trim: final_trim,
        duration_us: 0,
        sample_rate: 0,
        channels: 0,
        loop_region: None,
    })
}

/// Probes duration/rate/channels and optimizes WAV (16/24/32-bit/float) & FLAC → gapless MP3 CBR.
/// MP3 & OGG sources pass through untouched (zero quality loss!).
pub fn optimize_audio(
    raw: &[u8],
    source_codec: MediaCodec,
    cfg: &OptimizeConfig,
    cache_dir: &Path,
) -> Result<OptimizedAsset> {
    let orig_len = raw.len() as i64;

    // Fast path: non-WAV/FLAC compressed formats or disabled optimizer pass through directly!
    if !cfg.enabled || !matches!(source_codec, MediaCodec::Wav | MediaCodec::Flac) {
        let (rate, channels, duration_us) = if source_codec == MediaCodec::Wav {
            probe_wav_header_meta(raw).unwrap_or_else(|| probe_audio_meta(raw, "wav"))
        } else {
            probe_audio_meta(raw, source_codec.ext())
        };
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

    // Check metadata-backed audio cache BEFORE decoding WAV/FLAC PCM!
    let cache_key = sha256_hex(
        format!(
            "{OPTIMIZER_VERSION}:mp3:{}:{}",
            sha256_hex(raw),
            cfg.mp3_bitrate_kbps
        )
        .as_bytes(),
    );
    let cache_file = cache_dir.join(format!("{cache_key}.niac"));
    if let Some(hit) = read_audio_cache(&cache_file, orig_len) {
        return Ok(hit);
    }

    // Decode WAV (fast PCM16 parser, symphonia fallback for 24/32-bit/float) or FLAC via symphonia.
    let pcm = match source_codec {
        MediaCodec::Wav => {
            parse_wav_pcm16(raw).or_else(|| decode_pcm16_via_symphonia(raw, "wav"))
        }
        MediaCodec::Flac => decode_pcm16_via_symphonia(raw, "flac"),
        _ => None,
    };

    let Some(wav) = pcm else {
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
    };

    let frames = wav.samples.len() as u64 / wav.channels.max(1) as u64;
    let duration_us = frames.saturating_mul(1_000_000) / wav.rate.max(1) as u64;

    if !mp3_rate_supported(wav.rate) || !(1..=2).contains(&wav.channels) {
        write_audio_cache(
            &cache_file,
            source_codec,
            duration_us,
            wav.rate,
            wav.channels,
            raw,
        );
        return Ok(OptimizedAsset {
            bytes: raw.to_vec(),
            codec: source_codec,
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

    match encode_gapless_mp3(&wav, cfg.mp3_bitrate_kbps) {
        Ok(mp3) if mp3.len() < raw.len() => {
            write_audio_cache(
                &cache_file,
                MediaCodec::Mp3,
                duration_us,
                wav.rate,
                wav.channels,
                &mp3,
            );
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
        _ => {
            write_audio_cache(
                &cache_file,
                source_codec,
                duration_us,
                wav.rate,
                wav.channels,
                raw,
            );
            Ok(OptimizedAsset {
                bytes: raw.to_vec(),
                codec: source_codec,
                width: 0,
                height: 0,
                trim: None,
                duration_us,
                sample_rate: wav.rate,
                channels: wav.channels,
                loop_region: None,
                saved_bytes: 0,
            })
        }
    }
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

    // Check cache BEFORE parsing TTF tables or running Unicode decomposition!
    let all_chars_str: String = chars.iter().collect();
    let cache_key = sha256_hex(
        format!(
            "{OPTIMIZER_VERSION}:font:{}:{}",
            sha256_hex(raw),
            sha256_hex(all_chars_str.as_bytes())
        )
        .as_bytes(),
    );
    let cache_file = cache_dir.join(format!("{cache_key}.font"));
    if let Ok(hit) = fs::read(&cache_file) {
        return Ok(hit);
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
    let tmp = cache_file.with_extension("font.tmp");
    if fs::write(&tmp, &out).is_ok() {
        let _ = fs::rename(&tmp, &cache_file);
    }
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

/// Zero-allocation WAV header metadata probe (used by dev/passthrough builds).
fn probe_wav_header_meta(b: &[u8]) -> Option<(u32, u16, u64)> {
    if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut block_align = 0u16;
    let mut data_len = 0u64;
    while pos + 8 <= b.len() {
        let n = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().ok()?) as usize;
        let start = pos + 8;
        let end = start.checked_add(n)?;
        if end > b.len() {
            return None;
        }
        match &b[pos..pos + 4] {
            b"fmt " if n >= 16 => {
                channels = u16::from_le_bytes(b[start + 2..start + 4].try_into().ok()?);
                rate = u32::from_le_bytes(b[start + 4..start + 8].try_into().ok()?);
                block_align = u16::from_le_bytes(b[start + 12..start + 14].try_into().ok()?);
            }
            b"data" => data_len = n as u64,
            _ => {}
        }
        pos = end + (n % 2);
    }
    if rate == 0 || channels == 0 || block_align == 0 {
        return None;
    }
    let frames = data_len / block_align as u64;
    let duration_us = frames.saturating_mul(1_000_000) / rate as u64;
    Some((rate, channels, duration_us))
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

/// Decodes 24-bit/32-bit/float WAV or FLAC into interleaved PCM16 via Symphonia for MP3 encoding.
fn decode_pcm16_via_symphonia(bytes: &[u8], ext: &str) -> Option<WavPcm> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;

    let mut hint = Hint::new();
    hint.with_extension(ext);
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &Default::default(),
        )
        .ok()?;
    let mut format = probed.format;
    let track = format.default_track()?;
    let rate = track.codec_params.sample_rate?;
    let channels = track.codec_params.channels?.count() as u16;
    if !(1..=2).contains(&channels) {
        return None;
    }
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .ok()?;

    let mut samples = Vec::new();
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
        samples.extend_from_slice(buf.samples());
    }
    if samples.is_empty() {
        return None;
    }
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
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
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
