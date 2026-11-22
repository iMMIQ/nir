//! High-performance ordered 2D wgpu compositor with linear premultiplied blending,
//! multi-blend pipelines (Normal/Add/Multiply/Screen), UV sub-rects, rotated quads,
//! rule-mask/wipe/dissolve transitions, glyphon text, and LRU GPU texture cache.
#![forbid(unsafe_code)]

use bytemuck::{Pod, Zeroable};
use glyphon::{
    Cache, Color, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use nir_ir::{AssetId, BlendMode, NirError, Result};
use nir_ui::{DrawPacket, Quad, SpecialQuad, TextEngine};
use std::collections::{BTreeSet, HashMap};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
pub use wgpu;

const DEFAULT_GPU_CACHE_LIMIT: u64 = 256 * 1024 * 1024;

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
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white_tex: GpuTexture,
    textures: HashMap<AssetId, GpuTexture>,
    texture_bytes: u64,
    texture_limit: u64,
    use_clock: u64,
    scratch: Option<(GpuTexture, GpuTexture, u32, u32)>,
    vertices: wgpu::Buffer,
    scratch_verts: Vec<Vertex>,
    capacity: usize,
    pub text: TextEngine,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    viewport: Viewport,
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

/// Decodes PNG / WebP / JPEG into premultiplied RGBA8 (suitable for worker threads).
pub fn decode_image(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| rerr(e.to_string()))?;
    let img = reader.decode().map_err(|e| rerr(e.to_string()))?;
    let w = img.width();
    let h = img.height();
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(rerr("image dimensions out of range"));
    }
    let mut pixels = img.to_rgba8().into_raw();
    premultiply_rgba(&mut pixels);
    Ok((w, h, pixels))
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
            label: Some("linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
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

        let white_tex = Self::create_rgba_texture(&device, &queue, &layout, &sampler, 1, 1, &[255, 255, 255, 255]);
        let initial_cap = 2048;
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quads"),
            size: (initial_cap * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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
            layout,
            sampler,
            white_tex,
            textures: HashMap::new(),
            texture_bytes: 0,
            texture_limit: DEFAULT_GPU_CACHE_LIMIT,
            use_clock: 0,
            scratch: None,
            vertices,
            scratch_verts: Vec::with_capacity(initial_cap),
            capacity: initial_cap,
            text: TextEngine::default(),
            atlas,
            text_renderer,
            viewport,
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

    pub fn load_font(&mut self, id: AssetId, bytes: Vec<u8>) -> Result<()> {
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

    fn create_rgba_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        w: u32,
        h: u32,
        rgba: &[u8],
    ) -> GpuTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
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
            bytes: w as u64 * h as u64 * 4,
            last_used: 0,
        }
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
        let mut tex = Self::create_rgba_texture(
            &self.device,
            &self.queue,
            &self.layout,
            &self.sampler,
            w,
            h,
            pixels,
        );
        self.use_clock = self.use_clock.wrapping_add(1);
        tex.last_used = self.use_clock;
        if let Some(old) = self.textures.insert(id, tex) {
            self.texture_bytes = self.texture_bytes.saturating_sub(old.bytes);
        }
        self.texture_bytes = self.texture_bytes.saturating_add(w as u64 * h as u64 * 4);
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
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
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
        let bytes = w as u64 * h as u64 * 4;
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
    /// Never fails with `E_BUDGET`!
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
        for (id, _) in victims {
            if self.texture_bytes <= self.texture_limit {
                break;
            }
            if let Some(tex) = self.textures.remove(&id) {
                self.texture_bytes = self.texture_bytes.saturating_sub(tex.bytes);
            }
        }
    }

    pub fn render(&mut self, p: &DrawPacket, dpr: f32) -> Result<()> {
        if self.surface.is_none() {
            return Ok(());
        }
        let has_transition = !p.transition_from.is_empty() || !p.transition_to.is_empty();
        if !has_transition {
            self.scratch = None;
        }

        // 1. Prepare text glyphs via glyphon
        self.atlas.trim();
        let paint_runs: Vec<nir_ui::TextRun> = p
            .texts
            .iter()
            .flat_map(|r| r.shadow_run().into_iter().chain(std::iter::once(r.clone())))
            .collect();
        self.text.layout_texts(&paint_runs);
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        let mut areas = Vec::new();
        for r in &paint_runs {
            let Some(buf) = self.text.buffers.get(&r.cache_key) else {
                continue;
            };
            let color = Color::rgba(
                (r.color[0] * 255.0) as u8,
                (r.color[1] * 255.0) as u8,
                (r.color[2] * 255.0) as u8,
                (r.color[3] * 255.0) as u8,
            );
            let mut bounds = TextBounds {
                left: (r.x * dpr) as i32,
                top: (r.y * dpr) as i32,
                right: ((r.x + r.width) * dpr) as i32,
                bottom: ((r.y + r.height) * dpr) as i32,
            };
            if let Some([cx, cy, cw, ch]) = r.clip {
                bounds.left = bounds.left.max((cx * dpr) as i32);
                bounds.top = bounds.top.max((cy * dpr) as i32);
                bounds.right = bounds.right.min(((cx + cw) * dpr) as i32);
                bounds.bottom = bounds.bottom.min(((cy + ch) * dpr) as i32);
            }
            if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
                continue;
            }
            if let Some(vis) = r.visible_bytes {
                let offsets = TextEngine::line_offsets(&r.text, !r.emphasis.is_empty());
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
                        top: bounds.top.max(((r.y + run.line_top - r.scroll) * dpr) as i32),
                        right: bounds.right.min(((r.x + right + 1.0) * dpr) as i32),
                        bottom: bounds
                            .bottom
                            .min(((r.y + run.line_top + run.line_height - r.scroll) * dpr) as i32),
                    };
                    if row.bottom > row.top {
                        areas.push(TextArea {
                            buffer: buf,
                            left: r.x * dpr,
                            top: (r.y - r.scroll) * dpr,
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
                    left: r.x * dpr,
                    top: (r.y - r.scroll) * dpr,
                    scale: dpr,
                    bounds,
                    default_color: color,
                    custom_glyphs: &[],
                });
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

        // 2. Build vertices (main quads + optional transition source/target quads)
        let mut verts = std::mem::take(&mut self.scratch_verts);
        verts.clear();
        let append = |verts: &mut Vec<Vertex>, quads: &[Quad], vw: f32, vh: f32| {
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
                for idx in [0, 1, 2, 2, 1, 3] {
                    verts.push(Vertex {
                        pos: [
                            corners[idx][0] / vw.max(1.0) * 2.0 - 1.0,
                            1.0 - corners[idx][1] / vh.max(1.0) * 2.0,
                        ],
                        uv: uvs[idx],
                        color,
                    });
                }
            }
        };
        append(&mut verts, &p.quads, p.width, p.height);
        let tr_from_start = verts.len();
        let mut tr_to_start = tr_from_start;
        if has_transition {
            let [sw, sh] = p.stage_size;
            if sw > 0 && sh > 0 {
                let need_alloc = !self
                    .scratch
                    .as_ref()
                    .is_some_and(|(_, _, w, h)| *w == sw && *h == sh);
                if need_alloc {
                    self.scratch = Some((self.offscreen(sw, sh), self.offscreen(sw, sh), sw, sh));
                }
                append(&mut verts, &p.transition_from, sw as f32, sh as f32);
                tr_to_start = verts.len();
                append(&mut verts, &p.transition_to, sw as f32, sh as f32);
            }
        }
        if verts.len() > self.capacity {
            let next_cap = verts.len().next_power_of_two().min(262_144);
            self.vertices = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quads"),
                size: (next_cap * std::mem::size_of::<Vertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = next_cap;
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

        // 4. Optional offscreen render passes for stage transition
        if has_transition {
            if let Some((src, dst, sw, sh)) = &self.scratch {
                for (target, quads, start) in [
                    (src, &p.transition_from, tr_from_start),
                    (dst, &p.transition_to, tr_to_start),
                ] {
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
                    self.paint_quads(&mut pass, quads, start, *sw as f32, *sh as f32, [*sw, *sh], p);
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
        self.atlas.trim();
        self.submitted += 1;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_quads(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        quads: &[Quad],
        v_offset: usize,
        vw: f32,
        vh: f32,
        physical: [u32; 2],
        pkt: &DrawPacket,
    ) {
        if quads.is_empty() {
            return;
        }
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        let sx = physical[0] as f32 / vw.max(1.0);
        let sy = physical[1] as f32 / vh.max(1.0);

        // Batch consecutive quads sharing (asset, blend, scissor) into 1 draw call!
        type QuadBatch = (Option<AssetId>, BlendMode, [u32; 4], u32, u32);
        let mut batch: Option<QuadBatch> = None;
        let flush = |pass: &mut wgpu::RenderPass<'_>, b: &mut Option<QuadBatch>| {
            if let Some((_, _, _, s, e)) = b.take() {
                pass.draw(s..e, 0..1);
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
            let vs = (v_offset + i * 6) as u32;
            let ve = vs + 6;

            if q.special == SpecialQuad::StageTransition {
                flush(pass, &mut batch);
                if let Some((src, dst, _, _)) = &self.scratch {
                    pass.set_pipeline(&self.mix_pipeline);
                    pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
                    pass.set_bind_group(0, &src.bind, &[]);
                    pass.set_bind_group(1, &dst.bind, &[]);
                    let mask = pkt
                        .transition
                        .mask_asset()
                        .and_then(|id| self.textures.get(&id))
                        .unwrap_or(&self.white_tex);
                    pass.set_bind_group(2, &mask.bind, &[]);
                    pass.draw(vs..ve, 0..1);
                }
                continue;
            }

            if let Some((ba, bb, bs, _, bend)) = batch.as_mut() {
                if *ba == q.asset && *bb == q.blend && *bs == scissor && *bend == vs {
                    *bend = ve;
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
            pass.set_pipeline(&self.pipelines[q.blend as usize]);
            pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
            pass.set_bind_group(0, &tex.bind, &[]);
            batch = Some((q.asset, q.blend, scissor, vs, ve));
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

    #[test]
    fn decode_image_png_roundtrip() {
        let mut img = image::RgbaImage::new(4, 3);
        for p in img.pixels_mut() {
            *p = image::Rgba([180, 90, 45, 200]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        let (w, h, rgba) = decode_image(buf.get_ref()).unwrap();
        assert_eq!((w, h), (4, 3));
        assert_eq!(rgba.len(), 4 * 3 * 4);
    }
}
