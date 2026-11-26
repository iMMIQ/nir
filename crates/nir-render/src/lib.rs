//! High-performance ordered 2D wgpu compositor with linear premultiplied blending,
//! multi-blend pipelines (Normal/Add/Multiply/Screen), indexed quads, GPU mipmaps,
//! cached offscreen transitions, zero-clone glyphon text, SIMD WebP decode, and LRU GPU texture cache.
#![forbid(unsafe_code)]

use bytemuck::{Pod, Zeroable};
use flate2::read::ZlibDecoder;
use glyphon::{
    Cache, Color, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use nir_ir::{AssetId, BlendMode, ImageDelta, NirError, Result};
use nir_ui::{DrawPacket, Quad, SpecialQuad, TextEngine, TextRun};
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
pub use wgpu;

const DESKTOP_GPU_CACHE_LIMIT: u64 = 256 * 1024 * 1024;
const ANDROID_GPU_CACHE_LIMIT: u64 = 128 * 1024 * 1024;
const WEBGL2_GPU_CACHE_LIMIT: u64 = 96 * 1024 * 1024;
const ATLAS_TRIM_INTERVAL: u64 = 64;

fn default_cache_limit_for(backend: RendererBackend) -> u64 {
    if matches!(backend, RendererBackend::WebGl2 | RendererBackend::Gles) {
        WEBGL2_GPU_CACHE_LIMIT
    } else if cfg!(target_os = "android") {
        ANDROID_GPU_CACHE_LIMIT
    } else {
        DESKTOP_GPU_CACHE_LIMIT
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererBackend {
    Dx12,
    Metal,
    Vulkan,
    Gles,
    WebGpu,
    WebGl2,
}

impl RendererBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dx12 => "dx12",
            Self::Metal => "metal",
            Self::Vulkan => "vulkan",
            Self::Gles => "gles",
            Self::WebGpu => "webgpu",
            Self::WebGl2 => "webgl2",
        }
    }

    pub const fn is_gl(self) -> bool {
        matches!(self, Self::WebGl2 | Self::Gles)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    rect_params: [f32; 4],
    border_color: [f32; 4],
    /// `[local_u, local_v, param (glass_rim | shadow_blur), mode (0=std, 1=shadow, 2=frosted)]`
    extra: [f32; 4],
}

struct GpuTexture {
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    _texture: wgpu::Texture,
    bytes: u64,
    last_used: u64,
}

struct PreparedFrame {
    ran_text_prepare: bool,
    inline_quad_start: usize,
    render_from: bool,
    render_to: bool,
    tr_from_quad_start: usize,
    tr_to_quad_start: usize,
    run_blur: bool,
    blur_stage_quad_start: usize,
    blur_pass1_quad: usize,
    blur_pass2_quad: usize,
}

pub struct Renderer {
    _instance: wgpu::Instance,
    pub device: wgpu::Device,
    queue: wgpu::Queue,
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    pipelines: [wgpu::RenderPipeline; 4],
    plain_pipeline: wgpu::RenderPipeline,
    mix_pipeline: wgpu::RenderPipeline,
    mip_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    mip_vertices: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white_tex: GpuTexture,
    textures: HashMap<AssetId, GpuTexture>,
    last_pinned: BTreeSet<AssetId>,
    evicted_ids: Vec<AssetId>,
    texture_bytes: u64,
    texture_limit: u64,
    texture_epoch: u64,
    use_clock: u64,
    scratch: Option<(GpuTexture, GpuTexture, u32, u32)>,
    scratch_from_sig: Option<u64>,
    scratch_to_sig: Option<u64>,
    blur_pair: Option<(GpuTexture, GpuTexture, u32, u32)>,
    blur_stage_sig: Option<u64>,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    scratch_verts: Vec<Vertex>,
    quad_capacity: usize,
    pub text: TextEngine,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    viewport: Viewport,
    last_viewport: [u32; 2],
    last_text_sig: Option<u64>,
    text_font_epoch: u64,
    text_prepares: u64,
    cached_inline_quads: Vec<Quad>,
    swash: SwashCache,
    pub submitted: u64,
    pub backend: RendererBackend,
    pub adapter_info: String,
    lost: Arc<AtomicBool>,
}

fn rerr(msg: impl Into<String>) -> NirError {
    NirError::new("E_RENDER", "wgpu", msg)
}

fn linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Exact 64 KiB LUT for sRGB-encoded linear-space premultiplication.
static PREMULTIPLY_LUT: OnceLock<[u8; 65536]> = OnceLock::new();

fn premultiply_lut() -> &'static [u8; 65536] {
    PREMULTIPLY_LUT.get_or_init(|| {
        let mut table = [0u8; 65536];
        for alpha in 1u16..=254 {
            let a = alpha as f32 / 255.0;
            for ch in 0u16..=255 {
                table[(alpha * 256 + ch) as usize] = (srgb(linear(ch as f32 / 255.0) * a) * 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
        for ch in 0u16..=255 {
            table[(255 * 256 + ch) as usize] = ch as u8;
        }
        table
    })
}

pub fn premultiply_rgba(pixels: &mut [u8]) {
    let table = premultiply_lut();
    for px in pixels.as_chunks_mut::<4>().0 {
        match px[3] {
            255 => {}
            0 => px[..3].fill(0),
            a => {
                let row = a as usize * 256;
                px[0] = table[row + px[0] as usize];
                px[1] = table[row + px[1] as usize];
                px[2] = table[row + px[2] as usize];
            }
        }
    }
}

/// Decodes WebP (via SIMD libwebp), PNG, and JPEG into premultiplied RGBA8
/// (skips premultiplication pass completely for opaque images!).
#[cfg(not(target_arch = "wasm32"))]
pub fn decode_image(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    // SIMD libwebp fast path for WebP assets (the primary optimized format!).
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        if let Some(decoded) = webp::Decoder::new(bytes).decode() {
            let w = decoded.width();
            let h = decoded.height();
            if w == 0 || h == 0 || w > 8192 || h > 8192 {
                return Err(rerr("image dimensions out of range"));
            }
            let pixels = match decoded.layout() {
                webp::PixelLayout::Rgb => {
                    let src: &[u8] = &decoded;
                    let mut out = Vec::with_capacity((w as usize) * (h as usize) * 4);
                    for px in src.as_chunks::<3>().0 {
                        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                    }
                    out
                }
                webp::PixelLayout::Rgba => {
                    let mut out = decoded.to_vec();
                    premultiply_rgba(&mut out);
                    out
                }
            };
            return Ok((w, h, pixels));
        }
    }

    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| rerr(e.to_string()))?;
    let img = reader.decode().map_err(|e| rerr(e.to_string()))?;
    let w = img.width();
    let h = img.height();
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(rerr("image dimensions out of range"));
    }
    let has_alpha = img.color().has_alpha();
    let mut pixels = img.to_rgba8().into_raw();
    if has_alpha {
        premultiply_rgba(&mut pixels);
    }
    Ok((w, h, pixels))
}

/// Center-crops an RGBA8 image to `target_w:target_h` (with slight upper bias when cropping height
/// so character faces stay nicely framed), resizes, and encodes compact WebP (q=72) for save thumbnails.
#[cfg(not(target_arch = "wasm32"))]
pub fn encode_thumbnail_webp(
    rgba: &[u8],
    width: u32,
    height: u32,
    target_w: u32,
    target_h: u32,
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || target_w == 0 || target_h == 0 {
        return Err(rerr("thumbnail dimensions must be non-zero"));
    }
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| rerr("thumbnail RGBA buffer length mismatch"))?;
    let target_ar = target_w as f32 / target_h as f32;
    let src_ar = width as f32 / height as f32;
    let (cx, cy, cw, ch) = if src_ar > target_ar {
        let cw = ((height as f32 * target_ar).round() as u32).clamp(1, width);
        let cx = (width - cw) / 2;
        (cx, 0, cw, height)
    } else if src_ar < target_ar {
        let ch = ((width as f32 / target_ar).round() as u32).clamp(1, height);
        // Upper bias keeps character faces visible in 16:9 thumbnails.
        let cy = (((height - ch) as f32 * 0.35).round() as u32).min(height - ch);
        (0, cy, width, ch)
    } else {
        (0, 0, width, height)
    };
    let cropped = image::imageops::crop_imm(&img, cx, cy, cw, ch).to_image();
    let resized = image::imageops::resize(
        &cropped,
        target_w,
        target_h,
        image::imageops::FilterType::Triangle,
    );
    let encoded = webp::Encoder::from_rgba(resized.as_raw(), target_w, target_h).encode(72.0);
    Ok(encoded.to_vec())
}

/// Encodes an RGBA8 image as lossless PNG (used for borderless full-res screenshots).
#[cfg(not(target_arch = "wasm32"))]
pub fn encode_png_rgba(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    if width == 0 || height == 0 || rgba.len() != (width as usize) * (height as usize) * 4 {
        return Err(rerr("png RGBA buffer dimension mismatch"));
    }
    let mut out = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut out);
    encoder
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| rerr(format!("png encode: {e}")))?;
    Ok(out)
}

