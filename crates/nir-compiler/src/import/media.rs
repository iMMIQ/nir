//! Offline, bounded GAL and audio conversion. No external image/audio process.
use anyhow::{bail, ensure, Context, Result};
use image::{ImageEncoder, RgbaImage};
use std::io::{Cursor, Read};

const MAX_PIXELS: usize = 16 * 1024 * 1024;
const MAX_AUDIO: usize = 64 * 1024 * 1024;

struct Bytes<'a> {
    data: &'a [u8],
    at: usize,
}
impl<'a> Bytes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .context("E_IMPORT_MEDIA: offset overflow")?;
        let b = self
            .data
            .get(self.at..end)
            .context("E_IMPORT_MEDIA: truncated data")?;
        self.at = end;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn blob(&mut self) -> Result<&'a [u8]> {
        let len = self.u32()? as usize;
        self.take(len)
    }
}
fn int(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(at..at + 4)
            .context("E_IMPORT_MEDIA: truncated header")?
            .try_into()?,
    ))
}
fn inflate(data: &[u8], max: usize) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(max as u64 + 1)
        .read_to_end(&mut result)?;
    ensure!(
        result.len() <= max,
        "E_IMPORT_MEDIA_LIMIT: inflated data exceeds limit"
    );
    Ok(result)
}

pub(super) fn gal_size(data: &[u8]) -> Result<(u32, u32)> {
    ensure!(
        data.starts_with(b"Gale105") || data.starts_with(b"Gale106"),
        "E_IMPORT_GAL_VERSION: expected Gale105/106"
    );
    let width = int(data, 15)?;
    let height = int(data, 19)?;
    ensure!(
        width > 0
            && height > 0
            && width <= 8192
            && height <= 8192
            && (width as usize) * (height as usize) <= MAX_PIXELS,
        "E_IMPORT_GAL_SIZE: unsupported dimensions"
    );
    Ok((width, height))
}

