//! Full Auto-Optimizer Pipeline (`nir-opt/v4`):
//! - Source-fingerprint `(len, mtime)` pre-decode disk cache (skips `.gal` decode & `.wav` reads on warm hits!)
//! - Direct `DecodedPixels` fast path (zero intermediate PNG encode/decode round-trips!)
//! - Edge-inward Alpha Auto-Trim + content-adaptive WebP quality (`Q=85` CGs / `Q=90` small UI & sprites)
//! - Optional KTX2 BC1/BC3 hardware GPU-compressed texture encoding (`--ktx2`)
//! - Dual-mono stereo detection & automatic mono downmix
//! - Non-looped voice leading/trailing digital silence trimming
//! - Gapless LAME VBR MP3 (`-V 5` mono / `-V 3` stereo) + Pure-Rust Ogg Opus (`48k` mono / `96k` stereo)
//! - Automatic re-optimization of bloated high-bitrate (`>= 105 kbps`) mono OGG voice clips
//! - Zero-copy OGG Vorbis / Opus / WAV header metadata probing
//! - Automatic 256×144 WebP gallery thumbnail generation
//! - Fast HarfBuzz font subsetting directly from `NirProgram` characters

use anyhow::{bail, Context, Result};
use nir_ir::{AssetKind, AudioLoopRegion, MediaCodec, SpriteTrim};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const OPTIMIZER_VERSION: &str = "nir-opt/v4";
const IMG_CACHE_MAGIC: &[u8; 4] = b"NIOC";
const AUDIO_CACHE_MAGIC: &[u8; 4] = b"NIAC";

/// Audio transcoding codec target for WAV/FLAC (and oversized mono OGG voices).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodecMode {
    /// Universal 100%-compatible LAME VBR MP3 (`-V 5` mono ~65 kbps, `-V 3` stereo ~155 kbps) with gapless Xing tag.
    Mp3Vbr,
    /// Modern pure-Rust Ogg Opus (`48 kbps` mono speech, `96 kbps` stereo music) — smallest possible `.nirpak`!
    Opus,
}

#[derive(Debug, Clone, Copy)]
pub struct OptimizeConfig {
    pub enabled: bool,
    /// Base WebP quality for full-size opaque CGs/BGs (default `85`, PSNR ~41-42 dB).
    /// Smaller UI elements and transparent sprites automatically get `+5` (capped at `90`).
    pub webp_quality: u8,
    pub lossless_images: bool,
    pub auto_trim_sprites: bool,
    /// Enable 16×16 tile-masked star-graph CG/sprite differential compression.
    pub delta_images: bool,
    /// Auto-generate 256×144 WebP thumbnails for `ui.gallery` items to prevent VRAM spikes.
    pub gallery_thumbnails: bool,
    /// Encode large images as ZLIB-supercompressed KTX2 BC1/BC3 GPU hardware textures.
    pub ktx2_textures: bool,
    pub audio_codec: AudioCodecMode,
    pub mp3_bitrate_kbps: u16,
    pub opus_mono_kbps: u16,
    pub opus_stereo_kbps: u16,
    /// Trim leading/trailing digital silence (< -54 dBFS) on short mono voice/SFX clips.
    pub trim_audio_silence: bool,
    /// Re-compress bloated high-bitrate (>= 105 kbps) mono OGG voices when savings >= 25%.
    pub recompress_high_bitrate_ogg: bool,
    pub subset_fonts: bool,
}

impl Default for OptimizeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            webp_quality: 85,
            lossless_images: false,
            auto_trim_sprites: true,
            delta_images: true,
            gallery_thumbnails: true,
            ktx2_textures: false,
            audio_codec: AudioCodecMode::Mp3Vbr,
            mp3_bitrate_kbps: 160,
            opus_mono_kbps: 48,
            opus_stereo_kbps: 96,
            trim_audio_silence: true,
            recompress_high_bitrate_ogg: true,
            subset_fonts: true,
        }
    }
}

impl OptimizeConfig {
    pub fn image_sig(&self) -> String {
        format!(
            "{OPTIMIZER_VERSION}:img:{}:{}:{}:{}:{}",
            self.enabled as u8,
            self.webp_quality,
            self.lossless_images as u8,
            self.auto_trim_sprites as u8,
            self.ktx2_textures as u8,
        )
    }

    pub fn audio_sig(&self) -> String {
        let mode = match self.audio_codec {
            AudioCodecMode::Mp3Vbr => "mp3vbr",
            AudioCodecMode::Opus => "opus",
        };
        format!(
            "{OPTIMIZER_VERSION}:aud:{}:{}:{}:{}:{}:{}:{}",
            self.enabled as u8,
            mode,
            self.mp3_bitrate_kbps,
            self.opus_mono_kbps,
            self.opus_stereo_kbps,
            self.trim_audio_silence as u8,
            self.recompress_high_bitrate_ogg as u8,
        )
    }
}

/// Direct uncompressed raster pixels passed into `optimize_decoded_pixels`.
pub enum RawPixels<'a> {
    Rgb {
        width: u32,
        height: u32,
        data: &'a [u8],
    },
    Rgba {
        width: u32,
        height: u32,
        data: &'a [u8],
    },
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

/// Lightweight cache handle returned when streaming `.nirpak` directly from disk cache without holding blobs in RAM!
#[derive(Debug, Clone)]
pub struct CachedPayloadRef {
    pub path: PathBuf,
    pub payload_offset: u64,
    pub payload_len: u64,
    pub sha256: [u8; 32],
}

