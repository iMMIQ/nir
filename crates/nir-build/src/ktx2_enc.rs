//! Pure-Rust KTX2 GPU-compressed texture encoder (BC1 RGB 8:1 / BC3 RGBA 4:1 + ZLIB supercompression).
//! Produces spec-compliant KTX2 containers readable by `ktx2::Reader` and uploadable directly
//! to GPU VRAM (`Bc1RgbaUnormSrgb` / `Bc3RgbaUnormSrgb`) with pure-Rust `bcdec_rs` fallback!

use flate2::{write::ZlibEncoder, Compression};
use std::io::Write;
use texpresso::{Algorithm, Format, Params};

const KTX2_IDENTIFIER: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];

// Standard Khronos Vulkan VkFormat IDs
const VK_FORMAT_BC1_RGBA_SRGB_BLOCK: u32 = 134;
const VK_FORMAT_BC3_SRGB_BLOCK: u32 = 138;

// Standard KTX2 supercompressionScheme = 3 (ZLIB)
const SUPERCOMPRESSION_ZLIB: u32 = 3;

/// Encodes an RGBA8 image (padded cleanly to multiple of 4 pixels when needed) into a
/// ZLIB-supercompressed KTX2 BC1 (opaque, 8:1 VRAM) or BC3 (alpha, 4:1 VRAM) texture.
pub fn encode_ktx2_bcn(rgba: &[u8], width: u32, height: u32, has_alpha: bool) -> Vec<u8> {
    let bw = (width as usize).div_ceil(4);
    let bh = (height as usize).div_ceil(4);
    let pad_w = bw * 4;
    let pad_h = bh * 4;

    // Premultiply RGB by alpha for BC3 so direct GPU hardware sampling matches NIR's
    // premultiplied-alpha blend pipeline with zero edge halos!
    let mut padded: Vec<u8> = if pad_w == width as usize && pad_h == height as usize {
        rgba.to_vec()
    } else {
        let mut buf = vec![0u8; pad_w * pad_h * 4];
        let w = width as usize;
        let h = height as usize;
        for y in 0..pad_h {
            let sy = y.min(h - 1);
            let src_row = &rgba[sy * w * 4..(sy + 1) * w * 4];
            let dst_row = &mut buf[y * pad_w * 4..(y + 1) * pad_w * 4];
            dst_row[..w * 4].copy_from_slice(src_row);
            let last_px = &src_row[(w - 1) * 4..w * 4];
            for x in w..pad_w {
                dst_row[x * 4..(x + 1) * 4].copy_from_slice(last_px);
            }
        }
        buf
    };
    if has_alpha {
        for px in padded.as_chunks_mut::<4>().0 {
            let a = px[3] as u16;
            if a == 0 {
                px[0] = 0;
                px[1] = 0;
                px[2] = 0;
            } else if a < 255 {
                px[0] = ((px[0] as u16 * a + 127) / 255) as u8;
                px[1] = ((px[1] as u16 * a + 127) / 255) as u8;
                px[2] = ((px[2] as u16 * a + 127) / 255) as u8;
            }
        }
    }

    let (fmt, vk_fmt) = if has_alpha {
        (Format::Bc3, VK_FORMAT_BC3_SRGB_BLOCK)
    } else {
        (Format::Bc1, VK_FORMAT_BC1_RGBA_SRGB_BLOCK)
    };

    let mut raw_blocks = vec![0u8; fmt.compressed_size(pad_w, pad_h)];
    let params = Params {
        algorithm: Algorithm::ClusterFit,
        weigh_colour_by_alpha: false,
        ..Params::default()
    };
    fmt.compress(&padded, pad_w, pad_h, params, &mut raw_blocks);

    // ZLIB supercompress the BCn block payload (typically shrinks BC1/BC3 on anime CGs by 2x-4x on disk!)
    let mut zenc = ZlibEncoder::new(Vec::with_capacity(raw_blocks.len() / 2), Compression::default());
    let _ = zenc.write_all(&raw_blocks);
    let compressed = zenc.finish().unwrap_or_else(|_| raw_blocks.clone());

    // Minimal valid KTX2 Data Format Descriptor (44 bytes total: 4B totalSize + 40B basic block)
    let color_model: u8 = if has_alpha { 130 } else { 128 }; // KHR_DF_MODEL_BC3 / KHR_DF_MODEL_BC1A
    let dfd_total_size: u32 = 4 + 24 + 16; // 1 basic descriptor (24B) + 1 sample (16B)
    let mut dfd = Vec::with_capacity(dfd_total_size as usize);
    dfd.extend_from_slice(&dfd_total_size.to_le_bytes());
    // Basic DFD block header (8 bytes): vendorId(0) + descriptorType(0) + versionNumber(2) + descriptorBlockSize(40)
    dfd.extend_from_slice(&0u32.to_le_bytes());
    dfd.extend_from_slice(&2u16.to_le_bytes());
    dfd.extend_from_slice(&40u16.to_le_bytes());
    // colorModel, colorPrimaries (BT709=1), transferFunction (SRGB=2), flags (KHR_DF_FLAG_ALPHA_PREMULTIPLIED=1 for BC3)
    dfd.push(color_model);
    dfd.push(1);
    dfd.push(2);
    dfd.push(u8::from(has_alpha));
    // texelBlockDimension 0..3 (4x4x1x1 stored as dim-1: 3, 3, 0, 0)
    dfd.extend_from_slice(&[3, 3, 0, 0]);
    // bytesPlane 0..7 (8 bytes/block for BC1, 16 bytes/block for BC3)
    let bpb: u8 = if has_alpha { 16 } else { 8 };
    dfd.extend_from_slice(&[bpb, 0, 0, 0, 0, 0, 0, 0]);
    // Sample 0 (16 bytes): bitOffset(0), bitLength(bpb*8 - 1), channelType(0), samplePosition(0..3), lower(0), upper(u32::MAX)
    dfd.extend_from_slice(&0u16.to_le_bytes());
    dfd.push(bpb.saturating_mul(8).saturating_sub(1));
    dfd.push(0);
    dfd.extend_from_slice(&[0, 0, 0, 0]);
    dfd.extend_from_slice(&0u32.to_le_bytes());
    dfd.extend_from_slice(&u32::MAX.to_le_bytes());

    // Header (80B) + LevelIndex (1 * 24B = 24B) = 104 bytes -> DFD at 104 (already 4-byte aligned)
    let dfd_offset: u32 = 80 + 24;
    let dfd_len: u32 = dfd.len() as u32;
    let kvd_offset: u32 = 0;
    let kvd_len: u32 = 0;
    let sgd_offset: u64 = 0;
    let sgd_len: u64 = 0;

    // Align level 0 data to 16 bytes
    let after_dfd = (dfd_offset + dfd_len) as usize;
    let level_pad = (16 - (after_dfd % 16)) % 16;
    let level_offset = (after_dfd + level_pad) as u64;

    let mut out = Vec::with_capacity(level_offset as usize + compressed.len());
    out.extend_from_slice(&KTX2_IDENTIFIER); // 0..12
    out.extend_from_slice(&vk_fmt.to_le_bytes()); // 12..16 vkFormat
    out.extend_from_slice(&1u32.to_le_bytes()); // 16..20 typeSize
    out.extend_from_slice(&width.to_le_bytes()); // 20..24 pixelWidth
    out.extend_from_slice(&height.to_le_bytes()); // 24..28 pixelHeight
    out.extend_from_slice(&0u32.to_le_bytes()); // 28..32 pixelDepth
    out.extend_from_slice(&0u32.to_le_bytes()); // 32..36 layerCount
    out.extend_from_slice(&1u32.to_le_bytes()); // 36..40 faceCount
    out.extend_from_slice(&1u32.to_le_bytes()); // 40..44 levelCount
    out.extend_from_slice(&SUPERCOMPRESSION_ZLIB.to_le_bytes()); // 44..48 supercompressionScheme
    out.extend_from_slice(&dfd_offset.to_le_bytes()); // 48..52
    out.extend_from_slice(&dfd_len.to_le_bytes()); // 52..56
    out.extend_from_slice(&kvd_offset.to_le_bytes()); // 56..60
    out.extend_from_slice(&kvd_len.to_le_bytes()); // 60..64
    out.extend_from_slice(&sgd_offset.to_le_bytes()); // 64..72
    out.extend_from_slice(&sgd_len.to_le_bytes()); // 72..80
    // Level 0 index (24 bytes): byteOffset, byteLength, uncompressedByteLength
    out.extend_from_slice(&level_offset.to_le_bytes());
    out.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
    out.extend_from_slice(&(raw_blocks.len() as u64).to_le_bytes());
    out.extend_from_slice(&dfd);
    out.resize(out.len() + level_pad, 0);
    out.extend_from_slice(&compressed);
    out
}