/// Single-frame GAL 105/106. Multiple visible layers are composited with their
/// origin, opacity and color key. Unresolved forward block references fail.
pub(super) fn gal(data: &[u8]) -> Result<RgbaImage> {
    let (width, height) = gal_size(data)?;
    let mut r = Bytes::new(data);
    r.take(7)?;
    let header = r.blob()?;
    ensure!(header.len() >= 36, "E_IMPORT_GAL: short header");
    ensure!(
        int(header, 16)? == 1,
        "E_IMPORT_GAL_FRAMES: animated GAL requires animation adaptation"
    );
    ensure!(
        header[21] == 0,
        "E_IMPORT_GAL_ENCRYPTION: encrypted GAL is unsupported"
    );
    let compression = header[22];
    ensure!(
        compression <= 1,
        "E_IMPORT_GAL_COMPRESSION: unsupported compression"
    );
    let bw = int(header, 28)? as usize;
    let bh = int(header, 32)? as usize;
    ensure!(
        bw <= 8192 && bh <= 8192 && ((bw == 0) == (bh == 0)),
        "E_IMPORT_GAL_BLOCK: invalid block size"
    );
    r.blob()?; // frame name
    r.take(13)?; // mask and frame metadata
    let count = r.u32()? as usize;
    ensure!(
        (1..=64).contains(&count),
        "E_IMPORT_GAL_LAYERS: invalid layer count"
    );
    let fw = r.u32()? as usize;
    let fh = r.u32()? as usize;
    ensure!(
        fw == width as usize && fh == height as usize,
        "E_IMPORT_GAL_SIZE: inconsistent frame dimensions"
    );
    let bpp = r.u32()? as usize;
    ensure!(
        matches!(bpp, 8 | 24 | 32),
        "E_IMPORT_GAL_PIXEL: supported depths are 8, 24 and 32"
    );
    let palette = if bpp == 8 { r.take(1024)? } else { &[] };
    let stride = (fw * (bpp / 8) + 3) & !3;
    let alpha_stride = (fw + 3) & !3;
    let layer_bytes = (stride + alpha_stride)
        .checked_mul(fh)
        .and_then(|n| n.checked_mul(count))
        .context("E_IMPORT_GAL_LIMIT: layer storage overflow")?;
    ensure!(
        layer_bytes <= 128 * 1024 * 1024,
        "E_IMPORT_GAL_LIMIT: layer storage exceeds 128 MiB"
    );
    let mut previous = Vec::new();
    let mut previous_alpha = Vec::new();
    let mut output = RgbaImage::new(width, height);
    for _ in 0..count {
        let x = r.u32()? as i32;
        let y = r.u32()? as i32;
        let visible = r.u8()? != 0;
        let key = r.u32()?;
        let opacity = r.u32()?;
        ensure!(opacity <= 255, "E_IMPORT_GAL_ALPHA: invalid layer opacity");
        let has_alpha = r.u8()? != 0;
        r.blob()?; // layer name
        let packed = r.blob()?;
        let packed_alpha = r.blob()?;
        let limit = stride * fh + fw * fh * 8;
        let unpacked;
        let packed = if compression == 0 {
            unpacked = inflate(packed, limit)?;
            unpacked.as_slice()
        } else {
            packed
        };
        let pixels = blocks(packed, fw, fh, bpp / 8, bw, bh, &previous)?;
        let alpha = if has_alpha {
            let raw;
            let packed_alpha = if compression == 0 {
                raw = inflate(packed_alpha, alpha_stride * fh + fw * fh * 8)?;
                raw.as_slice()
            } else {
                packed_alpha
            };
            blocks(packed_alpha, fw, fh, 1, bw, bh, &previous_alpha)?
        } else {
            vec![255; alpha_stride * fh]
        };
        if visible {
            for row in 0..fh {
                for col in 0..fw {
                    let dx = x as i64 + col as i64;
                    let dy = y as i64 + row as i64;
                    if dx < 0 || dy < 0 || dx >= width as i64 || dy >= height as i64 {
                        continue;
                    }
                    let at = row * stride + col * (bpp / 8);
                    let c = if bpp == 8 {
                        &palette[pixels[at] as usize * 4..][..4]
                    } else {
                        &pixels[at..at + bpp / 8]
                    };
                    let mut a = alpha[row * alpha_stride + col] as u32;
                    if bpp == 32 {
                        a = a * c[3] as u32 / 255;
                    }
                    // Delphi TColor is packed R + G<<8 + B<<16; -1 disables keying.
                    if key != u32::MAX
                        && key == c[2] as u32 | ((c[1] as u32) << 8) | ((c[0] as u32) << 16)
                    {
                        a = 0;
                    }
                    a = a * opacity / 255;
                    let dst = &mut output.get_pixel_mut(dx as u32, dy as u32).0;
                    let da = dst[3] as u32;
                    let oa = a + da * (255 - a) / 255;
                    for channel in 0..3 {
                        let mixed =
                            c[2 - channel] as u32 * a + dst[channel] as u32 * da * (255 - a) / 255;
                        dst[channel] = mixed.checked_div(oa).unwrap_or(0) as u8;
                    }
                    dst[3] = oa as u8;
                }
            }
        }
        previous.push(pixels);
        previous_alpha.push(alpha);
    }
    // GAL stores two optional rectangle lists after its frame pixels (editor
    // regions / hit regions). They do not alter the raster; still parse and
    // bound them rather than accepting an arbitrary opaque trailing payload.
    if r.at < data.len() {
        for _ in 0..2 {
            let count = r.u32()? as usize;
            ensure!(count <= 4096, "E_IMPORT_GAL_REGIONS: too many rectangles");
            for _ in 0..count {
                for _ in 0..4 {
                    let coordinate = r.u32()? as i32;
                    ensure!(
                        (-8192..=8192).contains(&coordinate),
                        "E_IMPORT_GAL_REGIONS: rectangle out of bounds"
                    );
                }
            }
        }
    }
    ensure!(
        r.at == data.len(),
        "E_IMPORT_GAL_TRAILING: unexpected bytes"
    );
    Ok(output)
}