/// Converts a premultiplied RGBA8 image into a square straight-alpha RGBA8 window icon.
#[cfg(not(target_arch = "wasm32"))]
pub fn resize_rgba_icon(
    premultiplied_rgba: &[u8],
    width: u32,
    height: u32,
    icon_size: u32,
) -> Result<Vec<u8>> {
    if width == 0
        || height == 0
        || icon_size == 0
        || premultiplied_rgba.len() != (width as usize) * (height as usize) * 4
    {
        return Err(rerr("icon RGBA buffer dimension mismatch"));
    }
    let mut straight = premultiplied_rgba.to_vec();
    for px in straight.as_chunks_mut::<4>().0 {
        let a = px[3];
        if a > 0 && a < 255 {
            let af = a as f32 / 255.0;
            for c in &mut px[..3] {
                let lin = linear(*c as f32 / 255.0) / af;
                *c = (srgb(lin.min(1.0)) * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    let img = image::RgbaImage::from_raw(width, height, straight)
        .ok_or_else(|| rerr("icon image buffer error"))?;
    let side = width.min(height);
    let cx = (width - side) / 2;
    let cy = ((height - side) as f32 * 0.35).round() as u32;
    let cropped = image::imageops::crop_imm(&img, cx, cy.min(height - side), side, side).to_image();
    let resized = image::imageops::resize(
        &cropped,
        icon_size,
        icon_size,
        image::imageops::FilterType::Triangle,
    );
    Ok(resized.into_raw())
}

/// Composites a decoded premultiplied RGBA8 differential patch onto a premultiplied RGBA8 base.
/// - Opaque CG mode (`tile_mask` empty): pixels with `alpha >= 128` replace `base` cleanly.
/// - Transparent sprite mode (`tile_mask` non-empty): 16×16 tiles with bit=1 overwrite `base` verbatim,
///   preserving exact alpha deletions and semi-transparent edges!
pub fn composite_delta_rgba(
    base_rgba: &[u8],
    base_w: u32,
    base_h: u32,
    patch_rgba: &[u8],
    patch_w: u32,
    patch_h: u32,
    delta: &ImageDelta,
) -> Result<Vec<u8>> {
    let bw = base_w as usize;
    let bh = base_h as usize;
    let pw = patch_w as usize;
    let ph = patch_h as usize;
    if base_rgba.len() != bw * bh * 4 || patch_rgba.len() != pw * ph * 4 {
        return Err(rerr("delta composite buffer dimension mismatch"));
    }
    let mut out = base_rgba.to_vec();
    let ox = delta.x as usize;
    let oy = delta.y as usize;
    if delta.tile_mask.is_empty() {
        for y in 0..ph {
            let dy = oy + y;
            if dy >= bh {
                break;
            }
            for x in 0..pw {
                let dx = ox + x;
                if dx >= bw {
                    break;
                }
                let so = (y * pw + x) * 4;
                if patch_rgba[so + 3] >= 128 {
                    let doff = (dy * bw + dx) * 4;
                    out[doff..doff + 3].copy_from_slice(&patch_rgba[so..so + 3]);
                    out[doff + 3] = 255;
                }
            }
        }
    } else {
        const TILE: usize = 16;
        let p_tiles_x = pw.div_ceil(TILE);
        let p_tiles_y = ph.div_ceil(TILE);
        for pty in 0..p_tiles_y {
            for ptx in 0..p_tiles_x {
                let bit = pty * p_tiles_x + ptx;
                let mask_byte = delta.tile_mask.get(bit / 8).copied().unwrap_or(0);
                if (mask_byte & (1u8 << (bit % 8))) == 0 {
                    continue;
                }
                let y0 = pty * TILE;
                let y1 = (y0 + TILE).min(ph);
                let x0 = ptx * TILE;
                let x1 = (x0 + TILE).min(pw);
                for y in y0..y1 {
                    let dy = oy + y;
                    if dy >= bh {
                        break;
                    }
                    let copy_w = x1.min(bw.saturating_sub(ox)).saturating_sub(x0);
                    if copy_w == 0 {
                        continue;
                    }
                    let so = (y * pw + x0) * 4;
                    let doff = (dy * bw + ox + x0) * 4;
                    out[doff..doff + copy_w * 4].copy_from_slice(&patch_rgba[so..so + copy_w * 4]);
                }
            }
        }
    }
    Ok(out)
}

/// Decoded KTX2 GPU-compressed level-0 texture payload (BC1 / BC3 / BC7 / ASTC 4x4).
#[derive(Debug, Clone)]
pub struct DecodedKtx2 {
    pub width: u32,
    pub height: u32,
    pub format: Ktx2BlockFormat,
    pub premultiplied: bool,
    pub blocks: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ktx2BlockFormat {
    Bc1Srgb,
    Bc3Srgb,
    Bc7Srgb,
    Astc4x4Srgb,
}

impl Ktx2BlockFormat {
    pub const fn block_bytes(self) -> u32 {
        match self {
            Self::Bc1Srgb => 8,
            Self::Bc3Srgb | Self::Bc7Srgb | Self::Astc4x4Srgb => 16,
        }
    }

    pub const fn wgpu_format(self) -> wgpu::TextureFormat {
        match self {
            Self::Bc1Srgb => wgpu::TextureFormat::Bc1RgbaUnormSrgb,
            Self::Bc3Srgb => wgpu::TextureFormat::Bc3RgbaUnormSrgb,
            Self::Bc7Srgb => wgpu::TextureFormat::Bc7RgbaUnormSrgb,
            Self::Astc4x4Srgb => wgpu::TextureFormat::Astc {
                block: wgpu::AstcBlock::B4x4,
                channel: wgpu::AstcChannel::UnormSrgb,
            },
        }
    }

    pub const fn required_feature(self) -> wgpu::Features {
        match self {
            Self::Bc1Srgb | Self::Bc3Srgb | Self::Bc7Srgb => wgpu::Features::TEXTURE_COMPRESSION_BC,
            Self::Astc4x4Srgb => wgpu::Features::TEXTURE_COMPRESSION_ASTC,
        }
    }
}

/// Parses a standard KTX2 container (including ZLIB supercompression) into raw GPU blocks.
pub fn parse_ktx2_level0(bytes: &[u8]) -> Result<DecodedKtx2> {
    let reader = ktx2::Reader::new(bytes).map_err(|e| rerr(format!("ktx2 parse: {e:?}")))?;
    let hdr = reader.header();
    let width = hdr.pixel_width;
    let height = hdr.pixel_height;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(rerr("ktx2 dimensions out of range"));
    }
    let fmt = match hdr.format {
        Some(ktx2::Format::BC1_RGBA_SRGB_BLOCK | ktx2::Format::BC1_RGB_SRGB_BLOCK) => {
            Ktx2BlockFormat::Bc1Srgb
        }
        Some(ktx2::Format::BC3_SRGB_BLOCK) => Ktx2BlockFormat::Bc3Srgb,
        Some(ktx2::Format::BC7_SRGB_BLOCK) => Ktx2BlockFormat::Bc7Srgb,
        Some(ktx2::Format::ASTC_4x4_SRGB_BLOCK) => Ktx2BlockFormat::Astc4x4Srgb,
        other => return Err(rerr(format!("unsupported ktx2 VkFormat: {other:?}"))),
    };
    let level = reader
        .levels()
        .next()
        .ok_or_else(|| rerr("ktx2 has no mip levels"))?;
    let blocks = match hdr.supercompression_scheme {
        None => level.data.to_vec(),
        Some(ktx2::SupercompressionScheme::ZLIB) => {
            let mut out = Vec::with_capacity(level.uncompressed_byte_length as usize);
            ZlibDecoder::new(level.data)
                .read_to_end(&mut out)
                .map_err(|e| rerr(format!("ktx2 zlib inflate: {e}")))?;
            out
        }
        Some(other) => {
            return Err(rerr(format!(
                "unsupported ktx2 supercompression scheme: {other:?}"
            )))
        }
    };
    let bw = width.div_ceil(4) as usize;
    let bh = height.div_ceil(4) as usize;
    let expected = bw * bh * fmt.block_bytes() as usize;
    if blocks.len() < expected {
        return Err(rerr("ktx2 level-0 payload truncated"));
    }
    let premultiplied = reader.dfd_blocks().iter().any(|b| {
        matches!(
            b,
            ktx2::dfd::Block::Basic(basic)
                if basic
                    .flags
                    .contains(ktx2::dfd::DataFormatFlags::ALPHA_PREMULTIPLIED)
        )
    });
    Ok(DecodedKtx2 {
        width,
        height,
        format: fmt,
        premultiplied,
        blocks,
    })
}

/// Pure-Rust CPU fallback decoder for BC1/BC3/BC7 KTX2 textures when the GPU adapter
/// lacks hardware `TEXTURE_COMPRESSION_BC` (e.g., WebGL2 or older mobile GPUs).
pub fn decode_ktx2_to_rgba_premultiplied(ktx: &DecodedKtx2) -> Result<Vec<u8>> {
    let w = ktx.width as usize;
    let h = ktx.height as usize;
    let bw = w.div_ceil(4);
    let bh = h.div_ceil(4);
    let bpb = ktx.format.block_bytes() as usize;
    let mut out = vec![0u8; w * h * 4];
    let mut blk_rgba = [0u8; 4 * 4 * 4];

    for by in 0..bh {
        for bx in 0..bw {
            let src_off = (by * bw + bx) * bpb;
            let src = &ktx.blocks[src_off..src_off + bpb];
            match ktx.format {
                Ktx2BlockFormat::Bc1Srgb => bcdec_rs::bc1(src, &mut blk_rgba, 16),
                Ktx2BlockFormat::Bc3Srgb => bcdec_rs::bc3(src, &mut blk_rgba, 16),
                Ktx2BlockFormat::Bc7Srgb => bcdec_rs::bc7(src, &mut blk_rgba, 16),
                Ktx2BlockFormat::Astc4x4Srgb => {
                    return Err(rerr("ASTC CPU fallback decode unsupported"))
                }
            }
            for py in 0..4 {
                let dy = by * 4 + py;
                if dy >= h {
                    break;
                }
                let copy_px = 4.min(w - bx * 4);
                let dst_off = (dy * w + bx * 4) * 4;
                let s_off = py * 16;
                out[dst_off..dst_off + copy_px * 4]
                    .copy_from_slice(&blk_rgba[s_off..s_off + copy_px * 4]);
            }
        }
    }
    if ktx.format != Ktx2BlockFormat::Bc1Srgb && !ktx.premultiplied {
        premultiply_rgba(&mut out);
    }
    Ok(out)
}

/// Smart on-demand mipmap count: 1:1 / upscaled 2D CGs & BGs skip unnecessary mip chains
/// (saving 25% VRAM and 4 offscreen GPU render passes per uploaded texture!), while textures
/// genuinely downscaled relative to the active viewport get up to 3 mip levels.
fn mip_level_count_for(w: u32, h: u32, vp_w: u32, vp_h: u32, backend: RendererBackend) -> u32 {
    if backend.is_gl() || w.max(h) < 256 {
        return 1;
    }
    if w <= vp_w.saturating_mul(5) / 4 && h <= vp_h.saturating_mul(5) / 4 {
        return 1;
    }
    let max_dim = w.max(h);
    (32 - max_dim.leading_zeros()).clamp(1, 3)
}

fn mip_memory_bytes(w: u32, h: u32, mips: u32) -> u64 {
    let mut total = 0u64;
    let mut mw = w.max(1);
    let mut mh = h.max(1);
    for _ in 0..mips {
        total += mw as u64 * mh as u64 * 4;
        mw = (mw / 2).max(1);
        mh = (mh / 2).max(1);
    }
    total
}

fn build_quad_indices(quad_count: usize) -> Vec<u32> {
    let mut idx = Vec::with_capacity(quad_count * 6);
    for q in 0..quad_count as u32 {
        let b = q * 4;
        idx.extend_from_slice(&[b, b + 1, b + 2, b + 2, b + 1, b + 3]);
    }
    idx
}

fn hash_text_runs(runs: &[TextRun], dpr: f32, vp: [u32; 2], font_epoch: u64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    vp.hash(&mut h);
    dpr.to_bits().hash(&mut h);
    font_epoch.hash(&mut h);
    runs.len().hash(&mut h);
    for r in runs {
        r.cache_key.hash(&mut h);
        r.visible_bytes.hash(&mut h);
        r.x.to_bits().hash(&mut h);
        r.y.to_bits().hash(&mut h);
        r.width.to_bits().hash(&mut h);
        r.height.to_bits().hash(&mut h);
        r.scroll.to_bits().hash(&mut h);
        r.valign_center.hash(&mut h);
        for c in r.color {
            c.to_bits().hash(&mut h);
        }
        if let Some(s) = r.shadow {
            1u8.hash(&mut h);
            for o in s.offset {
                o.to_bits().hash(&mut h);
            }
            for c in s.color {
                c.to_bits().hash(&mut h);
            }
        } else {
            0u8.hash(&mut h);
        }
        if let Some(oc) = r.outline {
            1u8.hash(&mut h);
            for c in oc {
                c.to_bits().hash(&mut h);
            }
        } else {
            0u8.hash(&mut h);
        }
        if let Some(cl) = r.clip {
            1u8.hash(&mut h);
            for f in cl {
                f.to_bits().hash(&mut h);
            }
        } else {
            0u8.hash(&mut h);
        }
        r.ruby_spans.len().hash(&mut h);
        for (s, e, t) in &r.ruby_spans {
            s.hash(&mut h);
            e.hash(&mut h);
            t.hash(&mut h);
        }
        r.inline_images.hash(&mut h);
    }
    h.finish()
}

fn hash_transition_quads(quads: &[Quad], sw: u32, sh: u32, epoch: u64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    sw.hash(&mut h);
    sh.hash(&mut h);
    epoch.hash(&mut h);
    quads.len().hash(&mut h);
    for q in quads {
        for f in q.rect {
            f.to_bits().hash(&mut h);
        }
        for f in q.uv {
            f.to_bits().hash(&mut h);
        }
        if let Some(c) = q.corners {
            1u8.hash(&mut h);
            for pt in c {
                pt[0].to_bits().hash(&mut h);
                pt[1].to_bits().hash(&mut h);
            }
        } else {
            0u8.hash(&mut h);
        }
        for f in q.color {
            f.to_bits().hash(&mut h);
        }
        q.asset.hash(&mut h);
        (q.blend as u8).hash(&mut h);
        if let Some(cl) = q.clip {
            1u8.hash(&mut h);
            for f in cl {
                f.to_bits().hash(&mut h);
            }
        } else {
            0u8.hash(&mut h);
        }
    }
    h.finish()
}

fn select_surface_format(
    formats: &[wgpu::TextureFormat],
    backend: RendererBackend,
) -> Result<(
    wgpu::TextureFormat,
    wgpu::TextureFormat,
    Vec<wgpu::TextureFormat>,
)> {
    if backend.is_gl() {
        let fmt = formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| formats.first().copied())
            .ok_or_else(|| rerr("no surface format"))?;
        Ok((fmt, fmt, Vec::new()))
    } else {
        let surface_fmt = formats
            .iter()
            .copied()
            .find(|f| f.is_srgb() || f.add_srgb_suffix().is_srgb())
            .or_else(|| formats.first().copied())
            .ok_or_else(|| rerr("no surface format"))?;
        let fmt = surface_fmt.add_srgb_suffix();
        let views = if fmt == surface_fmt {
            vec![]
        } else {
            vec![fmt]
        };
        Ok((surface_fmt, fmt, views))
    }
}

fn blend_state(mode: BlendMode) -> wgpu::BlendState {
    match mode {
        BlendMode::Normal => wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        BlendMode::Add => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        },
        BlendMode::Multiply => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        },
        BlendMode::Screen => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrc,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        },
    }
}

