//! Offline, bounded GAL and audio conversion. No external image/audio process.
use anyhow::{bail, ensure, Context, Result};
use image::{ImageEncoder, RgbaImage};
use std::io::{Cursor, Read};

const MAX_PIXELS: usize = 16 * 1024 * 1024;
const MAX_AUDIO: usize = 128 * 1024 * 1024;

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
    if data.starts_with(b"GaleX200") {
        let mut input = Bytes::new(data);
        input.take(8)?;
        let xml = galx_directory(input.blob()?)?;
        let doc = roxmltree::Document::parse_with_options(
            &xml,
            roxmltree::ParsingOptions {
                allow_dtd: false,
                nodes_limit: 16_384,
            },
        )?;
        let root = doc.root_element();
        ensure!(
            root.tag_name().name() == "Frames"
                && root.attribute("Version") == Some("200")
                && root.attribute("Count") == Some("1"),
            "E_IMPORT_GAL_FRAMES: expected static GaleX200"
        );
        ensure!(
            root.attribute("Randomized") == Some("0"),
            "E_IMPORT_GAL_ENCRYPTION: encrypted GAL is unsupported"
        );
        let width: u32 = root
            .attribute("Width")
            .context("E_IMPORT_GAL_XML: Width")?
            .parse()?;
        let height: u32 = root
            .attribute("Height")
            .context("E_IMPORT_GAL_XML: Height")?
            .parse()?;
        ensure!(
            width > 0
                && height > 0
                && width <= 8192
                && height <= 8192
                && width as usize * height as usize <= MAX_PIXELS,
            "E_IMPORT_GAL_SIZE: unsupported dimensions"
        );
        return Ok((width, height));
    }
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

/// Source profiles supply the documented state count and order. Dimensions
/// alone never decide whether a raster contains states or animation frames.
pub(super) fn horizontal_states(image: &RgbaImage, count: u32) -> Result<Vec<RgbaImage>> {
    ensure!(
        (1..=16).contains(&count)
            && image.width() > 0
            && image.height() > 0
            && image.width().is_multiple_of(count),
        "E_IMPORT_UI_SKIN: invalid horizontal state strip"
    );
    let width = image.width() / count;
    ensure!(width > 0, "E_IMPORT_UI_SKIN: empty state");
    Ok((0..count)
        .map(|n| image::imageops::crop_imm(image, n * width, 0, width, image.height()).to_image())
        .collect())
}

/// Bake a finite, static source TileNew into one PNG without adding runtime
/// tiling semantics or changing the source tile's alpha and edge pixels.
pub(super) fn tile(image: &RgbaImage, width: u32, height: u32) -> Result<RgbaImage> {
    ensure!(
        image.width() > 0
            && image.height() > 0
            && width > 0
            && height > 0
            && width <= 8192
            && height <= 8192
            && width as usize * height as usize <= MAX_PIXELS,
        "E_IMPORT_UI_TILE: invalid tiled dimensions"
    );
    Ok(RgbaImage::from_fn(width, height, |x, y| {
        *image.get_pixel(x % image.width(), y % image.height())
    }))
}