fn blocks(
    data: &[u8],
    w: usize,
    h: usize,
    channels: usize,
    bw: usize,
    bh: usize,
    previous: &[Vec<u8>],
) -> Result<Vec<u8>> {
    let stride = (w * channels + 3) & !3;
    if bw == 0 || bh == 0 {
        ensure!(
            data.len() == stride * h,
            "E_IMPORT_GAL_PIXELS: invalid raster length"
        );
        return Ok(data.to_vec());
    }
    let columns = w.div_ceil(bw);
    let rows = h.div_ceil(bh);
    let mut r = Bytes::new(data);
    let references = (0..columns * rows)
        .map(|_| Ok((r.u32()? as i32, r.u32()? as i32)))
        .collect::<Result<Vec<_>>>()?;
    let mut out = vec![0; stride * h];
    for (index, (frame, layer)) in references.iter().copied().enumerate() {
        let x = index % columns * bw;
        let y = index / columns * bh;
        let rw = bw.min(w - x) * channels;
        let rh = bh.min(h - y);
        for row in 0..rh {
            let dst = (y + row) * stride + x * channels;
            match frame {
                -1 => out[dst..dst + rw].copy_from_slice(r.take(rw)?),
                -2 => {
                    ensure!(
                        layer >= 0 && (layer as usize) < index,
                        "E_IMPORT_GAL_REFERENCE: invalid/forward block reference"
                    );
                    let sx = layer as usize % columns * bw;
                    let sy = layer as usize / columns * bh;
                    ensure!(
                        sx * channels + rw <= w * channels && sy + rh <= h,
                        "E_IMPORT_GAL_REFERENCE: block exceeds raster"
                    );
                    let src = (sy + row) * stride + sx * channels;
                    out.copy_within(src..src + rw, dst);
                }
                0 => {
                    let source = previous
                        .get(usize::try_from(layer)?)
                        .context("E_IMPORT_GAL_REFERENCE: missing earlier layer")?;
                    out[dst..dst + rw].copy_from_slice(&source[dst..dst + rw]);
                }
                _ => bail!("E_IMPORT_GAL_REFERENCE: invalid frame reference"),
            }
        }
    }
    ensure!(
        r.at == data.len(),
        "E_IMPORT_GAL_PIXELS: trailing block bytes"
    );
    Ok(out)
}
pub(super) fn blacken(image: &mut RgbaImage) {
    for pixel in image.pixels_mut() {
        pixel.0[..3].fill(0);
    }
}

pub(super) fn png(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    image::codecs::png::PngEncoder::new(&mut bytes).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(bytes)
}

