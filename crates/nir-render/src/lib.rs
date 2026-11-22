//! High-performance ordered 2D wgpu compositor with linear premultiplied blending,
//! multi-blend pipelines (Normal/Add/Multiply/Screen), indexed quads, GPU mipmaps,
//! cached offscreen transitions, zero-clone glyphon text, SIMD WebP decode, and LRU GPU texture cache.
#![forbid(unsafe_code)]

use bytemuck::{Pod, Zeroable};
use glyphon::{
    Cache, Color, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use nir_ir::{AssetId, BlendMode, NirError, Result};
use nir_ui::{DrawPacket, Quad, SpecialQuad, TextEngine, TextRun};
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
pub use wgpu;

const DEFAULT_GPU_CACHE_LIMIT: u64 = 256 * 1024 * 1024;
const ATLAS_TRIM_INTERVAL: u64 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererBackend {
    Dx12,
    Metal,
    Vulkan,
    WebGpu,
    WebGl2,
}

impl RendererBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dx12 => "dx12",
            Self::Metal => "metal",
            Self::Vulkan => "vulkan",
            Self::WebGpu => "webgpu",
            Self::WebGl2 => "webgl2",
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

struct GpuTexture {
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    _texture: wgpu::Texture,
    bytes: u64,
    last_used: u64,
}

pub struct Renderer {
    _instance: wgpu::Instance,
    pub device: wgpu::Device,
    queue: wgpu::Queue,
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    pipelines: [wgpu::RenderPipeline; 4],
    mix_pipeline: wgpu::RenderPipeline,
    mip_pipeline: wgpu::RenderPipeline,
    mip_vertices: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white_tex: GpuTexture,
    textures: HashMap<AssetId, GpuTexture>,
    texture_bytes: u64,
    texture_limit: u64,
    texture_epoch: u64,
    use_clock: u64,
    scratch: Option<(GpuTexture, GpuTexture, u32, u32)>,
    scratch_from_sig: Option<u64>,
    scratch_to_sig: Option<u64>,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    scratch_verts: Vec<Vertex>,
    quad_capacity: usize,
    pub text: TextEngine,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    viewport: Viewport,
    last_viewport: [u32; 2],
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

fn mip_level_count_for(w: u32, h: u32, backend: RendererBackend) -> u32 {
    if backend == RendererBackend::WebGl2 || w.max(h) < 256 {
        return 1;
    }
    let max_dim = w.max(h);
    ((32 - max_dim.leading_zeros()).max(1)).min(5)
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
) -> Result<(wgpu::TextureFormat, wgpu::TextureFormat, Vec<wgpu::TextureFormat>)> {
    match backend {
        RendererBackend::WebGl2 => {
            let fmt = formats
                .iter()
                .copied()
                .find(wgpu::TextureFormat::is_srgb)
                .or_else(|| formats.first().copied())
                .ok_or_else(|| rerr("no surface format"))?;
            Ok((fmt, fmt, Vec::new()))
        }
        _ => {
            let surface_fmt = formats
                .iter()
                .copied()
                .find(|f| f.is_srgb() || f.add_srgb_suffix().is_srgb())
                .or_else(|| formats.first().copied())
                .ok_or_else(|| rerr("no surface format"))?;
            let fmt = surface_fmt.add_srgb_suffix();
            let views = if fmt == surface_fmt { vec![] } else { vec![fmt] };
            Ok((surface_fmt, fmt, views))
        }
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
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| rerr(e.to_string()))?;
        let info = adapter.get_info();
        let adapter_info = format!("{:?} / {} / {}", info.backend, info.name, info.driver);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("NIR v2 Renderer"),
                required_features: wgpu::Features::empty(),
                required_limits: match backend {
                    RendererBackend::WebGl2 => wgpu::Limits::downlevel_webgl2_defaults(),
                    _ => wgpu::Limits::downlevel_defaults(),
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
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
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

        let mip_quad = [
            Vertex {
                pos: [-1.0, 1.0],
                uv: [0.0, 0.0],
                color: [1.0; 4],
            },
            Vertex {
                pos: [1.0, 1.0],
                uv: [1.0, 0.0],
                color: [1.0; 4],
            },
            Vertex {
                pos: [-1.0, -1.0],
                uv: [0.0, 1.0],
                color: [1.0; 4],
            },
            Vertex {
                pos: [1.0, -1.0],
                uv: [1.0, 1.0],
                color: [1.0; 4],
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
        let text_renderer = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        Ok(Self {
            _instance: instance,
            device,
            queue,
            surface: Some(surface),
            config,
            format,
            pipelines,
            mix_pipeline,
            mip_pipeline,
            mip_vertices,
            layout,
            sampler,
            white_tex,
            textures: HashMap::new(),
            texture_bytes: 0,
            texture_limit: DEFAULT_GPU_CACHE_LIMIT,
            texture_epoch: 0,
            use_clock: 0,
            scratch: None,
            scratch_from_sig: None,
            scratch_to_sig: None,
            vertices,
            indices,
            scratch_verts: Vec::with_capacity(initial_quads * 4),
            quad_capacity: initial_quads,
            text: TextEngine::default(),
            atlas,
            text_renderer,
            viewport,
            last_viewport: [0, 0],
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
            .map_err(|e| NirError::new("E_FONT", "load_font", e))
    }

    pub fn set_texture_cache_limit(&mut self, bytes: u64) {
        self.texture_limit = bytes.max(32 * 1024 * 1024);
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
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
    }

    pub fn release_surface(&mut self) {
        self.surface = None;
    }

    pub fn rebind_surface(&mut self, surface: wgpu::Surface<'static>) {
        surface.configure(&self.device, &self.config);
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
        let (w, h, pixels) = decode_image(bytes)?;
        self.upload_rgba_premultiplied(id, w, h, &pixels)
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
        let mips = mip_level_count_for(w, h, self.backend);
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
        let mips = mip_level_count_for(w, h, self.backend);
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
                premultiplied_alpha: true,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        if mips > 1 {
            self.generate_mipmaps(&texture, mips);
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
        Ok(())
    }

    /// Graceful LRU eviction when VRAM exceeds `texture_limit`, preserving `pinned` active assets.
    pub fn evict_lru(&mut self, pinned: &BTreeSet<AssetId>) {
        if self.texture_bytes <= self.texture_limit {
            return;
        }
        let mut victims: Vec<(AssetId, u64)> = self
            .textures
            .iter()
            .filter(|(id, _)| !pinned.contains(id))
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
                    top: bounds.top.max(((y + run.line_top - r.scroll) * dpr) as i32),
                    right: bounds.right.min(((x + right + 1.0) * dpr) as i32),
                    bottom: bounds
                        .bottom
                        .min(((y + run.line_top + run.line_height - r.scroll) * dpr) as i32),
                };
                if row.bottom > row.top {
                    areas.push(TextArea {
                        buffer: buf,
                        left: x * dpr,
                        top: (y - r.scroll) * dpr,
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
                top: (y - r.scroll) * dpr,
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
        let has_transition = !p.transition_from.is_empty() || !p.transition_to.is_empty();
        if !has_transition {
            // Keep `self.scratch` GPU textures pooled across transitions; only clear cache signatures!
            self.scratch_from_sig = None;
            self.scratch_to_sig = None;
        }

        // 1. Prepare text glyphs via glyphon (zero TextRun cloning, cached line_offsets!)
        self.text.layout_texts(&p.texts);
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
        }
        let mut areas = Vec::with_capacity(p.texts.len() * 2);
        for r in &p.texts {
            if let Some(s) = r.shadow {
                let sk = r.shadow_cache_key();
                if let Some(entry) = self.text.buffers.get(&sk) {
                    let scol = [
                        s.color[0],
                        s.color[1],
                        s.color[2],
                        s.color[3] * r.color[3],
                    ];
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

        // 2. Build indexed vertices (4 vertices per quad instead of 6!)
        let mut verts = std::mem::take(&mut self.scratch_verts);
        verts.clear();
        let append = |verts: &mut Vec<Vertex>, quads: &[Quad], vw: f32, vh: f32| {
            let inv_w = 2.0 / vw.max(1.0);
            let inv_h = 2.0 / vh.max(1.0);
            for q in quads {
                let [x, y, w, h] = q.rect;
                let color = if q.special == SpecialQuad::StageTransition {
                    p.transition.shader_params(p.transition_progress)
                } else {
                    let a = q.color[3];
                    [
                        linear(q.color[0]) * a,
                        linear(q.color[1]) * a,
                        linear(q.color[2]) * a,
                        a,
                    ]
                };
                let corners = q
                    .corners
                    .unwrap_or([[x, y], [x + w, y], [x, y + h], [x + w, y + h]]);
                let [u0, v0, u1, v1] = q.uv;
                let uvs = [[u0, v0], [u1, v0], [u0, v1], [u1, v1]];
                for idx in 0..4 {
                    verts.push(Vertex {
                        pos: [corners[idx][0] * inv_w - 1.0, 1.0 - corners[idx][1] * inv_h],
                        uv: uvs[idx],
                        color,
                    });
                }
            }
        };
        append(&mut verts, &p.quads, p.width, p.height);

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
                    append(&mut verts, &p.transition_from, sw as f32, sh as f32);
                    self.scratch_from_sig = Some(from_sig);
                }
                if self.scratch_to_sig != Some(to_sig) {
                    render_to = true;
                    tr_to_quad_start = verts.len() / 4;
                    append(&mut verts, &p.transition_to, sw as f32, sh as f32);
                    self.scratch_to_sig = Some(to_sig);
                }
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
            .chain(p.transition_from.iter())
            .chain(p.transition_to.iter())
        {
            if let Some(id) = q.asset {
                if let Some(tex) = self.textures.get_mut(&id) {
                    tex.last_used = now_tick;
                }
            }
        }

        // 3. Acquire swapchain frame
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

        // 4. Optional offscreen render passes for stage transition (only when side changed!)
        if render_from || render_to {
            if let Some((src, dst, sw, sh)) = &self.scratch {
                for (need, target, quads, qstart) in [
                    (render_from, src, &p.transition_from, tr_from_quad_start),
                    (render_to, dst, &p.transition_to, tr_to_quad_start),
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
                        *sw as f32,
                        *sh as f32,
                        [*sw, *sh],
                        p,
                    );
                }
            }
        }

        // 5. Main swapchain pass
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            let phys = [self.config.width, self.config.height];
            self.paint_quads(&mut pass, &p.quads, 0, p.width, p.height, phys, p);
            pass.set_scissor_rect(0, 0, phys[0], phys[1]);
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .map_err(|e| rerr(e.to_string()))?;
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.submitted += 1;
        if self.submitted % ATLAS_TRIM_INTERVAL == 0 {
            self.atlas.trim();
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_quads(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        quads: &[Quad],
        quad_offset: usize,
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

        // Batch consecutive quads sharing (asset, blend, scissor) into 1 indexed draw call,
        // and track current pipeline/scissor to eliminate redundant GPU state binds!
        type QuadBatch = (Option<AssetId>, BlendMode, [u32; 4], u32, u32);
        let mut batch: Option<QuadBatch> = None;
        let mut cur_pipeline: Option<u8> = None; // 0..3 = blend pipelines, 4 = mix_pipeline
        let mut cur_scissor: Option<[u32; 4]> = None;

        let flush = |pass: &mut wgpu::RenderPass<'_>, b: &mut Option<QuadBatch>| {
            if let Some((_, _, _, s, e)) = b.take() {
                pass.draw_indexed(s..e, 0, 0..1);
            }
        };

        for (i, q) in quads.iter().enumerate() {
            let clip = q.clip.unwrap_or([0.0, 0.0, vw, vh]);
            let x = (clip[0] * sx).max(0.0) as u32;
            let y = (clip[1] * sy).max(0.0) as u32;
            let r = ((clip[0] + clip[2]) * sx).clamp(0.0, physical[0] as f32) as u32;
            let b = ((clip[1] + clip[3]) * sy).clamp(0.0, physical[1] as f32) as u32;
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

            if let Some((ba, bb, bs, _, bend)) = batch.as_mut() {
                if *ba == q.asset && *bb == q.blend && *bs == scissor && *bend == is {
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
            let pl_id = q.blend as u8;
            if cur_pipeline != Some(pl_id) {
                pass.set_pipeline(&self.pipelines[q.blend as usize]);
                cur_pipeline = Some(pl_id);
            }
            if cur_scissor != Some(scissor) {
                pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
                cur_scissor = Some(scissor);
            }
            pass.set_bind_group(0, &tex.bind, &[]);
            batch = Some((q.asset, q.blend, scissor, is, ie));
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
}