pub struct OptimizedMeta {
    pub codec: MediaCodec,
    pub width: u32,
    pub height: u32,
    pub trim: Option<SpriteTrim>,
    pub duration_us: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub loop_region: Option<AudioLoopRegion>,
    pub saved_bytes: i64,
    pub cache_ref: CachedPayloadRef,
}

pub fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let d = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
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
        13 => MediaCodec::Opus,
        _ => return None,
    })
}

/// Copies a cached payload directly from a `.nioc`/`.niac` file into `writer` with an 8 KiB stack buffer.
pub fn stream_cached_payload(c: &CachedPayloadRef, writer: &mut impl Write) -> std::io::Result<()> {
    let mut f = fs::File::open(&c.path)?;
    f.seek(SeekFrom::Start(c.payload_offset))?;
    let mut limited = f.take(c.payload_len);
    std::io::copy(&mut limited, writer)?;
    Ok(())
}

/// Reads the raw payload bytes of a `CachedPayloadRef`.
pub fn read_cached_payload(c: &CachedPayloadRef) -> Result<Vec<u8>> {
    let mut f = fs::File::open(&c.path)?;
    f.seek(SeekFrom::Start(c.payload_offset))?;
    let mut buf = vec![0u8; c.payload_len as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn fp_image_cache_path(
    cache_dir: &Path,
    asset_name: &str,
    fp: (u64, u64),
    cfg: &OptimizeConfig,
) -> PathBuf {
    let key = sha256_hex(
        format!("fp:{}:{}:{}:{}", cfg.image_sig(), asset_name, fp.0, fp.1).as_bytes(),
    );
    cache_dir.join(format!("{key}.nioc"))
}

pub fn fp_audio_cache_path(
    cache_dir: &Path,
    asset_name: &str,
    fp: (u64, u64),
    cfg: &OptimizeConfig,
) -> PathBuf {
    let key = sha256_hex(
        format!("fp:{}:{}:{}:{}", cfg.audio_sig(), asset_name, fp.0, fp.1).as_bytes(),
    );
    cache_dir.join(format!("{key}.niac"))
}

/// Reads ONLY the header + SHA-256 of a `.nioc` cache file without keeping payload in RAM!
pub fn read_image_cache_meta_only(path: &Path, orig_len: i64) -> Option<OptimizedMeta> {
    let mut f = fs::File::open(path).ok()?;
    let file_len = f.metadata().ok()?.len();
    if file_len < 46 {
        return None;
    }
    let mut hdr = [0u8; 70];
    let head_read = (file_len as usize).min(hdr.len());
    f.read_exact(&mut hdr[..head_read]).ok()?;
    if &hdr[0..4] != IMG_CACHE_MAGIC {
        return None;
    }
    let codec = codec_from_u8(hdr[4])?;
    let width = u32::from_le_bytes(hdr[5..9].try_into().ok()?);
    let height = u32::from_le_bytes(hdr[9..13].try_into().ok()?);
    let has_trim = hdr[13];
    let (trim, hash_start) = if has_trim == 1 {
        if head_read < 14 + 24 + 32 {
            return None;
        }
        let u = |off: usize| u32::from_le_bytes(hdr[off..off + 4].try_into().unwrap());
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
    let payload_offset = (hash_start + 32) as u64;
    if file_len < payload_offset {
        return None;
    }
    let mut sha = [0u8; 32];
    sha.copy_from_slice(&hdr[hash_start..hash_start + 32]);
    let payload_len = file_len - payload_offset;
    Some(OptimizedMeta {
        codec,
        width,
        height,
        trim,
        duration_us: 0,
        sample_rate: 0,
        channels: 0,
        loop_region: None,
        saved_bytes: orig_len - payload_len as i64,
        cache_ref: CachedPayloadRef {
            path: path.to_path_buf(),
            payload_offset,
            payload_len,
            sha256: sha,
        },
    })
}

/// Reads ONLY the header + SHA-256 of a `.niac` cache file without loading audio payload into RAM!
pub fn read_audio_cache_meta_only(path: &Path, orig_len: i64) -> Option<OptimizedMeta> {
    let mut f = fs::File::open(path).ok()?;
    let file_len = f.metadata().ok()?.len();
    if file_len < 51 {
        return None;
    }
    let mut hdr = [0u8; 51];
    f.read_exact(&mut hdr).ok()?;
    if &hdr[0..4] != AUDIO_CACHE_MAGIC {
        return None;
    }
    let codec = codec_from_u8(hdr[4])?;
    let duration_us = u64::from_le_bytes(hdr[5..13].try_into().ok()?);
    let sample_rate = u32::from_le_bytes(hdr[13..17].try_into().ok()?);
    let channels = u16::from_le_bytes(hdr[17..19].try_into().ok()?);
    let mut sha = [0u8; 32];
    sha.copy_from_slice(&hdr[19..51]);
    let payload_offset = 51u64;
    let payload_len = file_len - payload_offset;
    Some(OptimizedMeta {
        codec,
        width: 0,
        height: 0,
        trim: None,
        duration_us,
        sample_rate,
        channels,
        loop_region: None,
        saved_bytes: orig_len - payload_len as i64,
        cache_ref: CachedPayloadRef {
            path: path.to_path_buf(),
            payload_offset,
            payload_len,
            sha256: sha,
        },
    })
}

/// Fast header-only dimension probe — reads only the image header (~24 bytes).
fn probe_image_dimensions(raw: &[u8], source_codec: MediaCodec) -> Result<(u32, u32)> {
    if source_codec == MediaCodec::Webp {
        if let Some(f) = webp::BitstreamFeatures::new(raw) {
            return Ok((f.width(), f.height()));
        }
    }
    if source_codec == MediaCodec::Ktx2 && raw.len() >= 28 {
        let w = u32::from_le_bytes(raw[20..24].try_into().unwrap());
        let h = u32::from_le_bytes(raw[24..28].try_into().unwrap());
        return Ok((w, h));
    }
    let reader = image::ImageReader::new(Cursor::new(raw))
        .with_guessed_format()
        .context("E_OPT_IMAGE: format probe failed")?;
    reader
        .into_dimensions()
        .context("E_OPT_IMAGE: dimension probe failed")
}

fn read_image_cache(path: &Path, orig_len: i64) -> Option<OptimizedAsset> {
    let meta = read_image_cache_meta_only(path, orig_len)?;
    let bytes = read_cached_payload(&meta.cache_ref).ok()?;
    Some(OptimizedAsset {
        bytes,
        codec: meta.codec,
        width: meta.width,
        height: meta.height,
        trim: meta.trim,
        duration_us: 0,
        sample_rate: 0,
        channels: 0,
        loop_region: None,
        saved_bytes: meta.saved_bytes,
    })
}

pub fn write_image_cache(
    path: &Path,
    codec: MediaCodec,
    width: u32,
    height: u32,
    trim: Option<SpriteTrim>,
    bytes: &[u8],
) -> CachedPayloadRef {
    let sha = sha256_bytes(bytes);
    let hdr_len = 14 + if trim.is_some() { 24 } else { 0 } + 32;
    let mut buf = Vec::with_capacity(hdr_len + bytes.len());
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
    buf.extend_from_slice(&sha);
    buf.extend_from_slice(bytes);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension(format!("nioc.{}.tmp", std::process::id()));
    if fs::write(&tmp, &buf).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
    CachedPayloadRef {
        path: path.to_path_buf(),
        payload_offset: hdr_len as u64,
        payload_len: bytes.len() as u64,
        sha256: sha,
    }
}

fn read_audio_cache(path: &Path, orig_len: i64) -> Option<OptimizedAsset> {
    let meta = read_audio_cache_meta_only(path, orig_len)?;
    let bytes = read_cached_payload(&meta.cache_ref).ok()?;
    Some(OptimizedAsset {
        bytes,
        codec: meta.codec,
        width: 0,
        height: 0,
        trim: None,
        duration_us: meta.duration_us,
        sample_rate: meta.sample_rate,
        channels: meta.channels,
        loop_region: None,
        saved_bytes: meta.saved_bytes,
    })
}

pub fn write_audio_cache(
    path: &Path,
    codec: MediaCodec,
    duration_us: u64,
    sample_rate: u32,
    channels: u16,
    bytes: &[u8],
) -> CachedPayloadRef {
    let sha = sha256_bytes(bytes);
    let hdr_len = 51usize;
    let mut buf = Vec::with_capacity(hdr_len + bytes.len());
    buf.extend_from_slice(AUDIO_CACHE_MAGIC);
    buf.push(codec_to_u8(codec));
    buf.extend_from_slice(&duration_us.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sha);
    buf.extend_from_slice(bytes);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension(format!("niac.{}.tmp", std::process::id()));
    if fs::write(&tmp, &buf).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
    CachedPayloadRef {
        path: path.to_path_buf(),
        payload_offset: hdr_len as u64,
        payload_len: bytes.len() as u64,
        sha256: sha,
    }
}

/// Edge-inward alpha trim: scans outer borders inward and rejects opaque images in O(1).
fn try_auto_trim(rgba: &image::RgbaImage) -> Option<(image::RgbaImage, SpriteTrim)> {
    let (w, h) = rgba.dimensions();
    if w < 16 || h < 16 {
        return None;
    }
    let raw = rgba.as_raw();
    let row_stride = w as usize * 4;

    let alpha_at = |x: u32, y: u32| -> u8 { raw[y as usize * row_stride + x as usize * 4 + 3] };
    if (alpha_at(0, 0) > 0 && alpha_at(w - 1, h - 1) > 0)
        || (alpha_at(w - 1, 0) > 0 && alpha_at(0, h - 1) > 0)
    {
        return None;
    }

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

/// Content-adaptive WebP quality: large opaque CGs/BGs use `cfg.webp_quality` (default 85),
/// while crisp small UI icons/buttons and transparent sprites receive +5 (capped at 90).
#[inline]
pub fn effective_webp_quality(w: u32, h: u32, has_alpha: bool, cfg: &OptimizeConfig) -> u8 {
    if has_alpha || w.max(h) < 512 {
        cfg.webp_quality.saturating_add(5).min(90)
    } else {
        cfg.webp_quality
    }
}

fn encode_webp_rgb_q(rgb: &[u8], w: u32, h: u32, q: u8, lossless: bool) -> Vec<u8> {
    let enc = webp::Encoder::from_rgb(rgb, w, h);
    if lossless {
        enc.encode_lossless().to_vec()
    } else {
        enc.encode(q as f32).to_vec()
    }
}

fn encode_webp_rgba_q(rgba: &[u8], w: u32, h: u32, q: u8, lossless: bool) -> Vec<u8> {
    let enc = webp::Encoder::from_rgba(rgba, w, h);
    if lossless {
        enc.encode_lossless().to_vec()
    } else {
        enc.encode(q as f32).to_vec()
    }
}

/// Direct pixel optimizer bypassing intermediate PNG encoding/decoding completely!
/// Returns `(OptimizedAsset, has_alpha)` so callers can optionally reuse decoded pixels for delta clustering.
pub fn optimize_decoded_pixels(
    pixels: RawPixels<'_>,
    orig_source_len: i64,
    cfg: &OptimizeConfig,
) -> Result<(OptimizedAsset, bool)> {
    match pixels {
        RawPixels::Rgb { width, height, data } => {
            if cfg.enabled && cfg.ktx2_textures && width >= 64 && height >= 64 {
                let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
                for px in data.as_chunks::<3>().0 {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                let encoded = crate::ktx2_enc::encode_ktx2_bcn(&rgba, width, height, false);
                return Ok((
                    OptimizedAsset {
                        saved_bytes: orig_source_len - encoded.len() as i64,
                        bytes: encoded,
                        codec: MediaCodec::Ktx2,
                        width,
                        height,
                        trim: None,
                        duration_us: 0,
                        sample_rate: 0,
                        channels: 0,
                        loop_region: None,
                    },
                    false,
                ));
            }
            let q = effective_webp_quality(width, height, false, cfg);
            let encoded = encode_webp_rgb_q(data, width, height, q, cfg.lossless_images || !cfg.enabled);
            Ok((
                OptimizedAsset {
                    saved_bytes: orig_source_len - encoded.len() as i64,
                    bytes: encoded,
                    codec: MediaCodec::Webp,
                    width,
                    height,
                    trim: None,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                },
                false,
            ))
        }
        RawPixels::Rgba { width, height, data } => {
            let rgba = image::RgbaImage::from_raw(width, height, data.to_vec())
                .context("E_OPT_IMAGE: RGBA buffer size mismatch")?;
            let (mut final_rgba, trim) = if cfg.enabled && cfg.auto_trim_sprites {
                match try_auto_trim(&rgba) {
                    Some((cropped, trim)) => (cropped, Some(trim)),
                    None => (rgba, None),
                }
            } else {
                (rgba, None)
            };
            let (w, h) = final_rgba.dimensions();
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

            if cfg.enabled && cfg.ktx2_textures && w >= 64 && h >= 64 {
                let encoded =
                    crate::ktx2_enc::encode_ktx2_bcn(final_rgba.as_raw(), w, h, has_transparency);
                return Ok((
                    OptimizedAsset {
                        saved_bytes: orig_source_len - encoded.len() as i64,
                        bytes: encoded,
                        codec: MediaCodec::Ktx2,
                        width: w,
                        height: h,
                        trim,
                        duration_us: 0,
                        sample_rate: 0,
                        channels: 0,
                        loop_region: None,
                    },
                    has_transparency,
                ));
            }

            let q = effective_webp_quality(w, h, has_transparency, cfg);
            let encoded = if has_transparency {
                encode_webp_rgba_q(final_rgba.as_raw(), w, h, q, cfg.lossless_images || !cfg.enabled)
            } else {
                let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
                for px in final_rgba.as_raw().as_chunks::<4>().0 {
                    rgb.extend_from_slice(&px[..3]);
                }
                encode_webp_rgb_q(&rgb, w, h, q, cfg.lossless_images || !cfg.enabled)
            };
            Ok((
                OptimizedAsset {
                    saved_bytes: orig_source_len - encoded.len() as i64,
                    bytes: encoded,
                    codec: MediaCodec::Webp,
                    width: w,
                    height: h,
                    trim,
                    duration_us: 0,
                    sample_rate: 0,
                    channels: 0,
                    loop_region: None,
                },
                has_transparency,
            ))
        }
    }
}

pub fn optimize_image(
    raw: &[u8],
    source_codec: MediaCodec,
    cfg: &OptimizeConfig,
    cache_dir: &Path,
) -> Result<OptimizedAsset> {
    let orig_len = raw.len() as i64;

    // Fast header-only path for disabled optimizer, KTX2, or non-KTX2 JPEG passthrough.
    if !cfg.enabled || source_codec == MediaCodec::Ktx2 || (source_codec == MediaCodec::Jpeg && !cfg.ktx2_textures) {
        let (orig_w, orig_h) = probe_image_dimensions(raw, source_codec)?;
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

    // Fast path for opaque WebP when KTX2 is disabled: never needs trimming or re-encoding.
    if source_codec == MediaCodec::Webp && !cfg.ktx2_textures {
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

    // Check metadata-backed content cache BEFORE decoding pixels!
    let cache_key = sha256_hex(format!("{}:{}", cfg.image_sig(), sha256_hex(raw)).as_bytes());
    let cache_file = cache_dir.join(format!("{cache_key}.nioc"));
    if let Some(hit) = read_image_cache(&cache_file, orig_len) {
        return Ok(hit);
    }

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

    if !img.color().has_alpha() {
        let rgb = img.to_rgb8();
        let (mut opt_res, _) = optimize_decoded_pixels(
            RawPixels::Rgb {
                width: orig_w,
                height: orig_h,
                data: rgb.as_raw(),
            },
            orig_len,
            cfg,
        )?;
        if opt_res.codec == MediaCodec::Webp && opt_res.bytes.len() >= raw.len() {
            opt_res.bytes = raw.to_vec();
            opt_res.codec = source_codec;
            opt_res.saved_bytes = 0;
        }
        write_image_cache(
            &cache_file,
            opt_res.codec,
            opt_res.width,
            opt_res.height,
            None,
            &opt_res.bytes,
        );
        return Ok(opt_res);
    }

    let rgba = img.to_rgba8();
    let (mut opt_res, _) = optimize_decoded_pixels(
        RawPixels::Rgba {
            width: orig_w,
            height: orig_h,
            data: rgba.as_raw(),
        },
        orig_len,
        cfg,
    )?;

    if source_codec == MediaCodec::Webp && opt_res.trim.is_none() && !cfg.ktx2_textures {
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

    if opt_res.trim.is_none()
        && opt_res.codec == MediaCodec::Webp
        && opt_res.bytes.len() >= raw.len()
    {
        opt_res.bytes = raw.to_vec();
        opt_res.codec = source_codec;
        opt_res.width = orig_w;
        opt_res.height = orig_h;
        opt_res.saved_bytes = 0;
    }

    write_image_cache(
        &cache_file,
        opt_res.codec,
        opt_res.width,
        opt_res.height,
        opt_res.trim,
        &opt_res.bytes,
    );
    Ok(opt_res)
}

/// Generates a compact aspect-fit WebP gallery thumbnail (`<= 256×144`, `Q=78`) from an encoded image.
/// Returns `None` if decoding fails or the image is already small (`<= 320×180`).
pub fn generate_gallery_thumbnail(
    bytes: &[u8],
    codec: MediaCodec,
    cache_dir: &Path,
) -> Option<(u32, u32, Vec<u8>)> {
    if codec == MediaCodec::Ktx2 {
        return None;
    }
    let key = sha256_hex(format!("{OPTIMIZER_VERSION}:thumb256:{}", sha256_hex(bytes)).as_bytes());
    let cache_file = cache_dir.join(format!("{key}.nioc"));
    if let Some(hit) = read_image_cache(&cache_file, bytes.len() as i64) {
        return Some((hit.width, hit.height, hit.bytes));
    }
    let img = if codec == MediaCodec::Webp {
        webp::Decoder::new(bytes)
            .decode()
            .map(|d| d.to_image())
            .or_else(|| image::load_from_memory(bytes).ok())?
    } else {
        image::load_from_memory(bytes).ok()?
    };
    let (w, h) = (img.width(), img.height());
    if w <= 320 && h <= 180 {
        return None;
    }
    let scale = (256.0 / w as f32).min(144.0 / h as f32).min(1.0);
    let tw = ((w as f32 * scale).round() as u32).clamp(1, 256);
    let th = ((h as f32 * scale).round() as u32).clamp(1, 144);
    let resized = img.resize_exact(tw, th, image::imageops::FilterType::Lanczos3);
    let rgb = resized.to_rgb8();
    let webp_bytes = webp::Encoder::from_rgb(rgb.as_raw(), tw, th)
        .encode(78.0)
        .to_vec();
    write_image_cache(&cache_file, MediaCodec::Webp, tw, th, None, &webp_bytes);
    Some((tw, th, webp_bytes))
}

/// Decodes an optimized WebP/PNG/JPEG asset to straight RGBA8 (used by delta clustering).
pub fn decode_to_rgba(bytes: &[u8], codec: MediaCodec) -> Option<(u32, u32, Vec<u8>, bool)> {
    if codec == MediaCodec::Ktx2 {
        return None;
    }
    let img = if codec == MediaCodec::Webp {
        webp::Decoder::new(bytes)
            .decode()
            .map(|d| d.to_image())
            .or_else(|| image::load_from_memory(bytes).ok())?
    } else {
        image::load_from_memory(bytes).ok()?
    };
    let has_alpha = img.color().has_alpha();
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let actual_alpha = has_alpha && rgba.as_raw().as_chunks::<4>().0.iter().any(|px| px[3] < 255);
    Some((w, h, rgba.into_raw(), actual_alpha))
}

/// Probes duration/rate/channels and optimizes WAV/FLAC (plus bloated mono OGG voices!)
/// with dual-mono downmix, leading/trailing silence trim, and LAME VBR MP3 or Pure-Rust Opus!
pub fn optimize_audio(
    raw: &[u8],
    source_codec: MediaCodec,
    cfg: &OptimizeConfig,
    cache_dir: &Path,
) -> Result<OptimizedAsset> {
    let orig_len = raw.len() as i64;

    if !cfg.enabled {
        let (rate, channels, duration_us) = fast_probe_audio_meta(raw, source_codec);
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

    // Check if an OGG Vorbis file is a bloated high-bitrate mono voice clip (>= 105 kbps, >= 0.5s).
    let recompress_ogg = if source_codec == MediaCodec::Ogg && cfg.recompress_high_bitrate_ogg {
        if let Some((_sr, ch, dur_us)) = probe_ogg_vorbis_fast(raw) {
            if ch == 1 && dur_us >= 500_000 {
                let kbps = (raw.len() as f64 * 8.0) / (dur_us as f64 / 1_000_000.0) / 1000.0;
                kbps >= 105.0
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    if !matches!(source_codec, MediaCodec::Wav | MediaCodec::Flac) && !recompress_ogg {
        let (rate, channels, duration_us) = fast_probe_audio_meta(raw, source_codec);
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

    // Check metadata-backed audio cache BEFORE decoding PCM!
    let cache_key = sha256_hex(format!("{}:{}", cfg.audio_sig(), sha256_hex(raw)).as_bytes());
    let cache_file = cache_dir.join(format!("{cache_key}.niac"));
    if let Some(hit) = read_audio_cache(&cache_file, orig_len) {
        return Ok(hit);
    }

    let pcm = match source_codec {
        MediaCodec::Wav => parse_wav_pcm16(raw).or_else(|| decode_pcm16_via_symphonia(raw, "wav")),
        MediaCodec::Flac => decode_pcm16_via_symphonia(raw, "flac"),
        MediaCodec::Ogg => decode_pcm16_via_symphonia(raw, "ogg"),
        _ => None,
    };

    let Some(mut wav) = pcm else {
        let (rate, channels, duration_us) = fast_probe_audio_meta(raw, source_codec);
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

    // 1. Detect dual-mono stereo recordings (e.g., 95% of stereo voice WAVs!) and downmix losslessly to mono.
    detect_and_downmix_dual_mono(&mut wav);

    // 2. Trim leading/trailing digital silence (< -54 dBFS) on short mono voice/SFX clips (< 30s).
    if cfg.trim_audio_silence && wav.channels == 1 {
        trim_mono_silence(&mut wav);
    }

    let frames = wav.samples.len() as u64 / wav.channels.max(1) as u64;
    let duration_us = frames.saturating_mul(1_000_000) / wav.rate.max(1) as u64;

    let encoded_res: Option<(Vec<u8>, MediaCodec, u32)> = match cfg.audio_codec {
        AudioCodecMode::Opus => {
            match crate::opus_enc::encode_ogg_opus(
                &wav.samples,
                wav.rate,
                wav.channels,
                cfg.opus_mono_kbps,
                cfg.opus_stereo_kbps,
            ) {
                Ok(bytes) => Some((bytes, MediaCodec::Opus, 48_000)),
                Err(_) => None,
            }
        }
        AudioCodecMode::Mp3Vbr => {
            if mp3_rate_supported(wav.rate) && (1..=2).contains(&wav.channels) {
                encode_gapless_mp3_vbr(&wav, cfg.mp3_bitrate_kbps)
                    .ok()
                    .map(|bytes| (bytes, MediaCodec::Mp3, wav.rate))
            } else {
                None
            }
        }
    };

    // For re-compressed OGG, require >= 25% reduction to justify transcoding; for WAV/FLAC accept any win.
    let accept = |enc_len: usize| -> bool {
        if recompress_ogg {
            enc_len * 100 <= raw.len() * 75
        } else {
            enc_len < raw.len()
        }
    };

    if let Some((enc_bytes, enc_codec, out_rate)) = encoded_res {
        if accept(enc_bytes.len()) {
            write_audio_cache(
                &cache_file,
                enc_codec,
                duration_us,
                out_rate,
                wav.channels,
                &enc_bytes,
            );
            return Ok(OptimizedAsset {
                saved_bytes: orig_len - enc_bytes.len() as i64,
                bytes: enc_bytes,
                codec: enc_codec,
                width: 0,
                height: 0,
                trim: None,
                duration_us,
                sample_rate: out_rate,
                channels: wav.channels,
                loop_region: None,
            });
        }
    }

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

/// Detects dual-mono stereo (`max(|L-R|) <= 8` or side-channel RMS < -44 dBFS) and collapses to mono!
fn detect_and_downmix_dual_mono(wav: &mut WavPcm) {
    if wav.channels != 2 || wav.samples.len() < 4 {
        return;
    }
    let mut max_diff: i32 = 0;
    let mut side_sq_sum: f64 = 0.0;
    let mut pairs: u64 = 0;
    // Stride-sample up to 48,000 stereo pairs for O(1) speed even on long BGM tracks.
    let total_pairs = wav.samples.len() / 2;
    let step = (total_pairs / 48_000).max(1);
    let mut i = 0usize;
    while i < total_pairs {
        let l = wav.samples[i * 2] as i32;
        let r = wav.samples[i * 2 + 1] as i32;
        let d = (l - r).abs();
        if d > max_diff {
            max_diff = d;
        }
        let side = (l - r) as f64 * 0.5;
        side_sq_sum += side * side;
        pairs += 1;
        i += step;
    }
    let side_rms = (side_sq_sum / pairs.max(1) as f64).sqrt();
    // -44 dBFS on 16-bit full-scale (32768 * 10^(-44/20) ≈ 207); require tight <= 120 or max_diff <= 8.
    if max_diff <= 8 || side_rms < 120.0 {
        let mut mono = Vec::with_capacity(total_pairs);
        for pair in wav.samples.as_chunks::<2>().0 {
            let avg = ((pair[0] as i32 + pair[1] as i32) / 2) as i16;
            mono.push(avg);
        }
        wav.samples = mono;
        wav.channels = 1;
    }
}

/// Trims leading/trailing digital silence (< -54 dBFS ≈ amp 65) from short mono voice/SFX (< 30s),
/// preserving a 20 ms guard window on each side so plosives and breath tails are never clipped.
fn trim_mono_silence(wav: &mut WavPcm) {
    if wav.channels != 1 || wav.rate == 0 {
        return;
    }
    let total = wav.samples.len();
    let max_len = (wav.rate as usize).saturating_mul(30);
    let min_len = (wav.rate as usize) / 10; // >= 100 ms
    if total > max_len || total <= min_len {
        return;
    }
    const THRESHOLD: i16 = 65; // ~-54 dBFS
    let Some(first) = wav.samples.iter().position(|&s| s.saturating_abs() > THRESHOLD) else {
        return;
    };
    let last = wav
        .samples
        .iter()
        .rposition(|&s| s.saturating_abs() > THRESHOLD)
        .unwrap_or(first);
    let guard = (wav.rate as usize) / 50; // 20 ms
    let start = first.saturating_sub(guard);
    let end = (last + 1 + guard).min(total);
    // Only trim if removing at least 40 ms of silence
    let removed = start + (total - end);
    if removed >= (wav.rate as usize) / 25 && (end - start) >= min_len {
        wav.samples = wav.samples[start..end].to_vec();
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
    let tmp = cache_file.with_extension(format!("font.{}.tmp", std::process::id()));
    if fs::write(&tmp, &out).is_ok() {
        let _ = fs::rename(&tmp, &cache_file);
    }
    Ok(out)
}

/// Discovers a system Pan-CJK OpenType/TrueType font (preferring Noto Sans CJK SC/JP)
/// suitable for generating the shared Web fallback font (`fallback-cjk.otf`).
pub fn find_system_cjk_font() -> Option<(PathBuf, u32)> {
    let candidates = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
        "/usr/share/fonts/opentype/noto/NotoSansCJKjp-Regular.otf",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    ];
    for p in candidates {
        let path = PathBuf::from(p);
        if !path.is_file() {
            continue;
        }
        let Ok(raw) = fs::read(&path) else { continue };
        let count = ttf_parser::fonts_in_collection(&raw).unwrap_or(1);
        let mut best_idx = 0u32;
        for idx in 0..count {
            if let Ok(face) = ttf_parser::Face::parse(&raw, idx) {
                let is_sc = face.names().into_iter().any(|n| {
                    n.to_string()
                        .is_some_and(|s| s.contains("CJK SC") && !s.contains("Mono"))
                });
                if is_sc {
                    best_idx = idx;
                    break;
                }
            }
        }
        return Some((path, best_idx));
    }
    None
}

/// Builds a compact standalone Pan-CJK BMP fallback OpenType font (covering ALL 20,992 BMP CJK
/// Unified Ideographs + Kana + Fullwidth/Halfwidth + Punctuation + Symbols + Basic Latin)
/// for sharing across Web games with zero missing glyphs.
pub fn build_web_cjk_fallback_font(source_path: &Path, face_index: u32) -> Result<Vec<u8>> {
    let raw = fs::read(source_path)
        .with_context(|| format!("reading source CJK font `{}`", source_path.display()))?;
    let face = ttf_parser::Face::parse(&raw, face_index)
        .map_err(|_| anyhow::anyhow!("E_FONT: invalid face {face_index} in `{}`", source_path.display()))?;
    let source = hb_subset::FontFace::new_with_index(hb_subset::Blob::from_bytes(&raw)?, face_index)?;
    let mut input = hb_subset::SubsetInput::new().context("E_FONT_SUBSET")?;
    input.flags().remove_unrecognized_tables().retain_legacy_names();
    input.layout_feature_tag_set().clear();

    let ranges: &[(u32, u32)] = &[
        (0x0020, 0x017F), // Basic Latin, Latin-1, Latin Extended-A
        (0x2000, 0x27BF), // General Punctuation, Arrows, Math (−), Symbols (♥♪★☆), Dingbats
        (0x3000, 0x33FF), // CJK Symbols & Punctuation, Hiragana, Katakana, Bopomofo, Enclosed CJK
        (0x4E00, 0x9FFF), // Full BMP CJK Unified Ideographs (SC + TC + JP Kanji)
        (0xF900, 0xFAFF), // CJK Compatibility Ideographs
        (0xFE10, 0xFE6F), // Vertical & Small Form Variants
        (0xFF00, 0xFFEF), // Halfwidth & Fullwidth Forms
    ];
    for &(lo, hi) in ranges {
        for cp in lo..=hi {
            if let Some(ch) = char::from_u32(cp) {
                if face.glyph_index(ch).is_some() {
                    input.unicode_set().insert(ch);
                }
            }
        }
    }
    let subset = input
        .subset_font(&source)
        .context("E_FONT_SUBSET: harfbuzz Pan-CJK fallback subset failed")?;
    let out = subset.underlying_blob().to_vec();
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
        "opus" => (AssetKind::Audio, MediaCodec::Opus),
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

/// Zero-allocation WAV header metadata probe.
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

/// Zero-copy OGG Vorbis header & trailing granule probe (~1 µs, no Symphonia / no buffer clone!).
fn probe_ogg_vorbis_fast(b: &[u8]) -> Option<(u32, u16, u64)> {
    let head_scan = b.len().min(512);
    let idx = b[..head_scan].windows(7).position(|w| w == b"\x01vorbis")?;
    if idx + 28 > b.len() {
        return None;
    }
    let channels = b[idx + 11] as u16;
    let rate = u32::from_le_bytes(b[idx + 12..idx + 16].try_into().ok()?);
    if rate == 0 || channels == 0 {
        return None;
    }
    let mut last_granule = 0u64;
    let tail_start = b.len().saturating_sub(65_536);
    if let Some(rel) = b[tail_start..].windows(4).rposition(|w| w == b"OggS") {
        let off = tail_start + rel;
        if off + 14 <= b.len() {
            let g = u64::from_le_bytes(b[off + 6..off + 14].try_into().ok()?);
            if g != u64::MAX {
                last_granule = g;
            }
        }
    }
    let duration_us = last_granule.saturating_mul(1_000_000) / rate as u64;
    Some((rate, channels, duration_us))
}

/// Zero-copy fast audio metadata probe (WAV / OGG Vorbis / OGG Opus, Symphonia fallback for MP3/FLAC).
pub fn fast_probe_audio_meta(raw: &[u8], codec: MediaCodec) -> (u32, u16, u64) {
    match codec {
        MediaCodec::Wav => {
            if let Some(m) = probe_wav_header_meta(raw) {
                return m;
            }
        }
        MediaCodec::Ogg => {
            if let Some(m) = probe_ogg_vorbis_fast(raw) {
                return m;
            }
            if let Some(m) = crate::opus_enc::probe_ogg_opus_meta(raw) {
                return m;
            }
        }
        MediaCodec::Opus => {
            if let Some(m) = crate::opus_enc::probe_ogg_opus_meta(raw) {
                return m;
            }
        }
        _ => {}
    }
    probe_audio_meta_symphonia(raw, codec.ext())
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

/// Encodes gapless VBR MP3 using LAME MTRH:
/// - Mono voice/SFX: `VBR -V 5` (`Quality::Good`, ~65 kbps avg — -59% vs 160k CBR!)
/// - Stereo BGM/SFX: `VBR -V 3` (`Quality::VeryNice`, ~155 kbps avg, joint stereo)
fn encode_gapless_mp3_vbr(wav: &WavPcm, _fallback_kbps: u16) -> Result<Vec<u8>> {
    use mp3lame_encoder::{FlushGap, InterleavedPcm, Mode, MonoPcm, Quality, VbrMode};
    let (mode, vbr_q) = if wav.channels == 1 {
        (Mode::Mono, Quality::Good) // -V 5 (~65 kbps mono)
    } else {
        (Mode::JointStereo, Quality::VeryNice) // -V 3 (~155 kbps joint stereo)
    };
    let mut builder = mp3lame_encoder::Builder::new().context("LAME init")?;
    builder = builder
        .with_num_channels(wav.channels as u8)
        .and_then(|b| b.with_sample_rate(wav.rate))
        .and_then(|b| b.with_output_sample_rate(std::num::NonZeroU32::new(wav.rate)))
        .and_then(|b| b.with_mode(mode))
        .and_then(|b| b.with_vbr_mode(VbrMode::Mtrh))
        .and_then(|b| b.with_vbr_quality(vbr_q))
        .and_then(|b| b.with_quality(Quality::VeryNice))
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

fn probe_audio_meta_symphonia(bytes: &[u8], ext: &str) -> (u32, u16, u64) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn build_wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&channels.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        w.extend_from_slice(&(channels * 2).to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for &s in samples {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    }

    #[test]
    fn dual_mono_and_silence_trim_and_vbr_mp3_and_opus_work() {
        let dir = tempfile::tempdir().unwrap();
        // 0.2s leading silence + 0.3s 440Hz tone + 0.2s trailing silence in DUAL-MONO stereo!
        let sr = 44_100u32;
        let mut stereo = Vec::new();
        for _ in 0..(sr / 5) {
            stereo.extend_from_slice(&[0i16, 0i16]);
        }
        for i in 0..(sr * 3 / 10) {
            let v = ((2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr as f32).sin() * 14000.0)
                as i16;
            stereo.extend_from_slice(&[v, v]); // identical L == R (dual-mono!)
        }
        for _ in 0..(sr / 5) {
            stereo.extend_from_slice(&[0i16, 0i16]);
        }
        let wav = build_wav(sr, 2, &stereo);

        // 1. Default LAME VBR MP3: should detect dual-mono -> 1 channel, trim silence (~0.34s), write LAME tag!
        let cfg_mp3 = OptimizeConfig::default();
        let res_mp3 = optimize_audio(&wav, MediaCodec::Wav, &cfg_mp3, dir.path()).unwrap();
        assert_eq!(res_mp3.codec, MediaCodec::Mp3);
        assert_eq!(res_mp3.channels, 1, "dual-mono should auto-downmix to mono");
        assert!(res_mp3.duration_us < 420_000, "silence should be trimmed: {}", res_mp3.duration_us);
        assert!(nir_ir::lame::parse(&res_mp3.bytes).is_some());

        // 2. Opus mode: should produce even smaller Ogg Opus at 48 kHz mono!
        let cfg_opus = OptimizeConfig {
            audio_codec: AudioCodecMode::Opus,
            ..Default::default()
        };
        let res_opus = optimize_audio(&wav, MediaCodec::Wav, &cfg_opus, dir.path()).unwrap();
        assert_eq!(res_opus.codec, MediaCodec::Opus);
        assert_eq!(res_opus.channels, 1);
        assert_eq!(res_opus.sample_rate, 48_000);
        assert!(res_opus.bytes.len() < res_mp3.bytes.len() + 1024);
    }
}