/// Single-frame GAL 105/106. Multiple visible layers are composited with their
/// origin, opacity and color key. Unresolved forward block references fail.
pub(super) fn gal(data: &[u8]) -> Result<RgbaImage> {
    if data.starts_with(b"GaleX200") {
        return gal(&galx_container(data)?);
    }
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
        compression <= 2,
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
        let pixels = if compression == 2 {
            use image::ImageDecoder;
            ensure!(bpp == 24, "E_IMPORT_GAL_JPEG: expected RGB depth");
            let mut decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(packed))?;
            ensure!(
                decoder.dimensions() == (width, height),
                "E_IMPORT_GAL_JPEG: inconsistent dimensions"
            );
            decoder.set_limits(image::Limits::default())?;
            let rgb = image::DynamicImage::from_decoder(decoder)?.to_rgb8();
            let mut output = vec![0u8; stride * fh];
            for y in 0..fh {
                for x in 0..fw {
                    let c = rgb.get_pixel(x as u32, y as u32).0;
                    output[y * stride + x * 3..y * stride + x * 3 + 3]
                        .copy_from_slice(&[c[2], c[1], c[0]]);
                }
            }
            output
        } else {
            let unpacked;
            let packed = if compression == 0 {
                unpacked = inflate(packed, limit)?;
                unpacked.as_slice()
            } else {
                packed
            };
            blocks(packed, fw, fh, bpp / 8, bw, bh, &previous)?
        };
        let alpha = if has_alpha {
            let raw;
            let packed_alpha = if matches!(compression, 0 | 2) {
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

/// GaleX200 uses a zlib XML directory and the same bounded pixel blocks as
/// GAL106. Normalize its static directory, then reuse the raster compositor.
/// Format references: pylivemaker galimage and GARbro LiveMaker ImageGALX.
fn galx_container(data: &[u8]) -> Result<Vec<u8>> {
    let mut input = Bytes::new(data);
    ensure!(
        input.take(8)? == b"GaleX200",
        "E_IMPORT_GAL_VERSION: expected GaleX200"
    );
    let clean = galx_directory(input.blob()?)?;
    let doc = roxmltree::Document::parse_with_options(
        &clean,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 16_384,
        },
    )
    .context("E_IMPORT_GAL_XML: invalid XML")?;
    let root = doc.root_element();
    ensure!(root.tag_name().name() == "Frames", "E_IMPORT_GAL_XML: root");
    let number = |node: roxmltree::Node<'_, '_>, name: &str| -> Result<i32> {
        node.attribute(name)
            .with_context(|| format!("E_IMPORT_GAL_XML: missing {name}"))?
            .parse()
            .with_context(|| format!("E_IMPORT_GAL_XML: invalid {name}"))
    };
    ensure!(
        number(root, "Version")? == 200 && number(root, "Count")? == 1,
        "E_IMPORT_GAL_FRAMES: expected static GaleX200"
    );
    ensure!(
        number(root, "Randomized")? == 0,
        "E_IMPORT_GAL_ENCRYPTION: encrypted GAL is unsupported"
    );
    let compression = number(root, "CompType")?;
    ensure!(
        (0..=2).contains(&compression),
        "E_IMPORT_GAL_COMPRESSION: unsupported compression"
    );
    let width = number(root, "Width")?;
    let height = number(root, "Height")?;
    ensure!(
        width > 0
            && height > 0
            && width <= 8192
            && height <= 8192
            && width as usize * height as usize <= MAX_PIXELS,
        "E_IMPORT_GAL_SIZE: unsupported dimensions"
    );
    let frames: Vec<_> = root.children().filter(|n| n.is_element()).collect();
    ensure!(
        frames.len() == 1 && frames[0].tag_name().name() == "Frame",
        "E_IMPORT_GAL_XML: frames"
    );
    let directories: Vec<_> = frames[0].children().filter(|n| n.is_element()).collect();
    ensure!(
        directories.len() == 1 && directories[0].tag_name().name() == "Layers",
        "E_IMPORT_GAL_XML: layers"
    );
    let layers = directories[0];
    let count = number(layers, "Count")?;
    let bpp = number(layers, "Bpp")?;
    ensure!(
        (1..=64).contains(&count) && matches!(bpp, 8 | 24 | 32),
        "E_IMPORT_GAL_LAYERS: count/depth"
    );
    ensure!(
        number(layers, "Width")? == width
            && number(layers, "Height")? == height
            && number(root, "Bpp")? == bpp,
        "E_IMPORT_GAL_SIZE: inconsistent frame dimensions"
    );
    let mut header = vec![0u8; 36];
    for (offset, value) in [
        (4, width),
        (8, height),
        (12, bpp),
        (16, 1),
        (28, number(root, "BlockWidth")?),
        (32, number(root, "BlockHeight")?),
    ] {
        ensure!(value >= 0, "E_IMPORT_GAL_XML: negative header field");
        header[offset..offset + 4].copy_from_slice(&(value as u32).to_le_bytes());
    }
    header[22] = compression as u8;
    let blob = |out: &mut Vec<u8>, bytes: &[u8]| {
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    };
    let mut out = b"Gale106".to_vec();
    blob(&mut out, &header);
    blob(&mut out, b"");
    out.extend_from_slice(&[0; 13]);
    for value in [count, width, height, bpp] {
        out.extend_from_slice(&(value as u32).to_le_bytes());
    }
    if bpp == 8 {
        let rgb = layers
            .children()
            .find(|n| n.has_tag_name("RGB"))
            .and_then(|n| n.text())
            .context("E_IMPORT_GAL_PALETTE: missing palette")?;
        ensure!(
            rgb.len() <= 256 * 6 && rgb.len().is_multiple_of(6) && rgb.is_ascii(),
            "E_IMPORT_GAL_PALETTE: size"
        );
        let mut palette = vec![0u8; 1024];
        for (index, color) in rgb.as_bytes().as_chunks::<6>().0.iter().enumerate() {
            let hex = std::str::from_utf8(color)?;
            let c = u32::from_str_radix(hex, 16)?;
            palette[index * 4..index * 4 + 4].copy_from_slice(&[
                c as u8,
                (c >> 8) as u8,
                (c >> 16) as u8,
                0,
            ]);
        }
        out.extend_from_slice(&palette);
    }
    let entries: Vec<_> = layers
        .children()
        .filter(|n| n.has_tag_name("Layer"))
        .collect();
    ensure!(
        entries.len() == count as usize,
        "E_IMPORT_GAL_LAYERS: inconsistent directory"
    );
    for entry in entries {
        for name in ["Left", "Top"] {
            out.extend_from_slice(&number(entry, name)?.to_le_bytes());
        }
        let flag = |name| -> Result<u8> {
            let n = number(entry, name)?;
            ensure!(matches!(n, 0 | 1), "E_IMPORT_GAL_XML: invalid flag");
            Ok(n as u8)
        };
        out.push(flag("Visible")?);
        out.extend_from_slice(&number(entry, "TransColor")?.to_le_bytes());
        let alpha = number(entry, "Alpha")?;
        ensure!((0..=255).contains(&alpha), "E_IMPORT_GAL_ALPHA: opacity");
        out.extend_from_slice(&alpha.to_le_bytes());
        let alpha_on = flag("AlphaOn")?;
        out.push(alpha_on);
        blob(&mut out, b"");
        blob(&mut out, input.blob()?);
        let alpha_data = input.blob()?;
        ensure!(
            alpha_on != 0 || alpha_data.is_empty(),
            "E_IMPORT_GAL_ALPHA: unexpected disabled alpha payload"
        );
        blob(&mut out, alpha_data);
    }
    ensure!(
        input.at == data.len(),
        "E_IMPORT_GAL_TRAILING: unexpected GaleX bytes"
    );
    Ok(out)
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
    if data.starts_with(b"OggS") {
        let mut decoder = lewton::inside_ogg::OggStreamReader::new(Cursor::new(data))?;
        let channels = decoder.ident_hdr.audio_channels as usize;
        let rate = decoder.ident_hdr.audio_sample_rate;
        ensure!(
            matches!(channels, 1 | 2) && (8000..=192000).contains(&rate),
            "E_IMPORT_OGG_FORMAT: expected mono/stereo Vorbis"
        );
        let mut out = wave_header(channels, rate, 0);
        let serial = decoder.stream_serial();
        while let Some(packet) = decoder.read_dec_packet_itl()? {
            ensure!(
                decoder.stream_serial() == serial,
                "E_IMPORT_OGG_CHAIN: chained streams unsupported"
            );
            ensure!(
                packet.len() % channels == 0 && packet.len() <= (MAX_AUDIO - (out.len() - 44)) / 2,
                "E_IMPORT_AUDIO_LIMIT: decoded audio exceeds 128 MiB"
            );
            let needed = out.len() + packet.len() * 2;
            if needed > out.capacity() {
                let capacity = needed.next_power_of_two().min(MAX_AUDIO + 44);
                out.reserve_exact(capacity - out.len());
            }
            for sample in packet {
                out.extend(quantize(sample as f32 * gain).to_le_bytes());
            }
        }
        let bytes = (out.len() - 44) as u32;
        out[4..8].copy_from_slice(&(bytes + 36).to_le_bytes());
        out[40..44].copy_from_slice(&bytes.to_le_bytes());
        return Ok(out);
    }
    let (channels, rate, mut samples) = pcm(data)?;
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
    let mut out = wave_header(output_channels, rate, samples.len() as u32 * 2);
    out.reserve_exact(samples.len() * 2);
    for sample in samples {
        out.extend(sample.to_le_bytes());
    }
    Ok(out)
}
fn wave_header(channels: usize, rate: u32, bytes: u32) -> Vec<u8> {
    let mut out = b"RIFF".to_vec();
    out.extend((36 + bytes).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((channels as u16).to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * channels as u32 * 2).to_le_bytes());
    out.extend((channels as u16 * 2).to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(bytes.to_le_bytes());
    out
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

fn galx_directory(compressed: &[u8]) -> Result<String> {
    let xml = inflate(compressed, 1024 * 1024)?;
    let xml = std::str::from_utf8(&xml).context("E_IMPORT_GAL_XML: UTF-8")?;
    // GraphicsGale writes a duplicate Count attribute on Frame. Its editor
    // rectangles don't affect a static raster. Remove only this tag's
    // attributes; retain all root, layer and pixel metadata for validation.
    let mut clean = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(at) = rest.find("<Frame ") {
        clean.push_str(&rest[..at]);
        let end = rest[at..]
            .find('>')
            .context("E_IMPORT_GAL_XML: frame tag")?
            + at;
        ensure!(
            !rest[at + 1..end].contains('<'),
            "E_IMPORT_GAL_XML: frame tag"
        );
        clean.push_str("<Frame>");
        rest = &rest[end + 1..];
    }
    clean.push_str(rest);
    Ok(clean)
}

/// Layout discovery reads only the GAL directory, never the pixel payload.
/// Export still decodes and validates every referenced pixel/layer block.
pub(super) fn gal_file_static_size(path: &std::path::Path) -> Result<(u32, u32)> {
    let size = gal_file_size(path)?;
    let mut file = std::fs::File::open(path)?;
    let mut prefix = [0u8; 31];
    file.read_exact(&mut prefix)?;
    if !prefix.starts_with(b"GaleX200") {
        ensure!(
            i32::from_le_bytes(prefix[27..31].try_into()?) == 1,
            "E_IMPORT_GAL_FRAMES: animated GAL requires animation adaptation"
        );
    }
    Ok(size)
}

pub(super) fn gal_file_size(path: &std::path::Path) -> Result<(u32, u32)> {
    let mut file = std::fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() <= 32 * 1024 * 1024,
        "E_IMPORT_LIMIT: source file exceeds 32 MiB"
    );
    let mut prefix = [0u8; 12];
    file.read_exact(&mut prefix)?;
    let mut directory = prefix.to_vec();
    let length = if prefix.starts_with(b"GaleX200") {
        let length = u32::from_le_bytes(prefix[8..12].try_into().unwrap()) as usize;
        ensure!(
            length <= 2 * 1024 * 1024,
            "E_IMPORT_GAL_XML: compressed directory exceeds limit"
        );
        length
    } else {
        11
    };
    let mut remainder = vec![0u8; length];
    file.read_exact(&mut remainder)?;
    directory.extend(remainder);
    gal_size(&directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "private long Vorbis fixture; set NIR_IMPORT_AUDIO_SOURCE and NIR_IMPORT_AUDIO_FRAMES"]
    fn external_long_vorbis_preserves_complete_frames_header_gain_and_clipping() {
        let data = std::fs::read(std::env::var("NIR_IMPORT_AUDIO_SOURCE").unwrap()).unwrap();
        let frames: usize = std::env::var("NIR_IMPORT_AUDIO_FRAMES")
            .unwrap()
            .parse()
            .unwrap();
        let gain = 2.;
        let wav = audio(&data, gain).unwrap();
        let mut decoder = lewton::inside_ogg::OggStreamReader::new(Cursor::new(&data)).unwrap();
        let channels = decoder.ident_hdr.audio_channels as usize;
        assert_eq!(wav.len(), 44 + frames * channels * 2);
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
            wav.len() - 8
        );
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            decoder.ident_hdr.audio_sample_rate
        );
        assert_eq!(
            u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize,
            wav.len() - 44
        );
        let mut at = 44;
        while let Some(packet) = decoder.read_dec_packet_itl().unwrap() {
            for sample in packet {
                let expected = (f32::from(sample) * gain).round().clamp(-32768., 32767.) as i16;
                assert_eq!(
                    i16::from_le_bytes(wav[at..at + 2].try_into().unwrap()),
                    expected
                );
                at += 2;
            }
        }
        assert_eq!(at, wav.len());
        println!(
            "LONG_AUDIO_OK frames={frames} channels={channels} pcmBytes={}",
            wav.len() - 44
        );
        if let Ok(output) = std::env::var("NIR_IMPORT_AUDIO_OUTPUT") {
            std::fs::write(output, audio(&data, 1.).unwrap()).unwrap();
        }
    }
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
    fn static_file_discovery_reads_directory_and_rejects_animated_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("neutral.gal");
        let mut bytes = gal_fixture(false, false);
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(gal_file_static_size(&path).unwrap(), (2, 1));
        bytes[27..31].copy_from_slice(&2i32.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(gal_file_size(&path).unwrap(), (2, 1));
        assert!(gal_file_static_size(&path).is_err());
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
    fn galx_static_directory_preserves_pixels_and_rejects_invalid_containers() {
        use std::io::Write;
        let deflate = |bytes: &[u8]| {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        };
        for compression in [0, 1, 2] {
            let xml = format!(
                r#"<Frames Version="200" Width="2" Height="1" Bpp="24" Count="1" Randomized="0" CompType="{compression}" BlockWidth="0" BlockHeight="0"><Frame Name="test" Count="0" Count="1"><Layers Count="1" Width="2" Height="1" Bpp="24"><Layer Left="0" Top="0" Visible="1" TransColor="-1" Alpha="255" AlphaOn="1"/></Layers></Frame></Frames>"#
            );
            let make = |xml: &str| {
                let mut out = b"GaleX200".to_vec();
                let mut blob = |bytes: &[u8]| {
                    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                    out.extend_from_slice(bytes);
                };
                blob(&deflate(xml.as_bytes()));
                for (index, bytes) in [&[0, 0, 255, 0, 255, 0, 0, 0][..], &[128, 255, 0, 0][..]]
                    .into_iter()
                    .enumerate()
                {
                    if compression == 2 && index == 0 {
                        let mut jpeg = Vec::new();
                        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 100)
                            .encode(
                                &[80, 80, 80, 160, 160, 160],
                                2,
                                1,
                                image::ExtendedColorType::Rgb8,
                            )
                            .unwrap();
                        blob(&jpeg);
                    } else if compression != 1 {
                        blob(&deflate(bytes));
                    } else {
                        blob(bytes);
                    }
                }
                out
            };
            let data = make(&xml);
            let directory_end = 12 + u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
            let directory = &data[..directory_end];
            let temporary = tempfile::tempdir().unwrap();
            let path = temporary.path().join("directory.gal");
            std::fs::write(&path, directory).unwrap();
            assert_eq!(gal_file_size(&path).unwrap(), (2, 1));
            assert!(
                gal(directory).is_err(),
                "layout discovery must not replace complete pixel validation"
            );
            let image = gal(&data).unwrap();
            assert_eq!(gal_size(&data).unwrap(), (2, 1));
            if compression == 2 {
                for (x, expected) in [(0, 80i16), (1, 160)] {
                    let pixel = image.get_pixel(x, 0).0;
                    assert!(pixel[..3]
                        .iter()
                        .all(|value| (*value as i16 - expected).abs() <= 2));
                }
                assert_eq!(image.get_pixel(0, 0).0[3], 128);
                assert_eq!(image.get_pixel(1, 0).0[3], 255);
                assert!(gal(&make(&xml.replace("Width=\"2\"", "Width=\"3\""))).is_err());
            } else {
                assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 128]);
                assert_eq!(image.get_pixel(1, 0).0, [0, 255, 0, 255]);
            }
            for (from, to) in [
                ("Count=\"1\" Randomized", "Count=\"2\" Randomized"),
                ("Randomized=\"0\"", "Randomized=\"1\""),
                ("Alpha=\"255\"", "Alpha=\"256\""),
                ("Width=\"2\"", "Width=\"999999\""),
                ("</Frames>", "</Frames><unexpected/>"),
            ] {
                assert!(gal(&make(&xml.replace(from, to))).is_err());
            }
            for length in 0..data.len() {
                assert!(gal(&data[..length]).is_err());
            }
            let dtd = format!("<!DOCTYPE Frames [<!ENTITY x SYSTEM 'file:///etc/passwd'>]>{xml}");
            assert!(gal(&make(&dtd)).is_err());
            let mut tail = data.clone();
            tail.push(0);
            assert!(gal(&tail).is_err());
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