impl Renderer {
    pub async fn new(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        backend: RendererBackend,
    ) -> Result<Self> {
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
        {
            Ok(a) => a,
            Err(_) => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: true,
                })
                .await
                .map_err(|e| rerr(e.to_string()))?,
        };
        let info = adapter.get_info();
        let backend = match info.backend {
            wgpu::Backend::Gl => {
                if cfg!(target_arch = "wasm32") {
                    RendererBackend::WebGl2
                } else {
                    RendererBackend::Gles
                }
            }
            wgpu::Backend::Vulkan => RendererBackend::Vulkan,
            wgpu::Backend::Metal => RendererBackend::Metal,
            wgpu::Backend::Dx12 => RendererBackend::Dx12,
            wgpu::Backend::BrowserWebGpu => RendererBackend::WebGpu,
            _ => backend,
        };
        let adapter_info = format!("{:?} / {} / {}", info.backend, info.name, info.driver);
        let desired_compression = wgpu::Features::TEXTURE_COMPRESSION_BC
            | wgpu::Features::TEXTURE_COMPRESSION_ASTC
            | wgpu::Features::TEXTURE_COMPRESSION_ETC2;
        let optional_features = adapter.features() & desired_compression;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("NIR v2 Renderer"),
                required_features: optional_features,
                required_limits: if backend.is_gl() {
                    wgpu::Limits::downlevel_webgl2_defaults()
                } else {
                    wgpu::Limits::downlevel_defaults()
                }
                .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| rerr(e.to_string()))?;

        let lost = Arc::new(AtomicBool::new(false));
        let lost_flag = lost.clone();
        device.set_device_lost_callback(move |_, _| {
            lost_flag.store(true, Ordering::Relaxed);
        });

        let caps = surface.get_capabilities(&adapter);
        let (surface_format, format, view_formats) = select_surface_format(&caps.formats, backend)?;
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto)
        };
        let max_dim = device.limits().max_texture_dimension_2d;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: width.clamp(1, max_dim),
            height: height.clamp(1, max_dim),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
            view_formats,
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear_mip"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nir_quad"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });
        let vlayout = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4],
        }];

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let make_pipeline = |mode: BlendMode| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("quad"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &vlayout,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend_state(mode)),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let pipelines = [
            make_pipeline(BlendMode::Normal),
            make_pipeline(BlendMode::Add),
            make_pipeline(BlendMode::Multiply),
            make_pipeline(BlendMode::Screen),
        ];

        let plain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad_plain"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &vlayout,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_plain"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend_state(BlendMode::Normal)),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let mip_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mipmap_blit"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &vlayout,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("kawase_blur"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &vlayout,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blur"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let mip_quad = [
            Vertex {
                pos: [-1.0, 1.0],
                uv: [0.0, 0.0],
                color: [1.0; 4],
                rect_params: [0.0; 4],
                border_color: [0.0; 4],
                extra: [0.0, 0.0, 0.0, 0.0],
            },
            Vertex {
                pos: [1.0, 1.0],
                uv: [1.0, 0.0],
                color: [1.0; 4],
                rect_params: [0.0; 4],
                border_color: [0.0; 4],
                extra: [1.0, 0.0, 0.0, 0.0],
            },
            Vertex {
                pos: [-1.0, -1.0],
                uv: [0.0, 1.0],
                color: [1.0; 4],
                rect_params: [0.0; 4],
                border_color: [0.0; 4],
                extra: [0.0, 1.0, 0.0, 0.0],
            },
            Vertex {
                pos: [1.0, -1.0],
                uv: [1.0, 1.0],
                color: [1.0; 4],
                rect_params: [0.0; 4],
                border_color: [0.0; 4],
                extra: [1.0, 1.0, 0.0, 0.0],
            },
        ];
        let mip_vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mip_quad"),
            size: std::mem::size_of_val(&mip_quad) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&mip_vertices, 0, bytemuck::cast_slice(&mip_quad));

        let mix_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout, &layout, &layout],
            push_constant_ranges: &[],
        });
        let mix_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("transition"),
            layout: Some(&mix_pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &vlayout,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mix"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let white_tex = Self::create_rgba_texture_raw(
            &device,
            &queue,
            &layout,
            &sampler,
            1,
            1,
            &[255, 255, 255, 255],
            1,
        );
        let initial_quads = 512usize;
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad_verts"),
            size: (initial_quads * 4 * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let idx_data = build_quad_indices(initial_quads);
        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad_indices"),
            size: (idx_data.len() * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&indices, 0, bytemuck::cast_slice(&idx_data));

        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        Ok(Self {
            _instance: instance,
            device,
            queue,
            surface: Some(surface),
            config,
            format,
            pipelines,
            plain_pipeline,
            mix_pipeline,
            mip_pipeline,
            blur_pipeline,
            mip_vertices,
            layout,
            sampler,
            white_tex,
            textures: HashMap::new(),
            last_pinned: BTreeSet::new(),
            evicted_ids: Vec::new(),
            texture_bytes: 0,
            texture_limit: default_cache_limit_for(backend),
            texture_epoch: 0,
            use_clock: 0,
            scratch: None,
            scratch_from_sig: None,
            scratch_to_sig: None,
            blur_pair: None,
            blur_stage_sig: None,
            vertices,
            indices,
            scratch_verts: Vec::with_capacity(initial_quads * 4),
            quad_capacity: initial_quads,
            text: TextEngine::default(),
            atlas,
            text_renderer,
            viewport,
            last_viewport: [0, 0],
            last_text_sig: None,
            text_font_epoch: 0,
            text_prepares: 0,
            cached_inline_quads: Vec::new(),
            swash: SwashCache::new(),
            submitted: 0,
            backend,
            adapter_info,
            lost,
        })
    }

    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    pub fn adapter_info(&self) -> &str {
        &self.adapter_info
    }

    pub fn has_font(&self, id: AssetId) -> bool {
        self.text.has_font(id)
    }

    pub fn load_font(&mut self, id: AssetId, bytes: Vec<u8>) -> Result<()> {
        if self.text.has_font(id) {
            return Ok(());
        }
        self.text
            .add_font_asset(id, bytes)
            .map_err(|e| NirError::new("E_FONT", "load_font", e))?;
        self.text_font_epoch = self.text_font_epoch.wrapping_add(1);
        self.last_text_sig = None;
        Ok(())
    }

    /// Loads a shared fallback font (e.g. Web Pan-CJK fallback) used after bundled game fonts.
    pub fn load_fallback_font(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.text
            .add_fallback_font(bytes)
            .map_err(|e| NirError::new("E_FONT", "load_fallback_font", e))?;
        self.text_font_epoch = self.text_font_epoch.wrapping_add(1);
        self.last_text_sig = None;
        Ok(())
    }

    pub fn has_any_font(&self) -> bool {
        self.text.has_any_font()
    }

    pub fn has_missing_glyphs(&self) -> bool {
        self.text.missing_glyphs
    }

    /// Lazily loads OS system fallback fonts on native platforms when bundled/subsetted fonts
    /// lack glyphs (e.g. UI locale switch or unsubsetted symbols).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn ensure_system_fallback_fonts(&mut self) -> bool {
        if self.text.ensure_system_fallback_fonts() {
            self.text_font_epoch = self.text_font_epoch.wrapping_add(1);
            self.last_text_sig = None;
            true
        } else {
            false
        }
    }

    pub fn set_texture_cache_limit(&mut self, bytes: u64) {
        self.texture_limit = bytes.max(24 * 1024 * 1024);
        self.evict_lru_keeping(None);
    }

    /// Drops all cached GPU textures and transition scratch targets (e.g. under mobile memory pressure).
    pub fn clear_textures(&mut self) -> Vec<AssetId> {
        let mut removed: Vec<AssetId> = self.textures.keys().copied().collect();
        removed.extend(std::mem::take(&mut self.evicted_ids));
        self.textures.clear();
        self.texture_bytes = 0;
        self.scratch = None;
        self.scratch_from_sig = None;
        self.scratch_to_sig = None;
        self.blur_pair = None;
        self.blur_stage_sig = None;
        self.texture_epoch = self.texture_epoch.wrapping_add(1);
        removed
    }

    pub fn texture_memory_bytes(&self) -> u64 {
        self.texture_bytes
    }

    pub fn has_texture(&self, id: AssetId) -> bool {
        self.textures.contains_key(&id)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let max_dim = self.device.limits().max_texture_dimension_2d;
        self.config.width = width.clamp(1, max_dim);
        self.config.height = height.clamp(1, max_dim);
        self.last_text_sig = None;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
    }

    pub fn release_surface(&mut self) {
        self.surface = None;
    }

    pub fn rebind_surface(&mut self, surface: wgpu::Surface<'static>) {
        surface.configure(&self.device, &self.config);
        self.last_text_sig = None;
        self.surface = Some(surface);
    }

    #[allow(clippy::too_many_arguments)]
    fn create_rgba_texture_raw(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        w: u32,
        h: u32,
        rgba: &[u8],
        mip_levels: u32,
    ) -> GpuTexture {
        let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        if mip_levels > 1 {
            usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        GpuTexture {
            view,
            bind,
            _texture: texture,
            bytes: mip_memory_bytes(w, h, mip_levels),
            last_used: 0,
        }
    }

    fn generate_mipmaps(&self, texture: &wgpu::Texture, mip_levels: u32) {
        if mip_levels <= 1 {
            return;
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gen_mips"),
            });
        let views: Vec<wgpu::TextureView> = (0..mip_levels)
            .map(|level| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        for level in 1..mip_levels as usize {
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&views[level - 1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mip_blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &views[level],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.mip_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.set_vertex_buffer(0, self.mip_vertices.slice(..));
            pass.set_index_buffer(self.indices.slice(..24), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..6, 0, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
    }

    fn offscreen(&self, w: u32, h: u32) -> GpuTexture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transition"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        GpuTexture {
            view,
            bind,
            _texture: texture,
            bytes: w as u64 * h as u64 * 4,
            last_used: 0,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn upload_image_bytes(&mut self, id: AssetId, bytes: &[u8]) -> Result<()> {
        if bytes.len() >= 12 && bytes[0..4] == [0xAB, 0x4B, 0x54, 0x58] {
            return self.upload_ktx2_bytes(id, bytes);
        }
        let (w, h, pixels) = decode_image(bytes)?;
        self.upload_rgba_premultiplied(id, w, h, &pixels)
    }

    /// Uploads a KTX2 GPU-compressed texture directly to VRAM when hardware compression
    /// (`BC1`/`BC3`/`BC7`/`ASTC`) is supported, or falls back to pure-Rust CPU decode cleanly!
    pub fn upload_ktx2_bytes(&mut self, id: AssetId, bytes: &[u8]) -> Result<()> {
        let ktx = parse_ktx2_level0(bytes)?;
        self.upload_decoded_ktx2(id, &ktx)
    }

    pub fn upload_decoded_ktx2(&mut self, id: AssetId, ktx: &DecodedKtx2) -> Result<()> {
        let max_dim = self.device.limits().max_texture_dimension_2d;
        let bw = ktx.width.div_ceil(4);
        let bh = ktx.height.div_ceil(4);
        let pad_w = bw * 4;
        let pad_h = bh * 4;
        if pad_w == 0 || pad_h == 0 || pad_w > max_dim || pad_h > max_dim {
            return Err(rerr("ktx2 exceeds GPU texture limit"));
        }
        if !self
            .device
            .features()
            .contains(ktx.format.required_feature())
        {
            let rgba = decode_ktx2_to_rgba_premultiplied(ktx)?;
            return self.upload_rgba_premultiplied(id, ktx.width, ktx.height, &rgba);
        }

        let bpb = ktx.format.block_bytes();
        let bytes_per_row = bw * bpb;
        let payload_len = (bw as usize) * (bh as usize) * (bpb as usize);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ktx2_compressed"),
            size: wgpu::Extent3d {
                width: pad_w,
                height: pad_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ktx.format.wgpu_format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &ktx.blocks[..payload_len],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(bh),
            },
            wgpu::Extent3d {
                width: pad_w,
                height: pad_h,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let added = payload_len as u64;
        self.use_clock = self.use_clock.wrapping_add(1);
        self.texture_epoch = self.texture_epoch.wrapping_add(1);
        let tex = GpuTexture {
            view,
            bind,
            _texture: texture,
            bytes: added,
            last_used: self.use_clock,
        };
        if let Some(old) = self.textures.insert(id, tex) {
            self.texture_bytes = self.texture_bytes.saturating_sub(old.bytes);
        }
        self.texture_bytes = self.texture_bytes.saturating_add(added);
        self.evict_lru_keeping(Some(id));
        Ok(())
    }

    pub fn upload_rgba_premultiplied(
        &mut self,
        id: AssetId,
        w: u32,
        h: u32,
        pixels: &[u8],
    ) -> Result<()> {
        let max_dim = self.device.limits().max_texture_dimension_2d;
        if w == 0 || h == 0 || w > max_dim || h > max_dim {
            return Err(rerr("image exceeds GPU texture limit"));
        }
        if pixels.len() as u64 != w as u64 * h as u64 * 4 {
            return Err(rerr("decoded RGBA length mismatch"));
        }
        let mips = mip_level_count_for(w, h, self.config.width, self.config.height, self.backend);
        let mut tex = Self::create_rgba_texture_raw(
            &self.device,
            &self.queue,
            &self.layout,
            &self.sampler,
            w,
            h,
            pixels,
            mips,
        );
        if mips > 1 {
            self.generate_mipmaps(&tex._texture, mips);
        }
        let added = tex.bytes;
        self.use_clock = self.use_clock.wrapping_add(1);
        self.texture_epoch = self.texture_epoch.wrapping_add(1);
        tex.last_used = self.use_clock;
        if let Some(old) = self.textures.insert(id, tex) {
            self.texture_bytes = self.texture_bytes.saturating_sub(old.bytes);
        }
        self.texture_bytes = self.texture_bytes.saturating_add(added);
        self.evict_lru_keeping(Some(id));
        Ok(())
    }

    /// Zero-CPU-copy GPU texture upload from a browser hardware-decoded `ImageBitmap`!
    #[cfg(target_arch = "wasm32")]
    pub fn upload_image_bitmap(
        &mut self,
        id: AssetId,
        bitmap: &web_sys::ImageBitmap,
    ) -> Result<()> {
        let w = bitmap.width();
        let h = bitmap.height();
        let max_dim = self.device.limits().max_texture_dimension_2d;
        if w == 0 || h == 0 || w > max_dim || h > max_dim {
            return Err(rerr("ImageBitmap exceeds GPU texture limit"));
        }
        let mips = mip_level_count_for(w, h, self.config.width, self.config.height, self.backend);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.queue.copy_external_image_to_texture(
            &wgpu::CopyExternalImageSourceInfo {
                source: wgpu::ExternalImageSource::ImageBitmap(bitmap.clone()),
                origin: wgpu::Origin2d::ZERO,
                flip_y: false,
            },
            wgpu::CopyExternalImageDestInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
                color_space: wgpu::PredefinedColorSpace::Srgb,
                // WebGL2 lacks `UNRESTRICTED_EXTERNAL_TEXTURE_COPIES` and uploads the already-premultiplied
                // `ImageBitmap` verbatim when `premultiplied_alpha: false`; WebGPU expects `true`.
                premultiplied_alpha: self.backend != RendererBackend::WebGl2,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        if mips > 1 {
            self.generate_mipmaps(&texture, mips);
        } else if self.backend == RendererBackend::WebGl2 {
            // Flush queued GLES `CopyExternalImageToTexture` before JS host calls `bitmap.close()`!
            self.queue.submit(None);
        }
        let view = texture.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.use_clock = self.use_clock.wrapping_add(1);
        self.texture_epoch = self.texture_epoch.wrapping_add(1);
        let bytes = mip_memory_bytes(w, h, mips);
        let tex = GpuTexture {
            view,
            bind,
            _texture: texture,
            bytes,
            last_used: self.use_clock,
        };
        if let Some(old) = self.textures.insert(id, tex) {
            self.texture_bytes = self.texture_bytes.saturating_sub(old.bytes);
        }
        self.texture_bytes = self.texture_bytes.saturating_add(bytes);
        self.evict_lru_keeping(Some(id));
        Ok(())
    }

    /// Graceful LRU eviction when VRAM exceeds `texture_limit`, preserving `pinned` active assets.
    /// Returns all texture `AssetId`s evicted since the previous call so `Runtime` can reload them later.
    pub fn evict_lru(&mut self, pinned: &BTreeSet<AssetId>) -> Vec<AssetId> {
        self.last_pinned = pinned.clone();
        self.evict_lru_keeping(None);
        std::mem::take(&mut self.evicted_ids)
    }

    /// Drains any texture `AssetId`s evicted implicitly during texture uploads.
    pub fn take_evicted_assets(&mut self) -> Vec<AssetId> {
        std::mem::take(&mut self.evicted_ids)
    }

    fn evict_lru_keeping(&mut self, just_uploaded: Option<AssetId>) {
        if self.texture_bytes <= self.texture_limit {
            return;
        }
        let mut victims: Vec<(AssetId, u64)> = self
            .textures
            .iter()
            .filter(|(&id, _)| Some(id) != just_uploaded && !self.last_pinned.contains(&id))
            .map(|(&id, tex)| (id, tex.last_used))
            .collect();
        victims.sort_by_key(|(_, age)| *age);
        let mut evicted_any = false;
        for (id, _) in victims {
            if self.texture_bytes <= self.texture_limit {
                break;
            }
            if let Some(tex) = self.textures.remove(&id) {
                self.texture_bytes = self.texture_bytes.saturating_sub(tex.bytes);
                self.evicted_ids.push(id);
                evicted_any = true;
            }
        }
        if evicted_any {
            self.texture_epoch = self.texture_epoch.wrapping_add(1);
        }
    }

    fn push_text_areas<'a>(
        areas: &mut Vec<TextArea<'a>>,
        entry: &'a nir_ui::text_engine::CachedTextBuffer,
        r: &TextRun,
        x: f32,
        y: f32,
        rgba: [f32; 4],
        dpr: f32,
    ) {
        let color = Color::rgba(
            (rgba[0] * 255.0) as u8,
            (rgba[1] * 255.0) as u8,
            (rgba[2] * 255.0) as u8,
            (rgba[3] * 255.0) as u8,
        );
        let y_draw = if r.valign_center && entry.content_height > 0.0 {
            y + ((r.height - entry.content_height) * 0.5).max(0.0)
        } else {
            y
        };
        let mut bounds = TextBounds {
            left: (x * dpr) as i32,
            top: (y * dpr) as i32,
            right: ((x + r.width) * dpr) as i32,
            bottom: ((y + r.height) * dpr) as i32,
        };
        if let Some([cx, cy, cw, ch]) = r.clip {
            bounds.left = bounds.left.max((cx * dpr) as i32);
            bounds.top = bounds.top.max((cy * dpr) as i32);
            bounds.right = bounds.right.min(((cx + cw) * dpr) as i32);
            bounds.bottom = bounds.bottom.min(((cy + ch) * dpr) as i32);
        }
        if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
            return;
        }
        let buf = &entry.buffer;
        if let Some(vis) = r.visible_bytes {
            let offsets = &entry.line_offsets;
            for run in buf.layout_runs() {
                let off = *offsets.get(run.line_i).unwrap_or(&0);
                let right = run
                    .glyphs
                    .iter()
                    .filter(|g| off + g.end <= vis)
                    .map(|g| g.x + g.w)
                    .fold(0.0f32, f32::max);
                if right <= 0.0 {
                    continue;
                }
                let row = TextBounds {
                    left: bounds.left,
                    top: bounds
                        .top
                        .max(((y_draw + run.line_top - r.scroll) * dpr) as i32),
                    right: bounds.right.min(((x + right + 1.0) * dpr) as i32),
                    bottom: bounds
                        .bottom
                        .min(((y_draw + run.line_top + run.line_height - r.scroll) * dpr) as i32),
                };
                if row.bottom > row.top {
                    areas.push(TextArea {
                        buffer: buf,
                        left: x * dpr,
                        top: (y_draw - r.scroll) * dpr,
                        scale: dpr,
                        bounds: row,
                        default_color: color,
                        custom_glyphs: &[],
                    });
                }
            }
        } else {
            areas.push(TextArea {
                buffer: buf,
                left: x * dpr,
                top: (y_draw - r.scroll) * dpr,
                scale: dpr,
                bounds,
                default_color: color,
                custom_glyphs: &[],
            });
        }
    }

    pub fn render(&mut self, p: &DrawPacket, dpr: f32) -> Result<()> {
        if self.surface.is_none() {
            return Ok(());
        }
        let prepared = self.prepare_render_passes(p, dpr)?;

        // Acquire swapchain frame
        let surface = self.surface.as_mut().unwrap();
        let frame = match surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                surface.configure(&self.device, &self.config);
                match surface.get_current_texture() {
                    Ok(f) => f,
                    Err(wgpu::SurfaceError::Timeout) => return Ok(()),
                    Err(e) => return Err(rerr(e.to_string())),
                }
            }
            Err(e) => return Err(rerr(e.to_string())),
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.format),
            ..Default::default()
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nir_frame"),
            });
        self.encode_passes(&mut encoder, &view, p, &prepared)?;
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.submitted += 1;
        if prepared.ran_text_prepare && self.text_prepares.is_multiple_of(ATLAS_TRIM_INTERVAL) {
            self.atlas.trim();
        }
        Ok(())
    }

    /// Renders `p` offscreen and reads back the cropped `stage_rect` region as canonical sRGB RGBA8
    /// (zero letterbox bars!). Used strictly on-demand for `KeyP` full-resolution screenshots.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture_stage_rgba(&mut self, p: &DrawPacket, dpr: f32) -> Result<(u32, u32, Vec<u8>)> {
        let w = self.config.width.max(1);
        let h = self.config.height.max(1);
        self.capture_stage_rgba_at(p, w, h, dpr)
    }

    /// Low-cost offscreen capture for save thumbnails: renders directly into a compact target
    /// (`<= max_dim` pixels along the longest edge, e.g. 320px) so mobile/high-DPI devices avoid
    /// allocating & synchronously reading back a 10+ MB full-screen texture on the UI thread!
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture_stage_thumbnail_rgba(
        &mut self,
        p: &DrawPacket,
        dpr: f32,
        max_dim: u32,
    ) -> Result<(u32, u32, Vec<u8>)> {
        let full_w = self.config.width.max(1);
        let full_h = self.config.height.max(1);
        let cap = max_dim.max(64);
        let longest = full_w.max(full_h);
        if longest <= cap {
            return self.capture_stage_rgba_at(p, full_w, full_h, dpr);
        }
        let scale = cap as f32 / longest as f32;
        let w = ((full_w as f32 * scale).round() as u32).max(16);
        let h = ((full_h as f32 * scale).round() as u32).max(16);
        let scaled_dpr = (dpr * scale).max(0.1);
        self.capture_stage_rgba_at(p, w, h, scaled_dpr)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn capture_stage_rgba_at(
        &mut self,
        p: &DrawPacket,
        w: u32,
        h: u32,
        dpr: f32,
    ) -> Result<(u32, u32, Vec<u8>)> {
        let saved_w = self.config.width;
        let saved_h = self.config.height;
        self.config.width = w;
        self.config.height = h;
        let prepared = self.prepare_render_passes(p, dpr);
        self.config.width = saved_w;
        self.config.height = saved_h;
        if w != saved_w || h != saved_h {
            self.last_viewport = [0, 0];
            self.last_text_sig = None;
        }
        let prepared = prepared?;

        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stage_capture"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nir_capture"),
            });
        // Temporarily override `config.{width,height}` so `encode_passes` computes scissors
        // against the capture target dimensions rather than the swapchain dimensions!
        self.config.width = w;
        self.config.height = h;
        let encode_res = self.encode_passes(&mut encoder, &view, p, &prepared);
        self.config.width = saved_w;
        self.config.height = saved_h;
        encode_res?;

        // Compute cropped physical `stage_rect` bounds (fallback to full viewport if degenerate).
        let (sx, sy, sw, sh) = if p.stage_rect[2] > 16.0 && p.stage_rect[3] > 16.0 {
            let x = ((p.stage_rect[0] * dpr).round().max(0.0) as u32).min(w.saturating_sub(1));
            let y = ((p.stage_rect[1] * dpr).round().max(0.0) as u32).min(h.saturating_sub(1));
            let cw = ((p.stage_rect[2] * dpr).round().max(1.0) as u32).min(w - x);
            let ch = ((p.stage_rect[3] * dpr).round().max(1.0) as u32).min(h - y);
            (x, y, cw.max(1), ch.max(1))
        } else {
            (0, 0, w, h)
        };

        let unpadded_bpr = sw * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bpr = unpadded_bpr.div_ceil(align) * align;
        let buf_size = (padded_bpr as u64) * (sh as u64);
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stage_capture_staging"),
            size: buf_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: sx, y: sy, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(sh),
                },
            },
            wgpu::Extent3d {
                width: sw,
                height: sh,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        let _ = self.device.poll(wgpu::PollType::Wait);
        rx.recv()
            .map_err(|_| rerr("capture readback channel closed"))?
            .map_err(|e| rerr(format!("capture map_async: {e}")))?;

        let mapped = slice.get_mapped_range();
        let is_bgra = matches!(
            self.format,
            wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Bgra8Unorm
        );
        let mut out = vec![0u8; (sw as usize) * (sh as usize) * 4];
        for row in 0..sh as usize {
            let src_off = row * padded_bpr as usize;
            let dst_off = row * unpadded_bpr as usize;
            let src = &mapped[src_off..src_off + unpadded_bpr as usize];
            let dst = &mut out[dst_off..dst_off + unpadded_bpr as usize];
            if is_bgra {
                for (s, d) in src
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(dst.as_chunks_mut::<4>().0)
                {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            } else {
                dst.copy_from_slice(src);
                for d in dst.as_chunks_mut::<4>().0 {
                    d[3] = 255;
                }
            }
        }
        drop(mapped);
        staging.unmap();
        Ok((sw, sh, out))
    }

    fn prepare_render_passes(&mut self, p: &DrawPacket, dpr: f32) -> Result<PreparedFrame> {
        let has_transition = !p.transition_from.is_empty() || !p.transition_to.is_empty();
        if !has_transition {
            // Keep `self.scratch` GPU textures pooled across transitions; only clear cache signatures!
            self.scratch_from_sig = None;
            self.scratch_to_sig = None;
        }

        // 1. Prepare text glyphs via glyphon — skip prepare completely when text signature is unchanged!
        let cur_vp = [self.config.width, self.config.height];
        if cur_vp != self.last_viewport {
            self.viewport.update(
                &self.queue,
                Resolution {
                    width: cur_vp[0],
                    height: cur_vp[1],
                },
            );
            self.last_viewport = cur_vp;
            self.last_text_sig = None;
        }
        let text_sig = hash_text_runs(&p.texts, dpr, cur_vp, self.text_font_epoch);
        let mut ran_text_prepare = false;
        if self.last_text_sig != Some(text_sig) {
            ran_text_prepare = true;
            self.text.layout_texts(&p.texts);

            // Compute positioned ruby annotations directly from shaped base glyph boxes!
            let mut ruby_runs: Vec<TextRun> = Vec::new();
            for r in &p.texts {
                if r.ruby_spans.is_empty() {
                    continue;
                }
                let Some(entry) = self.text.buffers.get(&r.cache_key) else {
                    continue;
                };
                let y_draw = if r.valign_center && entry.content_height > 0.0 {
                    r.y + ((r.height - entry.content_height) * 0.5).max(0.0)
                } else {
                    r.y
                };
                let ruby_sz = (entry.effective_size * 0.44).clamp(8.0, 15.0);
                let offsets = &entry.line_offsets;
                for (b_start, b_end, ruby_text) in &r.ruby_spans {
                    if ruby_text.is_empty() {
                        continue;
                    }
                    if let Some(vis) = r.visible_bytes {
                        if *b_start >= vis {
                            continue;
                        }
                    }
                    let mut min_x = f32::INFINITY;
                    let mut max_x = f32::NEG_INFINITY;
                    let mut line_y = None;
                    for run in entry.buffer.layout_runs() {
                        let base = *offsets.get(run.line_i).unwrap_or(&0);
                        for g in run.glyphs {
                            let gs = base + g.start;
                            let ge = base + g.end;
                            if ge > *b_start && gs < *b_end {
                                if line_y.is_none() {
                                    line_y = Some(y_draw + run.line_top - r.scroll);
                                }
                                min_x = min_x.min(r.x + g.x);
                                max_x = max_x.max(r.x + g.x + g.w);
                            }
                        }
                        if line_y.is_some() {
                            break;
                        }
                    }
                    if let Some(ly) = line_y {
                        if max_x > min_x {
                            let span_w = max_x - min_x;
                            let ruby_w = (ruby_text.chars().count().max(1) as f32 * ruby_sz * 1.15)
                                .max(span_w + 8.0);
                            let rx = min_x + (span_w - ruby_w) * 0.5;
                            let ry = ly - ruby_sz * 0.92;
                            let mut rr = TextRun {
                                text: ruby_text.clone(),
                                visible_bytes: None,
                                x: rx,
                                y: ry,
                                width: ruby_w,
                                height: ruby_sz * 1.35,
                                size: ruby_sz,
                                line_height: ruby_sz * 1.18,
                                color: r.color,
                                emphasis: Vec::new(),
                                color_spans: Vec::new(),
                                ruby_spans: Vec::new(),
                                shadow: r.shadow,
                                outline: r.outline,
                                scroll: 0.0,
                                clip: r
                                    .clip
                                    .map(|[cx, cy, cw, ch]| [cx, cy - ruby_sz, cw, ch + ruby_sz]),
                                font_assets: r.font_assets.clone(),
                                align: nir_ir::TextAlign::Center,
                                valign_center: false,
                                auto_fit: false,
                                inline_images: Vec::new(),
                                cache_key: 0,
                            };
                            rr.compute_key();
                            ruby_runs.push(rr);
                        }
                    }
                }
            }
            if !ruby_runs.is_empty() {
                self.text.layout_texts(&ruby_runs);
            }

            let mut areas = Vec::with_capacity((p.texts.len() + ruby_runs.len()) * 5);
            for r in p.texts.iter().chain(ruby_runs.iter()) {
                let sk = r.shadow_cache_key();
                let has_outline = r.outline.is_some_and(|o| o[3] > 0.01);
                if let Some(s) = r.shadow {
                    // When 4-way outline is active and shadow offset is tiny, the outline already
                    // provides crisp dark separation; skip redundant 6th TextArea!
                    let shadow_redundant =
                        has_outline && s.offset[0].abs() <= 1.25 && s.offset[1].abs() <= 1.5;
                    if !shadow_redundant {
                        if let Some(entry) = self.text.buffers.get(&sk) {
                            let scol =
                                [s.color[0], s.color[1], s.color[2], s.color[3] * r.color[3]];
                            Self::push_text_areas(
                                &mut areas,
                                entry,
                                r,
                                r.x + s.offset[0],
                                r.y + s.offset[1],
                                scol,
                                dpr,
                            );
                        }
                    }
                }
                if let Some(ocol) = r.outline {
                    if ocol[3] > 0.01 {
                        if let Some(entry) = self.text.buffers.get(&sk) {
                            let off = (entry.effective_size * 0.044).clamp(0.8, 1.35);
                            let oc = [ocol[0], ocol[1], ocol[2], ocol[3] * r.color[3]];
                            for (dx, dy) in [(-off, 0.0), (off, 0.0), (0.0, -off), (0.0, off)] {
                                Self::push_text_areas(
                                    &mut areas,
                                    entry,
                                    r,
                                    r.x + dx,
                                    r.y + dy,
                                    oc,
                                    dpr,
                                );
                            }
                        }
                    }
                }
                if let Some(entry) = self.text.buffers.get(&r.cache_key) {
                    Self::push_text_areas(&mut areas, entry, r, r.x, r.y, r.color, dpr);
                }
            }
            self.text_renderer
                .prepare(
                    &self.device,
                    &self.queue,
                    &mut self.text.fonts,
                    &mut self.atlas,
                    &self.viewport,
                    areas,
                    &mut self.swash,
                )
                .map_err(|e| rerr(e.to_string()))?;

            self.cached_inline_quads.clear();
            for r in &p.texts {
                if r.inline_images.is_empty() {
                    continue;
                }
                if let Some(entry) = self.text.buffers.get(&r.cache_key) {
                    let y_draw = if r.valign_center && entry.content_height > 0.0 {
                        r.y + ((r.height - entry.content_height) * 0.5).max(0.0)
                    } else {
                        r.y
                    };
                    let icon_sz = entry.effective_size.max(8.0);
                    let offsets = &entry.line_offsets;
                    for &(byte_off, asset) in &r.inline_images {
                        if let Some(vis) = r.visible_bytes {
                            if byte_off >= vis {
                                continue;
                            }
                        }
                        'find_glyph: for run in entry.buffer.layout_runs() {
                            let base = *offsets.get(run.line_i).unwrap_or(&0);
                            for g in run.glyphs {
                                if base + g.start == byte_off {
                                    let w = g.w.max(icon_sz);
                                    let gx = r.x + g.x + (w - icon_sz) * 0.5;
                                    let gy = y_draw + run.line_top - r.scroll
                                        + (run.line_height - icon_sz) * 0.5;
                                    let mut q = Quad::image([gx, gy, icon_sz, icon_sz], asset);
                                    q.color[3] = r.color[3];
                                    q.clip = r.clip;
                                    self.cached_inline_quads.push(q);
                                    break 'find_glyph;
                                }
                            }
                        }
                    }
                }
            }
            self.last_text_sig = Some(text_sig);
            self.text_prepares = self.text_prepares.wrapping_add(1);
        }
        let inline_quads = std::mem::take(&mut self.cached_inline_quads);

        // 2. Build indexed vertices (4 vertices per quad; supports smooth gradients, SDF borders & CAS sharpening!)
        let mut verts = std::mem::take(&mut self.scratch_verts);
        verts.clear();
        let premul = |c: [f32; 4]| -> [f32; 4] {
            let a = c[3];
            [linear(c[0]) * a, linear(c[1]) * a, linear(c[2]) * a, a]
        };
        let [s0, s1] = p.stage_quad_range;
        let stage_slice = if s0 < s1 && s1 <= p.quads.len() {
            &p.quads[s0..s1]
        } else {
            &[][..]
        };
        let has_blur_target = p.needs_blur
            && !stage_slice.is_empty()
            && p.stage_rect[2] > 2.0
            && p.stage_rect[3] > 2.0;

        let append_in =
            |verts: &mut Vec<Vertex>, quads: &[Quad], ox: f32, oy: f32, vw: f32, vh: f32| {
                let inv_w = 2.0 / vw.max(1.0);
                let inv_h = 2.0 / vh.max(1.0);
                let local_uvs = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
                for q in quads {
                    let [x, y, w, h] = q.rect;
                    let (c0, c1) = if q.special == SpecialQuad::StageTransition {
                        let sp = p.transition.shader_params(p.transition_progress);
                        (sp, sp)
                    } else {
                        let start = premul(q.color);
                        let end = q.color_end.map_or(start, premul);
                        (start, end)
                    };
                    let vcols = if q.gradient_horizontal {
                        [c0, c1, c0, c1]
                    } else {
                        [c0, c0, c1, c1]
                    };
                    let corners =
                        q.corners
                            .unwrap_or([[x, y], [x + w, y], [x, y + h], [x + w, y + h]]);
                    let [u0, v0, u1, v1] = q.uv;
                    let uvs = [[u0, v0], [u1, v0], [u0, v1], [u1, v1]];
                    let rect_params = if q.corner_radius > 0.0
                        || q.border_width > 0.0
                        || q.special == SpecialQuad::SoftShadow
                        || q.glass_rim > 0.01
                    {
                        [w.max(1.0), h.max(1.0), q.corner_radius, q.border_width]
                    } else {
                        [w.max(1.0), h.max(1.0), 0.0, 0.0]
                    };
                    let (fx_param, fx_mode) = match q.special {
                        SpecialQuad::SoftShadow => (q.shadow_blur.max(3.0), 1.0),
                        SpecialQuad::FrostedPanel if has_blur_target => (q.glass_rim, 2.0),
                        _ => (q.glass_rim, 0.0),
                    };
                    let border_color = if q.border_width > 0.0 && q.border_color[3] > 0.0 {
                        premul(q.border_color)
                    } else if q.sharpen > 0.01
                        && q.asset.is_some()
                        && q.special == SpecialQuad::None
                    {
                        // Sentinel border_color.a < -0.5 enables CAS-lite edge sharpening in quad.wgsl!
                        [q.sharpen.clamp(0.0, 1.0), 0.0, 0.0, -1.0]
                    } else {
                        [0.0; 4]
                    };
                    for idx in 0..4 {
                        verts.push(Vertex {
                            pos: [
                                (corners[idx][0] - ox) * inv_w - 1.0,
                                1.0 - (corners[idx][1] - oy) * inv_h,
                            ],
                            uv: uvs[idx],
                            color: vcols[idx],
                            rect_params,
                            border_color,
                            extra: [local_uvs[idx][0], local_uvs[idx][1], fx_param, fx_mode],
                        });
                    }
                }
            };
        append_in(&mut verts, &p.quads, 0.0, 0.0, p.width, p.height);
        let inline_quad_start = verts.len() / 4;
        if !inline_quads.is_empty() {
            append_in(&mut verts, &inline_quads, 0.0, 0.0, p.width, p.height);
        }

        // Determine whether transition offscreen passes need re-rendering (skip static source/target!)
        let mut render_from = false;
        let mut render_to = false;
        let mut tr_from_quad_start = verts.len() / 4;
        let mut tr_to_quad_start = tr_from_quad_start;
        if has_transition {
            let [sw, sh] = p.stage_size;
            if sw > 0 && sh > 0 {
                let need_alloc = !self
                    .scratch
                    .as_ref()
                    .is_some_and(|(_, _, w, h)| *w == sw && *h == sh);
                if need_alloc {
                    self.scratch = Some((self.offscreen(sw, sh), self.offscreen(sw, sh), sw, sh));
                    self.scratch_from_sig = None;
                    self.scratch_to_sig = None;
                }
                let from_sig =
                    hash_transition_quads(&p.transition_from, sw, sh, self.texture_epoch);
                let to_sig = hash_transition_quads(&p.transition_to, sw, sh, self.texture_epoch);
                if self.scratch_from_sig != Some(from_sig) {
                    render_from = true;
                    tr_from_quad_start = verts.len() / 4;
                    append_in(
                        &mut verts,
                        &p.transition_from,
                        0.0,
                        0.0,
                        sw as f32,
                        sh as f32,
                    );
                    self.scratch_from_sig = Some(from_sig);
                }
                if self.scratch_to_sig != Some(to_sig) {
                    render_to = true;
                    tr_to_quad_start = verts.len() / 4;
                    append_in(&mut verts, &p.transition_to, 0.0, 0.0, sw as f32, sh as f32);
                    self.scratch_to_sig = Some(to_sig);
                }
            }
        }

        // Optional 1/8-res 2-Pass Kawase Blur of the pure stage for AmbientBackdrop, FrostedBackdrop & FrostedPanel!
        // Cached by stage quad signature so static scenes pay 0 blur passes!
        let mut run_blur = false;
        let mut blur_stage_quad_start = verts.len() / 4;
        let mut blur_pass1_quad = 0usize;
        let mut blur_pass2_quad = 0usize;
        if has_blur_target {
            let bw = (p.stage_size[0].max(160) / 6).clamp(112, 224);
            let bh = (p.stage_size[1].max(90) / 6).clamp(64, 150);
            let need_alloc = !self
                .blur_pair
                .as_ref()
                .is_some_and(|(_, _, w, h)| *w == bw && *h == bh);
            if need_alloc {
                self.blur_pair = Some((self.offscreen(bw, bh), self.offscreen(bw, bh), bw, bh));
                self.blur_stage_sig = None;
            }
            let mut bsig = hash_transition_quads(stage_slice, bw, bh, self.texture_epoch);
            if has_transition {
                let qprog = (p.transition_progress * 10.0).round() as u64;
                bsig ^= qprog.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            }
            if self.blur_stage_sig != Some(bsig) {
                run_blur = true;
                blur_stage_quad_start = verts.len() / 4;
                let [srx, sry, srw, srh] = p.stage_rect;
                append_in(&mut verts, stage_slice, srx, sry, srw, srh);

                // Fullscreen quad for Kawase pass 1 (radius 1.45 texels) & pass 2 (radius 2.65 texels)
                let push_fs_blur = |verts: &mut Vec<Vertex>, radius: f32| -> usize {
                    let qidx = verts.len() / 4;
                    let rp = [radius, radius, 0.0, 0.0];
                    for (pos, uv) in [
                        ([-1.0, 1.0], [0.0, 0.0]),
                        ([1.0, 1.0], [1.0, 0.0]),
                        ([-1.0, -1.0], [0.0, 1.0]),
                        ([1.0, -1.0], [1.0, 1.0]),
                    ] {
                        verts.push(Vertex {
                            pos,
                            uv,
                            color: [1.0; 4],
                            rect_params: rp,
                            border_color: [0.0; 4],
                            extra: [uv[0], uv[1], 0.0, 0.0],
                        });
                    }
                    qidx
                };
                blur_pass1_quad = push_fs_blur(&mut verts, 1.45);
                blur_pass2_quad = push_fs_blur(&mut verts, 2.65);
                self.blur_stage_sig = Some(bsig);
            }
        }

        let needed_quads = verts.len() / 4;
        if needed_quads > self.quad_capacity {
            let next_quads = needed_quads.next_power_of_two().min(65_536);
            self.vertices = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quad_verts"),
                size: (next_quads * 4 * std::mem::size_of::<Vertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let idx_data = build_quad_indices(next_quads);
            self.indices = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quad_indices"),
                size: (idx_data.len() * std::mem::size_of::<u32>()) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue
                .write_buffer(&self.indices, 0, bytemuck::cast_slice(&idx_data));
            self.quad_capacity = next_quads;
        }
        if !verts.is_empty() {
            self.queue
                .write_buffer(&self.vertices, 0, bytemuck::cast_slice(&verts));
        }
        self.scratch_verts = verts;

        // Touch LRU timestamps for active textures.
        self.use_clock = self.use_clock.wrapping_add(1);
        let now_tick = self.use_clock;
        for q in p
            .quads
            .iter()
            .chain(inline_quads.iter())
            .chain(p.transition_from.iter())
            .chain(p.transition_to.iter())
        {
            if let Some(id) = q.asset {
                if let Some(tex) = self.textures.get_mut(&id) {
                    tex.last_used = now_tick;
                }
            }
        }

        self.cached_inline_quads = inline_quads;
        Ok(PreparedFrame {
            ran_text_prepare,
            inline_quad_start,
            render_from,
            render_to,
            tr_from_quad_start,
            tr_to_quad_start,
            run_blur,
            blur_stage_quad_start,
            blur_pass1_quad,
            blur_pass2_quad,
        })
    }

    fn encode_passes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        p: &DrawPacket,
        prep: &PreparedFrame,
    ) -> Result<()> {
        let [s0, s1] = p.stage_quad_range;
        let stage_slice = if s0 < s1 && s1 <= p.quads.len() {
            &p.quads[s0..s1]
        } else {
            &[][..]
        };

        // 4. Optional offscreen render passes for stage transition (only when side changed!)
        if prep.render_from || prep.render_to {
            if let Some((src, dst, sw, sh)) = &self.scratch {
                for (need, target, quads, qstart) in [
                    (
                        prep.render_from,
                        src,
                        &p.transition_from,
                        prep.tr_from_quad_start,
                    ),
                    (prep.render_to, dst, &p.transition_to, prep.tr_to_quad_start),
                ] {
                    if !need {
                        continue;
                    }
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("transition_side"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target.view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    self.paint_quads(
                        &mut pass,
                        quads,
                        qstart,
                        [0.0, 0.0],
                        *sw as f32,
                        *sh as f32,
                        [*sw, *sh],
                        p,
                    );
                }
            }
        }

        // 4b. Optional 1/8-res 2-Pass Kawase Blur of pure stage (cached across static frames!)
        if prep.run_blur {
            if let Some((blur_a, blur_b, bw, bh)) = &self.blur_pair {
                // Step 0: Downsample pure stage quads into `blur_a`
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("blur_stage_capture"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &blur_a.view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.015,
                                    g: 0.020,
                                    b: 0.032,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    self.paint_quads(
                        &mut pass,
                        stage_slice,
                        prep.blur_stage_quad_start,
                        [p.stage_rect[0], p.stage_rect[1]],
                        p.stage_rect[2],
                        p.stage_rect[3],
                        [*bw, *bh],
                        p,
                    );
                }
                // Step 1: Kawase blur pass 1 (blur_a -> blur_b)
                for (src_tex, dst_tex, qidx) in [
                    (blur_a, blur_b, prep.blur_pass1_quad),
                    (blur_b, blur_a, prep.blur_pass2_quad),
                ] {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("kawase_blur_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &dst_tex.view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    pass.set_pipeline(&self.blur_pipeline);
                    pass.set_bind_group(0, &src_tex.bind, &[]);
                    pass.set_vertex_buffer(0, self.vertices.slice(..));
                    pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
                    let is = (qidx * 6) as u32;
                    pass.draw_indexed(is..(is + 6), 0, 0..1);
                }
            }
        }

        // 5. Main target pass
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.008,
                            g: 0.011,
                            b: 0.018,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let phys = [self.config.width, self.config.height];
            self.paint_quads(
                &mut pass,
                &p.quads,
                0,
                [0.0, 0.0],
                p.width,
                p.height,
                phys,
                p,
            );
            if !self.cached_inline_quads.is_empty() {
                self.paint_quads(
                    &mut pass,
                    &self.cached_inline_quads,
                    prep.inline_quad_start,
                    [0.0, 0.0],
                    p.width,
                    p.height,
                    phys,
                    p,
                );
            }
            pass.set_scissor_rect(0, 0, phys[0], phys[1]);
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .map_err(|e| rerr(e.to_string()))?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_quads(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        quads: &[Quad],
        quad_offset: usize,
        origin: [f32; 2],
        vw: f32,
        vh: f32,
        physical: [u32; 2],
        pkt: &DrawPacket,
    ) {
        if quads.is_empty() {
            return;
        }
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        let sx = physical[0] as f32 / vw.max(1.0);
        let sy = physical[1] as f32 / vh.max(1.0);

        // Batch consecutive quads sharing (asset, pipeline_id, scissor) into 1 indexed draw call,
        // routing flat/unrounded Normal quads to the lightweight `fs_plain` pipeline!
        type QuadBatch = (Option<AssetId>, u8, [u32; 4], u32, u32);
        let mut batch: Option<QuadBatch> = None;
        let mut cur_pipeline: Option<u8> = None; // 0..3 = styled blend, 4 = mix, 5 = fs_plain
        let mut cur_scissor: Option<[u32; 4]> = None;

        let flush = |pass: &mut wgpu::RenderPass<'_>, b: &mut Option<QuadBatch>| {
            if let Some((_, _, _, s, e)) = b.take() {
                pass.draw_indexed(s..e, 0, 0..1);
            }
        };

        for (i, q) in quads.iter().enumerate() {
            let clip = q.clip.unwrap_or([origin[0], origin[1], vw, vh]);
            let x = ((clip[0] - origin[0]) * sx).max(0.0) as u32;
            let y = ((clip[1] - origin[1]) * sy).max(0.0) as u32;
            let r = (((clip[0] - origin[0]) + clip[2]) * sx).clamp(0.0, physical[0] as f32) as u32;
            let b = (((clip[1] - origin[1]) + clip[3]) * sy).clamp(0.0, physical[1] as f32) as u32;
            if r <= x || b <= y {
                flush(pass, &mut batch);
                continue;
            }
            let scissor = [x, y, r - x, b - y];
            let is = ((quad_offset + i) * 6) as u32;
            let ie = is + 6;

            if q.special == SpecialQuad::StageTransition {
                flush(pass, &mut batch);
                if let Some((src, dst, _, _)) = &self.scratch {
                    if cur_pipeline != Some(4) {
                        pass.set_pipeline(&self.mix_pipeline);
                        cur_pipeline = Some(4);
                    }
                    if cur_scissor != Some(scissor) {
                        pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
                        cur_scissor = Some(scissor);
                    }
                    pass.set_bind_group(0, &src.bind, &[]);
                    pass.set_bind_group(1, &dst.bind, &[]);
                    let mask = pkt
                        .transition
                        .mask_asset()
                        .and_then(|id| self.textures.get(&id))
                        .unwrap_or(&self.white_tex);
                    pass.set_bind_group(2, &mask.bind, &[]);
                    pass.draw_indexed(is..ie, 0, 0..1);
                }
                continue;
            }

            if matches!(
                q.special,
                SpecialQuad::AmbientBackdrop
                    | SpecialQuad::FrostedBackdrop
                    | SpecialQuad::FrostedPanel
            ) {
                flush(pass, &mut batch);
                let bind = if let Some((blur_a, _, _, _)) = &self.blur_pair {
                    &blur_a.bind
                } else if q.special == SpecialQuad::FrostedPanel {
                    &self.white_tex.bind
                } else {
                    continue;
                };
                let pl_id = q.blend as u8;
                if cur_pipeline != Some(pl_id) {
                    pass.set_pipeline(&self.pipelines[q.blend as usize]);
                    cur_pipeline = Some(pl_id);
                }
                if cur_scissor != Some(scissor) {
                    pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
                    cur_scissor = Some(scissor);
                }
                pass.set_bind_group(0, bind, &[]);
                pass.draw_indexed(is..ie, 0, 0..1);
                continue;
            }

            let is_plain = q.blend == BlendMode::Normal
                && q.special == SpecialQuad::None
                && q.corner_radius <= 0.0
                && q.border_width <= 0.0
                && q.sharpen <= 0.01
                && q.glass_rim <= 0.01;
            let pl_id = if is_plain { 5u8 } else { q.blend as u8 };

            if let Some((ba, bp, bs, _, bend)) = batch.as_mut() {
                if *ba == q.asset && *bp == pl_id && *bs == scissor && *bend == is {
                    *bend = ie;
                    continue;
                }
            }
            flush(pass, &mut batch);

            let tex = match q.asset {
                Some(id) => match self.textures.get(&id) {
                    Some(t) => t,
                    None => continue, // Asset still loading asynchronously; skip gracefully!
                },
                None => &self.white_tex,
            };
            if cur_pipeline != Some(pl_id) {
                if is_plain {
                    pass.set_pipeline(&self.plain_pipeline);
                } else {
                    pass.set_pipeline(&self.pipelines[q.blend as usize]);
                }
                cur_pipeline = Some(pl_id);
            }
            if cur_scissor != Some(scissor) {
                pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
                cur_scissor = Some(scissor);
            }
            pass.set_bind_group(0, &tex.bind, &[]);
            batch = Some((q.asset, pl_id, scissor, is, ie));
        }
        flush(pass, &mut batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiply_lut_handles_edges_and_alpha() {
        let mut px = vec![200, 100, 50, 128, 255, 255, 255, 0, 10, 20, 30, 255];
        premultiply_rgba(&mut px);
        assert_eq!(&px[4..8], &[0, 0, 0, 0]);
        assert_eq!(&px[8..12], &[10, 20, 30, 255]);
        assert!(px[0] < 200 && px[0] > 100);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn decode_image_png_and_webp_roundtrip() {
        let mut img = image::RgbaImage::new(4, 3);
        for p in img.pixels_mut() {
            *p = image::Rgba([180, 90, 45, 200]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let (w, h, rgba) = decode_image(buf.get_ref()).unwrap();
        assert_eq!((w, h), (4, 3));
        assert_eq!(rgba.len(), 4 * 3 * 4);

        let webp_bytes = webp::Encoder::from_rgba(img.as_raw(), 4, 3).encode_lossless();
        let (ww, wh, wrgba) = decode_image(&webp_bytes).unwrap();
        assert_eq!((ww, wh), (4, 3));
        assert_eq!(wrgba.len(), 4 * 3 * 4);
    }

    #[test]
    fn composite_delta_handles_opaque_and_tile_masked_alpha_deletion() {
        // 32x32 base filled with opaque blue
        let mut base = vec![0u8; 32 * 32 * 4];
        for px in base.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[20, 40, 220, 255]);
        }
        // 1) Opaque CG mode (empty tile_mask): 16x16 patch at (16, 0) where left half is transparent
        //    (keep base) and right half is opaque red (overwrite base).
        let mut cg_patch = vec![0u8; 16 * 16 * 4];
        for y in 0..16 {
            for x in 8..16 {
                let o = (y * 16 + x) * 4;
                cg_patch[o..o + 4].copy_from_slice(&[240, 10, 10, 255]);
            }
        }
        let cg_delta = nir_ir::ImageDelta {
            base: AssetId(0),
            x: 16,
            y: 0,
            tile_mask: vec![],
        };
        let out_cg = composite_delta_rgba(&base, 32, 32, &cg_patch, 16, 16, &cg_delta).unwrap();
        // (16, 0) kept blue base
        assert_eq!(&out_cg[16 * 4..16 * 4 + 4], &[20, 40, 220, 255]);
        // (24, 0) overwritten with red
        assert_eq!(&out_cg[24 * 4..24 * 4 + 4], &[240, 10, 10, 255]);

        // 2) Sprite tile-mask mode: 32x16 patch at (0, 16), tile 0 active (deletes alpha -> 0), tile 1 inactive (keeps base)
        let mut sp_patch = vec![0u8; 32 * 16 * 4];
        // Put opaque green inside inactive tile 1 to prove tile_mask skips it!
        for y in 0..16 {
            for x in 16..32 {
                let o = (y * 32 + x) * 4;
                sp_patch[o..o + 4].copy_from_slice(&[0, 255, 0, 255]);
            }
        }
        let sp_delta = nir_ir::ImageDelta {
            base: AssetId(0),
            x: 0,
            y: 16,
            tile_mask: vec![0b0000_0001], // bit 0 active, bit 1 inactive
        };
        let out_sp = composite_delta_rgba(&base, 32, 32, &sp_patch, 32, 16, &sp_delta).unwrap();
        // Active tile (0, 16) replaced by transparent [0,0,0,0] -> alpha deletion preserved!
        let p_del = (16 * 32) * 4;
        assert_eq!(&out_sp[p_del..p_del + 4], &[0, 0, 0, 0]);
        // Inactive tile (20, 16) kept original blue base!
        let p_keep = (16 * 32 + 20) * 4;
        assert_eq!(&out_sp[p_keep..p_keep + 4], &[20, 40, 220, 255]);
    }

    #[test]
    fn smart_mip_level_policy_skips_1to1_and_caps_downscaled() {
        // 800x600 CG on 1280x720 desktop -> 1 mip (0 wasted VRAM)
        assert_eq!(
            mip_level_count_for(800, 600, 1280, 720, RendererBackend::Vulkan),
            1
        );
        // 2560x1440 CG on 1280x720 desktop -> downscaled, capped at 3 mips
        assert_eq!(
            mip_level_count_for(2560, 1440, 1280, 720, RendererBackend::Vulkan),
            3
        );
        // WebGL2 always uses 1 mip
        assert_eq!(
            mip_level_count_for(2560, 1440, 1280, 720, RendererBackend::WebGl2),
            1
        );
    }

    #[test]
    fn parse_and_cpu_decode_zlib_ktx2_bc1() {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;
        // 1 single 4x4 BC1 block: solid bright red in RGB565 (0xF800, 0xF800, selector=0)
        let block = [0x00u8, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(&block).unwrap();
        let zdata = z.finish().unwrap();

        let mut dfd = Vec::new();
        dfd.extend_from_slice(&44u32.to_le_bytes());
        dfd.extend_from_slice(&0u32.to_le_bytes());
        dfd.extend_from_slice(&2u16.to_le_bytes());
        dfd.extend_from_slice(&40u16.to_le_bytes());
        dfd.extend_from_slice(&[128, 1, 2, 0, 3, 3, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0]);
        dfd.extend_from_slice(&0u16.to_le_bytes());
        dfd.extend_from_slice(&[63, 0, 0, 0, 0, 0]);
        dfd.extend_from_slice(&0u32.to_le_bytes());
        dfd.extend_from_slice(&u32::MAX.to_le_bytes());

        let mut ktx = Vec::new();
        ktx.extend_from_slice(&[
            0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
        ]);
        ktx.extend_from_slice(&134u32.to_le_bytes()); // BC1_RGBA_SRGB_BLOCK
        ktx.extend_from_slice(&1u32.to_le_bytes());
        ktx.extend_from_slice(&4u32.to_le_bytes());
        ktx.extend_from_slice(&4u32.to_le_bytes());
        ktx.extend_from_slice(&0u32.to_le_bytes());
        ktx.extend_from_slice(&0u32.to_le_bytes());
        ktx.extend_from_slice(&1u32.to_le_bytes());
        ktx.extend_from_slice(&1u32.to_le_bytes());
        ktx.extend_from_slice(&3u32.to_le_bytes()); // ZLIB
        ktx.extend_from_slice(&104u32.to_le_bytes());
        ktx.extend_from_slice(&(dfd.len() as u32).to_le_bytes());
        ktx.extend_from_slice(&0u32.to_le_bytes());
        ktx.extend_from_slice(&0u32.to_le_bytes());
        ktx.extend_from_slice(&0u64.to_le_bytes());
        ktx.extend_from_slice(&0u64.to_le_bytes());
        let level_off = 104u64 + dfd.len() as u64;
        let pad = ((16 - (level_off % 16)) % 16) as usize;
        ktx.extend_from_slice(&(level_off + pad as u64).to_le_bytes());
        ktx.extend_from_slice(&(zdata.len() as u64).to_le_bytes());
        ktx.extend_from_slice(&(block.len() as u64).to_le_bytes());
        ktx.extend_from_slice(&dfd);
        ktx.resize(ktx.len() + pad, 0);
        ktx.extend_from_slice(&zdata);

        let parsed = parse_ktx2_level0(&ktx).unwrap();
        assert_eq!((parsed.width, parsed.height), (4, 4));
        assert_eq!(parsed.format, Ktx2BlockFormat::Bc1Srgb);
        let rgba = decode_ktx2_to_rgba_premultiplied(&parsed).unwrap();
        assert_eq!(rgba.len(), 4 * 4 * 4);
        assert!(rgba[0] > 240 && rgba[1] < 10 && rgba[2] < 10 && rgba[3] == 255);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn thumbnail_webp_png_and_icon_helpers_work() {
        let mut rgba = vec![0u8; 64 * 48 * 4];
        for px in rgba.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[90, 150, 230, 255]);
        }
        let webp = encode_thumbnail_webp(&rgba, 64, 48, 160, 90).unwrap();
        assert!(webp.len() > 12 && &webp[0..4] == b"RIFF" && &webp[8..12] == b"WEBP");
        let (tw, th, _) = decode_image(&webp).unwrap();
        assert_eq!((tw, th), (160, 90));

        let png = encode_png_rgba(&rgba, 64, 48).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let (pw, ph, _) = decode_image(&png).unwrap();
        assert_eq!((pw, ph), (64, 48));

        let icon = resize_rgba_icon(&rgba, 64, 48, 32).unwrap();
        assert_eq!(icon.len(), 32 * 32 * 4);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn quad_wgsl_compiles_cleanly_on_wgpu_validator() {
        use std::future::Future;
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};

        fn poll_sync<F: Future>(fut: F) -> F::Output {
            let mut fut = pin!(fut);
            let waker = Waker::noop();
            let mut cx = Context::from_waker(waker);
            loop {
                if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                    return v;
                }
                std::thread::yield_now();
            }
        }

        if wgpu::Instance::enabled_backend_features().is_empty() {
            return;
        }
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = poll_sync(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .or_else(|_| {
            poll_sync(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
        });
        let Ok(adapter) = adapter else {
            return;
        };
        let Ok((device, _queue)) = poll_sync(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("wgsl_validate"),
            required_features: wgpu::Features::empty(),
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        })) else {
            return;
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nir_quad_validate"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });
        let vlayout = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4],
        }];
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        for entry in ["fs", "fs_blur"] {
            let _ = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &vlayout,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Bgra8UnormSrgb,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
        }
        let mix_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout, &layout, &layout],
            push_constant_ranges: &[],
        });
        let _ = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fs_mix"),
            layout: Some(&mix_pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &vlayout,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mix"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Bgra8UnormSrgb,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
    }
}