#[allow(clippy::chunks_exact_to_as_chunks)] // Rust 1.85 MSRV
pub(super) fn audio(data: &[u8], gain: f32) -> Result<Vec<u8>> {
    ensure!(
        gain.is_finite() && (0.0..=4.0).contains(&gain),
        "E_IMPORT_AUDIO_GAIN: invalid volume"
    );
    let (channels, rate, mut samples) = if data.starts_with(b"OggS") {
        let mut decoder = lewton::inside_ogg::OggStreamReader::new(Cursor::new(data))?;
        let channels = decoder.ident_hdr.audio_channels as usize;
        let rate = decoder.ident_hdr.audio_sample_rate;
        ensure!(
            matches!(channels, 1 | 2) && (8000..=192000).contains(&rate),
            "E_IMPORT_OGG_FORMAT: expected mono/stereo Vorbis"
        );
        let mut samples = vec![];
        let serial = decoder.stream_serial();
        while let Some(packet) = decoder.read_dec_packet_itl()? {
            ensure!(
                decoder.stream_serial() == serial,
                "E_IMPORT_OGG_CHAIN: chained streams unsupported"
            );
            ensure!(
                samples.len() + packet.len() <= MAX_AUDIO / 2,
                "E_IMPORT_AUDIO_LIMIT: decoded audio exceeds 64 MiB"
            );
            samples.extend(packet);
        }
        (channels, rate, samples)
    } else {
        pcm(data)?
    };
    ensure!(
        matches!(channels, 1 | 2 | 6)
            && (8000..=192000).contains(&rate)
            && samples.len() % channels == 0,
        "E_IMPORT_AUDIO_FORMAT: unsupported channel count/rate"
    );
    let output_channels = channels.min(2);
    if channels == 6 {
        // WAVE 5.1 order: FL FR FC LFE BL BR; retain center/surround content.
        samples = samples
            .chunks_exact(6)
            .flat_map(|c| {
                let left =
                    c[0] as f32 + 0.707 * c[2] as f32 + 0.5 * c[3] as f32 + 0.707 * c[4] as f32;
                let right =
                    c[1] as f32 + 0.707 * c[2] as f32 + 0.5 * c[3] as f32 + 0.707 * c[5] as f32;
                [quantize(left / 2.914), quantize(right / 2.914)]
            })
            .collect();
    }
    for sample in &mut samples {
        *sample = quantize(*sample as f32 * gain);
    }
    let mut out = b"RIFF".to_vec();
    out.extend((36 + samples.len() as u32 * 2).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((output_channels as u16).to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * output_channels as u32 * 2).to_le_bytes());
    out.extend((output_channels as u16 * 2).to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend((samples.len() as u32 * 2).to_le_bytes());
    for sample in samples {
        out.extend(sample.to_le_bytes());
    }
    Ok(out)
}
fn quantize(sample: f32) -> i16 {
    sample.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}
#[allow(clippy::chunks_exact_to_as_chunks)] // Rust 1.85 MSRV
fn pcm(data: &[u8]) -> Result<(usize, u32, Vec<i16>)> {
    ensure!(
        data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WAVE",
        "E_IMPORT_AUDIO: expected WAV or Ogg/Vorbis"
    );
    let mut r = Bytes::new(&data[12..]);
    let mut format = None;
    let mut pcm = None;
    while r.at < r.data.len() {
        let kind = r.take(4)?;
        let b = r.blob()?;
        if kind == b"fmt " {
            ensure!(b.len() >= 16, "E_IMPORT_WAV: short format");
            format = Some(b);
        }
        if kind == b"data" {
            pcm = Some(b);
        }
        if b.len() % 2 == 1 {
            r.take(1)?;
        }
    }
    let f = format.context("E_IMPORT_WAV: missing format")?;
    let p = pcm.context("E_IMPORT_WAV: missing samples")?;
    ensure!(
        f[0..2] == [1, 0] && f[14..16] == [16, 0] && p.len() % 2 == 0 && p.len() <= MAX_AUDIO,
        "E_IMPORT_WAV: expected bounded PCM16"
    );
    let channels = u16::from_le_bytes(f[2..4].try_into()?) as usize;
    let rate = int(f, 4)?;
    ensure!(
        u16::from_le_bytes(f[12..14].try_into()?) as usize == channels * 2,
        "E_IMPORT_WAV: invalid block alignment"
    );
    Ok((
        channels,
        rate,
        p.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn gal_fixture(compressed: bool, alpha: bool) -> Vec<u8> {
        use std::io::Write;
        fn blob(out: &mut Vec<u8>, b: &[u8]) {
            out.extend((b.len() as u32).to_le_bytes());
            out.extend(b);
        }
        fn zip(b: &[u8]) -> Vec<u8> {
            let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
            z.write_all(b).unwrap();
            z.finish().unwrap()
        }
        let mut out = b"Gale106".to_vec();
        let mut header = vec![0; 40];
        for (at, n) in [(4, 2u32), (8, 1), (12, 24), (16, 1)] {
            header[at..at + 4].copy_from_slice(&n.to_le_bytes());
        }
        header[22] = if compressed { 0 } else { 1 };
        blob(&mut out, &header);
        blob(&mut out, &[]);
        out.extend([0; 13]);
        for n in [1u32, 2, 1, 24, 0, 0] {
            out.extend(n.to_le_bytes());
        }
        out.push(1);
        out.extend(u32::MAX.to_le_bytes());
        out.extend(255u32.to_le_bytes());
        out.push(alpha as u8);
        blob(&mut out, &[]);
        let pixels = [0, 0, 255, 0, 255, 0, 0, 0];
        blob(
            &mut out,
            &if compressed {
                zip(&pixels)
            } else {
                pixels.to_vec()
            },
        );
        blob(
            &mut out,
            &if alpha {
                if compressed {
                    zip(&[128, 255, 0, 0])
                } else {
                    vec![128, 255, 0, 0]
                }
            } else {
                vec![]
            },
        );
        out
    }
    #[test]
    fn gal_raw_and_deflate_preserve_color_and_alpha() {
        for compressed in [false, true] {
            for alpha in [false, true] {
                let image = gal(&gal_fixture(compressed, alpha)).unwrap();
                assert_eq!(
                    image.get_pixel(0, 0).0,
                    [255, 0, 0, if alpha { 128 } else { 255 }]
                );
                assert_eq!(image.get_pixel(1, 0).0, [0, 255, 0, 255]);
                assert_eq!(
                    image::load_from_memory(&png(&image).unwrap())
                        .unwrap()
                        .to_rgba8(),
                    image
                );
            }
        }
    }
    #[test]
    fn gal_block_references_are_bounded() {
        let mut data = vec![];
        for (f, l) in [(-1i32, 0i32), (-2, 0)] {
            data.extend(f.to_le_bytes());
            data.extend(l.to_le_bytes());
        }
        data.extend([1, 2, 3]);
        assert_eq!(
            blocks(&data, 2, 1, 3, 1, 1, &[]).unwrap(),
            vec![1, 2, 3, 1, 2, 3, 0, 0]
        );
        data[12..16].copy_from_slice(&1i32.to_le_bytes());
        assert!(blocks(&data, 2, 1, 3, 1, 1, &[]).is_err());
        let mut truncated = gal_fixture(true, false);
        truncated.truncate(truncated.len() - 1);
        assert!(gal(&truncated).is_err());
    }
    #[test]
    fn gal_rejects_excessive_layer_storage_before_allocating_pixels() {
        let mut data = gal_fixture(false, false);
        for (offset, value) in [(15, 8192u32), (19, 2048), (68, 64), (72, 8192), (76, 2048)] {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        assert!(gal(&data)
            .unwrap_err()
            .to_string()
            .contains("E_IMPORT_GAL_LIMIT"));
    }

    #[test]
    fn pcm_downmix_and_gain_are_bounded() {
        let samples = [1000i16, 2000, 0, 0, 0, 0];
        let mut wave = b"RIFF".to_vec();
        wave.extend(48u32.to_le_bytes());
        wave.extend(b"WAVEfmt ");
        wave.extend(16u32.to_le_bytes());
        wave.extend(1u16.to_le_bytes());
        wave.extend(6u16.to_le_bytes());
        wave.extend(44100u32.to_le_bytes());
        wave.extend(529200u32.to_le_bytes());
        wave.extend(12u16.to_le_bytes());
        wave.extend(16u16.to_le_bytes());
        wave.extend(b"data");
        wave.extend(12u32.to_le_bytes());
        for s in samples {
            wave.extend(s.to_le_bytes());
        }
        let decoded = audio(&wave, 1.).unwrap();
        let (channels, rate, pcm) = pcm(&decoded).unwrap();
        assert_eq!((channels, rate), (2, 44100));
        assert_eq!(pcm, vec![343, 686]);
        assert!(audio(&wave, f32::NAN).is_err());
        assert!(audio(&wave[..40], 1.).is_err());
    }
}
