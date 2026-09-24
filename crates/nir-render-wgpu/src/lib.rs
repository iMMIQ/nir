//! Small ordered 2D compositor. All colors blend in linear light, premultiplied.
#![forbid(unsafe_code)]
use bytemuck::{Pod, Zeroable};
use glyphon::{
    Cache, Color, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use nir_format::{Diagnostic, ErrorDomain, Recovery, Result};
use nir_presentation::{DrawPacket, TextEngine};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
pub use wgpu;

/// Browser rendering backend selected before a canvas context is bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererBackend {
    WebGpu,
    WebGl2,
}

impl RendererBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WebGpu => "webgpu",
            Self::WebGl2 => "webgl2",
        }
    }
}

const MAX_PENDING_PROFILES: usize = 64;

/// One CPU timing sample from the renderer.
///
/// `stage` is one of `prepare.layout`, `prepare.glyphs`, `vertex.build_write`,
/// `surface.acquire`, `command.encode`, `queue.submit`, `present`, `image.convert`,
/// or `image.write`. Times are in microseconds from the clock installed with
/// [`Renderer::set_profiling_clock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderProfile {
    pub stage: &'static str,
    pub start_us: u64,
    pub end_us: u64,
}

#[derive(Default)]
struct ProfileCollector {
    clock: Option<fn() -> u64>,
    records: Vec<RenderProfile>,
}

impl ProfileCollector {
    fn set_clock(&mut self, clock: Option<fn() -> u64>) {
        self.records.clear();
        self.clock = clock;
    }

    fn start(&self) -> Option<u64> {
        if self.records.len() >= MAX_PENDING_PROFILES {
            return None;
        }
        self.clock.map(|clock| clock())
    }

    fn end(&mut self, stage: &'static str, start_us: Option<u64>) {
        if self.records.len() >= MAX_PENDING_PROFILES {
            return;
        }
        if let (Some(clock), Some(start_us)) = (self.clock, start_us) {
            self.records.push(RenderProfile {
                stage,
                start_us,
                end_us: clock(),
            });
        }
    }

    fn take(&mut self) -> Vec<RenderProfile> {
        std::mem::take(&mut self.records)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}
struct Texture {
    bind: wgpu::BindGroup,
    _texture: wgpu::Texture,
}
struct ImageUpload {
    request: u32,
    pixels: Vec<u8>,
    texture: Texture,
    width: u32,
    height: u32,
    row: u32,
}
pub struct Renderer {
    _instance: wgpu::Instance,
    pub device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    mix_pipeline: wgpu::RenderPipeline,
    scratch: Option<(Texture, Texture, u32, u32)>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: BTreeMap<String, Texture>,
    uploads: BTreeMap<String, ImageUpload>,
    pub upload_steps: u64,
    vertices: wgpu::Buffer,
    capacity: usize,
    pub text: TextEngine,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    viewport: Viewport,
    swash: SwashCache,
    pub submitted: u64,
    pub adapter_info: String,
    pub backend: RendererBackend,
    lost: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<String>>>,
    profile: ProfileCollector,
}
fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("E_RENDER", "wgpu", message).classified(
        ErrorDomain::Render,
        "render",
        "gpu",
        vec![Recovery::Reload, Recovery::Exit],
    )
}
fn select_surface_format(
    formats: &[wgpu::TextureFormat],
    backend: RendererBackend,
) -> Result<(
    wgpu::TextureFormat,
    wgpu::TextureFormat,
    Vec<wgpu::TextureFormat>,
)> {
    match backend {
        RendererBackend::WebGpu => {
            let surface_format = *formats.first().ok_or_else(|| error("no surface format"))?;
            let format = surface_format.add_srgb_suffix();
            let views = if format == surface_format {
                Vec::new()
            } else {
                vec![format]
            };
            Ok((surface_format, format, views))
        }
        RendererBackend::WebGl2 => {
            // WebGL2 does not offer texture view reinterpretation. Its surface
            // presents through an sRGB conversion shader when configured as sRGB.
            let format = formats
                .iter()
                .copied()
                .find(wgpu::TextureFormat::is_srgb)
                .ok_or_else(|| error("WebGL2 surface has no sRGB format"))?;
            Ok((format, format, Vec::new()))
        }
    }
}
fn image_dimensions_allowed(width: u32, height: u32, max_texture_dimension: u32) -> bool {
    width > 0
        && height > 0
        && width <= 8192
        && height <= 8192
        && (width as u64) * (height as u64) <= 32 * 1024 * 1024
        && width <= max_texture_dimension
        && height <= max_texture_dimension
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
        1.055 * v.powf(1. / 2.4) - 0.055
    }
}
fn premultiply_pixel(pixel: &mut [u8; 4]) {
    match pixel[3] {
        255 => {}
        0 => pixel[..3].fill(0),
        alpha => {
            let a = alpha as f32 / 255.;
            for channel in &mut pixel[..3] {
                *channel = (srgb(linear(*channel as f32 / 255.) * a) * 255.)
                    .round()
                    .clamp(0., 255.) as u8;
            }
        }
    }
}
impl Renderer {
    pub async fn new(
        instance: &wgpu::Instance,
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
            .map_err(|e| error(e.to_string()))?;
        let info = adapter.get_info();
        let adapter_info = format!(
            "{:?} / {:?} / {} / {}",
            info.backend, info.device_type, info.name, info.driver
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("NIR renderer"),
                required_features: wgpu::Features::empty(),
                required_limits: match backend {
                    RendererBackend::WebGpu => wgpu::Limits::downlevel_defaults(),
                    RendererBackend::WebGl2 => wgpu::Limits::downlevel_webgl2_defaults(),
                }
                .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| error(e.to_string()))?;
        let lost = Arc::new(AtomicBool::new(false));
        let flag = lost.clone();
        device.set_device_lost_callback(move |_, _| {
            flag.store(true, Ordering::Relaxed);
        });
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink = errors.clone();
        device.on_uncaptured_error(Box::new(move |e| sink.lock().unwrap().push(e.to_string())));
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let caps = surface.get_capabilities(&adapter);
        let (surface_format, format, view_formats) = select_surface_format(&caps.formats, backend)?;
        let max_dimension = device.limits().max_texture_dimension_2d;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: width.clamp(1, max_dimension),
            height: height.clamp(1, max_dimension),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats,
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite image"),
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
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("NIR quad"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("linear premultiplied sprites"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 32,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2,2=>Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
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
        let mix_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("transition layout"),
            bind_group_layouts: &[&layout, &layout],
            push_constant_ranges: &[],
        });
        let mix_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("frozen scene dissolve"),
            layout: Some(&mix_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 32,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2,2=>Float32x4],
                }],
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let capacity = 19200;
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ordered quads"),
            size: (capacity * 32) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let mut r = Self {
            _instance: instance.clone(),
            device,
            queue,
            surface,
            config,
            format,
            pipeline,
            mix_pipeline,
            scratch: None,
            layout,
            sampler,
            textures: BTreeMap::new(),
            uploads: BTreeMap::new(),
            upload_steps: 0,
            vertices,
            capacity,
            text: TextEngine::default(),
            atlas,
            text_renderer,
            viewport,
            swash: SwashCache::new(),
            submitted: 0,
            adapter_info,
            backend,
            lost,
            errors,
            profile: ProfileCollector::default(),
        };
        r.upload_rgba("", 1, 1, &[255; 4]);
        if let Some(e) = r.device.pop_error_scope().await {
            return Err(error(e.to_string()));
        }
        Ok(r)
    }
    pub fn validation_error(&self) -> Option<String> {
        self.errors.lock().unwrap().first().cloned()
    }
    /// Enables CPU stage timing with a caller supplied monotonic microsecond clock.
    /// Passing `None` disables timing and clears any pending samples.
    pub fn set_profiling_clock(&mut self, clock: Option<fn() -> u64>) {
        self.profile.set_clock(clock);
    }
    /// Returns and clears up to 64 pending CPU stage timing samples.
    pub fn take_profile(&mut self) -> Vec<RenderProfile> {
        self.profile.take()
    }
    fn profile_start(&self) -> Option<u64> {
        self.profile.start()
    }
    fn profile_end(&mut self, stage: &'static str, start_us: Option<u64>) {
        self.profile.end(stage, start_us);
    }
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }
    pub fn destroy(&self) {
        self.device.destroy();
    }
    pub fn resize(&mut self, w: u32, h: u32) {
        let w = w.max(1).min(self.device.limits().max_texture_dimension_2d);
        let h = h.max(1).min(self.device.limits().max_texture_dimension_2d);
        if self.config.width != w || self.config.height != h {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
        }
    }
    pub fn has_image(&self, id: &str) -> bool {
        self.textures.contains_key(id)
    }
    pub fn image_started(&self, request: u32, id: &str) -> bool {
        self.has_image(id) || self.uploads.get(id).is_some_and(|u| u.request == request)
    }
    pub fn cancel_upload(&mut self, request: u32) {
        self.uploads.retain(|_, upload| upload.request != request);
    }
    /// PNG decoding is atomic; pixel conversion and GPU writes yield by row budget.
    /// A partially uploaded texture never enters the drawable texture map.
    pub fn upload_image_step(
        &mut self,
        request: u32,
        id: &str,
        bytes: &[u8],
        budget: usize,
    ) -> Result<(bool, usize)> {
        if self.has_image(id) {
            return Ok((true, 0));
        }
        self.prepare_image(request, id, bytes)?;
        let upload = self.uploads.get_mut(id).unwrap();
        let stride = upload.width as usize * 4;
        let rows = (budget / stride).min((upload.height - upload.row) as usize) as u32;
        if rows == 0 {
            return Ok((false, 0));
        }
        let start = upload.row as usize * stride;
        let end = start + rows as usize * stride;
        let convert_start = self.profile.start();
        for pixel in upload.pixels[start..end].as_chunks_mut::<4>().0 {
            premultiply_pixel(pixel);
        }
        self.profile.end("image.convert", convert_start);
        let write_start = self.profile.start();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &upload.texture._texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: upload.row,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &upload.pixels[start..end],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(upload.width * 4),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width: upload.width,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        self.profile.end("image.write", write_start);
        upload.row += rows;
        self.upload_steps += 1;
        let complete = upload.row == upload.height;
        if complete {
            self.textures
                .insert(id.into(), self.uploads.remove(id).unwrap().texture);
        }
        Ok((complete, end - start))
    }
    /// Atomic PNG decoding and texture allocation; no pixel uploads occur here.
    pub fn prepare_image(&mut self, request: u32, id: &str, bytes: &[u8]) -> Result<()> {
        if !self.image_started(request, id) {
            let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .map_err(|e| error(e.to_string()))?;
            let (w, h) = reader.into_dimensions().map_err(|e| error(e.to_string()))?;
            if !image_dimensions_allowed(w, h, self.device.limits().max_texture_dimension_2d) {
                return Err(error("image dimensions exceed supported texture limit"));
            }
            let pixels = image::load_from_memory(bytes)
                .map_err(|e| error(e.to_string()))?
                .to_rgba8()
                .into_raw();
            let texture = self.allocate_texture(id, w, h);
            self.uploads.insert(
                id.into(),
                ImageUpload {
                    request,
                    pixels,
                    texture,
                    width: w,
                    height: h,
                    row: 0,
                },
            );
        }
        Ok(())
    }
    fn allocate_texture(&self, id: &str, w: u32, h: u32) -> Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(id),
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
        let view = texture.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(id),
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
        Texture {
            bind,
            _texture: texture,
        }
    }
    fn upload_rgba(&mut self, id: &str, w: u32, h: u32, bytes: &[u8]) {
        let texture = self.allocate_texture(id, w, h);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture._texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
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
        self.textures.insert(id.into(), texture);
    }
    pub fn retain(&mut self, ids: &BTreeSet<String>) {
        self.uploads.retain(|id, _| ids.contains(id));
        self.textures
            .retain(|id, _| id.is_empty() || ids.contains(id));
    }
    pub fn prepare(&mut self, p: &DrawPacket, dpr: f32, full: bool) -> Result<()> {
        let layout_start = self.profile_start();
        self.text.layout(p);
        self.profile_end("prepare.layout", layout_start);
        if let Some(error) = self.text.missing_font.take() {
            return Err(
                Diagnostic::new("E_FONT_PLAN", "presentation.prepare", error).classified(
                    ErrorDomain::Render,
                    "prepare",
                    "font-plan",
                    vec![Recovery::KeepCurrent],
                ),
            );
        }
        let glyphs_start = self.profile_start();
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        let mut areas = vec![];
        for r in &p.texts {
            if r.preflight_only {
                continue;
            }
            let key = TextEngine::key(r);
            let b = &self.text.buffers[&key];
            let color = Color::rgba(
                (r.color[0] * 255.) as u8,
                (r.color[1] * 255.) as u8,
                (r.color[2] * 255.) as u8,
                (r.color[3] * 255.) as u8,
            );
            let mut bounds = TextBounds {
                left: (r.x * dpr) as i32,
                top: (r.y * dpr) as i32,
                right: ((r.x + r.width) * dpr) as i32,
                bottom: ((r.y + r.height) * dpr) as i32,
            };
            if let Some([x, y, w, h]) = r.clip {
                bounds.left = bounds.left.max((x * dpr) as i32);
                bounds.top = bounds.top.max((y * dpr) as i32);
                bounds.right = bounds.right.min(((x + w) * dpr) as i32);
                bounds.bottom = bounds.bottom.min(((y + h) * dpr) as i32);
            }
            if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
                continue;
            }
            if let Some(visible) = r.visible.filter(|_| !full) {
                let offsets = TextEngine::line_offsets(r);
                let scroll = r.scroll;
                for run in b.layout_runs() {
                    let off = *offsets.get(run.line_i).unwrap_or(&0);
                    let right = run
                        .glyphs
                        .iter()
                        .filter(|g| off + g.end <= visible)
                        .map(|g| g.x + g.w)
                        .fold(0f32, f32::max);
                    if right <= 0. {
                        continue;
                    }
                    let row = TextBounds {
                        left: bounds.left,
                        top: bounds.top.max(((r.y + run.line_top - scroll) * dpr) as i32),
                        right: bounds.right.min(((r.x + right + 1.) * dpr) as i32),
                        bottom: bounds
                            .bottom
                            .min(((r.y + run.line_top + run.line_height - scroll) * dpr) as i32),
                    };
                    if row.bottom > row.top {
                        areas.push(TextArea {
                            buffer: b,
                            left: r.x * dpr,
                            top: (r.y - scroll) * dpr,
                            scale: dpr,
                            bounds: row,
                            default_color: color,
                            custom_glyphs: &[],
                        });
                    }
                }
            } else {
                areas.push(TextArea {
                    buffer: b,
                    left: r.x * dpr,
                    top: (r.y - r.scroll) * dpr,
                    scale: dpr,
                    bounds,
                    default_color: color,
                    custom_glyphs: &[],
                });
            }
        }
        let glyphs_result = self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.text.fonts,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash,
        );
        self.profile_end("prepare.glyphs", glyphs_start);
        glyphs_result.map_err(|e| error(e.to_string()))?;
        Ok(())
    }
    fn offscreen(&self, w: u32, h: u32) -> Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frozen scene"),
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
            label: Some("frozen scene"),
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
        Texture {
            bind,
            _texture: texture,
        }
    }
    fn paint(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        quads: &[nir_presentation::Quad],
        start: usize,
        p: &DrawPacket,
        physical: [u32; 2],
    ) -> Result<()> {
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        for (i, q) in quads.iter().enumerate() {
            let id = q.asset.as_deref().unwrap_or("");
            let clip = q.clip.unwrap_or([0., 0., p.width, p.height]);
            let sx = physical[0] as f32 / p.width;
            let sy = physical[1] as f32 / p.height;
            let x = (clip[0] * sx).max(0.) as u32;
            let y = (clip[1] * sy).max(0.) as u32;
            let right = ((clip[0] + clip[2]) * sx).clamp(0., physical[0] as f32) as u32;
            let bottom = ((clip[1] + clip[3]) * sy).clamp(0., physical[1] as f32) as u32;
            if right <= x || bottom <= y {
                continue;
            }
            pass.set_scissor_rect(x, y, right - x, bottom - y);
            if id == "@transition" {
                let (a, b, _, _) = self
                    .scratch
                    .as_ref()
                    .ok_or_else(|| error("transition targets missing"))?;
                pass.set_pipeline(&self.mix_pipeline);
                pass.set_bind_group(0, &a.bind, &[]);
                pass.set_bind_group(1, &b.bind, &[]);
            } else {
                pass.set_pipeline(&self.pipeline);
                let texture = self
                    .textures
                    .get(id)
                    .ok_or_else(|| error(format!("required image not resident: {id}")))?;
                pass.set_bind_group(0, &texture.bind, &[]);
            }
            pass.draw((start + i * 6) as u32..(start + i * 6 + 6) as u32, 0..1);
        }
        Ok(())
    }
    pub fn render(&mut self, p: &DrawPacket, dpr: f32) -> Result<()> {
        self.prepare(p, dpr, false)?;
        let vertex_start = self.profile_start();
        let vertex_result = (|| -> Result<(usize, usize)> {
            let mut verts = vec![];
            let append = |verts: &mut Vec<Vertex>, quads: &[nir_presentation::Quad]| {
                for q in quads {
                    let [x, y, w, h] = q.rect;
                    let a = if q.asset.as_deref() == Some("@transition") {
                        p.transition_layers.as_ref().unwrap().2
                    } else {
                        q.color[3]
                    };
                    let color = [
                        linear(q.color[0]) * a,
                        linear(q.color[1]) * a,
                        linear(q.color[2]) * a,
                        a,
                    ];
                    for (dx, dy, u, v) in [
                        (0., 0., 0., 0.),
                        (w, 0., 1., 0.),
                        (0., h, 0., 1.),
                        (0., h, 0., 1.),
                        (w, 0., 1., 0.),
                        (w, h, 1., 1.),
                    ] {
                        verts.push(Vertex {
                            pos: [(x + dx) / p.width * 2. - 1., 1. - (y + dy) / p.height * 2.],
                            uv: [u, v],
                            color,
                        });
                    }
                }
            };
            append(&mut verts, &p.quads);
            let source_start = verts.len();
            let mut target_start = source_start;
            if let Some((a, b, _)) = &p.transition_layers {
                append(&mut verts, a);
                target_start = verts.len();
                append(&mut verts, b);
                let [w, h] = p.stage_size;
                if w == 0
                    || h == 0
                    || w > self.device.limits().max_texture_dimension_2d
                    || h > self.device.limits().max_texture_dimension_2d
                {
                    return Err(error("stage dimensions exceed GPU texture limit"));
                }
                if !self
                    .scratch
                    .as_ref()
                    .is_some_and(|(_, _, x, y)| *x == w && *y == h)
                {
                    self.scratch = Some((self.offscreen(w, h), self.offscreen(w, h), w, h));
                }
            }
            if verts.len() > self.capacity {
                return Err(error("quad capacity exceeded"));
            }
            self.queue
                .write_buffer(&self.vertices, 0, bytemuck::cast_slice(&verts));
            Ok((source_start, target_start))
        })();
        self.profile_end("vertex.build_write", vertex_start);
        let (source_start, target_start) = vertex_result?;

        let acquire_start = self.profile_start();
        let frame_result = match self.surface.get_current_texture() {
            Ok(f) => Ok(f),
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                self.surface.configure(&self.device, &self.config);
                self.surface
                    .get_current_texture()
                    .map_err(|e| error(e.to_string()))
            }
            Err(e) => Err(error(e.to_string())),
        };
        self.profile_end("surface.acquire", acquire_start);
        let frame = frame_result?;

        let encode_start = self.profile_start();
        let encode_result = (|| -> Result<wgpu::CommandBuffer> {
            let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.format),
                ..Default::default()
            });
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("NIR frame"),
                });
            if let Some((a, b, _)) = &p.transition_layers {
                let (source, target, w, h) = self.scratch.as_ref().unwrap();
                for (texture, quads, start) in
                    [(source, a, source_start), (target, b, target_start)]
                {
                    let view = texture._texture.create_view(&Default::default());
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("freeze side"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
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
                    self.paint(&mut pass, quads, start, p, [*w, *h])?;
                }
            }
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("scene, transition and final-resolution UI"),
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
                self.paint(
                    &mut pass,
                    &p.quads,
                    0,
                    p,
                    [self.config.width, self.config.height],
                )?;
                pass.set_scissor_rect(0, 0, self.config.width, self.config.height);
                self.text_renderer
                    .render(&self.atlas, &self.viewport, &mut pass)
                    .map_err(|e| error(e.to_string()))?;
            }
            Ok(encoder.finish())
        })();
        self.profile_end("command.encode", encode_start);
        let command_buffer = encode_result?;

        let submit_start = self.profile_start();
        self.queue.submit(Some(command_buffer));
        self.profile_end("queue.submit", submit_start);
        let present_start = self.profile_start();
        frame.present();
        self.profile_end("present", present_start);
        self.submitted += 1;
        Ok(())
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static CLOCK_VALUE: AtomicU64 = AtomicU64::new(0);

    fn test_clock() -> u64 {
        CLOCK_VALUE.fetch_add(1, Ordering::Relaxed)
    }

    fn original_premultiply_channel(channel: u8, alpha: u8) -> u8 {
        let a = alpha as f32 / 255.;
        (srgb(linear(channel as f32 / 255.) * a) * 255.)
            .round()
            .clamp(0., 255.) as u8
    }

    #[test]
    fn webgl_uses_srgb_surface_without_view_reinterpretation() {
        let formats = [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ];
        let (surface, view, extras) =
            select_surface_format(&formats, RendererBackend::WebGl2).unwrap();
        assert_eq!(surface, wgpu::TextureFormat::Rgba8UnormSrgb);
        assert_eq!(view, surface);
        assert!(extras.is_empty());
        assert!(select_surface_format(&formats[..1], RendererBackend::WebGl2).is_err());
    }

    #[test]
    fn webgpu_keeps_srgb_view_for_linear_surface() {
        let (surface, view, extras) =
            select_surface_format(&[wgpu::TextureFormat::Bgra8Unorm], RendererBackend::WebGpu)
                .unwrap();
        assert_eq!(surface, wgpu::TextureFormat::Bgra8Unorm);
        assert_eq!(view, wgpu::TextureFormat::Bgra8UnormSrgb);
        assert_eq!(extras, vec![view]);
    }

    #[test]
    fn image_limits_include_adapter_limit() {
        assert!(image_dimensions_allowed(4096, 4096, 4096));
        assert!(!image_dimensions_allowed(4097, 1, 4096));
        assert!(!image_dimensions_allowed(8192, 8192, 16384));
        assert!(!image_dimensions_allowed(0, 1, 4096));
    }

    #[test]
    fn pixel_premultiply_matches_original_formula_for_all_byte_pairs() {
        for alpha in 0..=u8::MAX {
            for channel in 0..=u8::MAX {
                let expected = original_premultiply_channel(channel, alpha);
                let mut pixel = [channel, channel, channel, alpha];
                premultiply_pixel(&mut pixel);
                assert_eq!(
                    pixel,
                    [expected, expected, expected, alpha],
                    "channel {channel}, alpha {alpha}"
                );
            }
        }
    }

    #[test]
    fn profiler_is_idle_bounded_and_drainable_without_a_gpu() {
        CLOCK_VALUE.store(0, Ordering::Relaxed);
        let mut profile = ProfileCollector::default();

        for _ in 0..3 {
            let start = profile.start();
            profile.end("disabled", start);
        }
        assert_eq!(CLOCK_VALUE.load(Ordering::Relaxed), 0);
        assert!(profile.records.is_empty());

        profile.set_clock(Some(test_clock));
        for _ in 0..=MAX_PENDING_PROFILES {
            let start = profile.start();
            profile.end("test.stage", start);
        }
        assert_eq!(profile.records.len(), MAX_PENDING_PROFILES);
        assert_eq!(
            CLOCK_VALUE.load(Ordering::Relaxed),
            (2 * MAX_PENDING_PROFILES) as u64
        );
        assert_eq!(
            profile.records[0],
            RenderProfile {
                stage: "test.stage",
                start_us: 0,
                end_us: 1,
            }
        );

        let records = profile.take();
        assert_eq!(records.len(), MAX_PENDING_PROFILES);
        assert!(profile.records.is_empty());

        let start = profile.start();
        profile.end("cleared-on-disable", start);
        assert_eq!(profile.records.len(), 1);
        profile.set_clock(None);
        assert!(profile.records.is_empty());
        let calls_before_disabled_sample = CLOCK_VALUE.load(Ordering::Relaxed);
        let start = profile.start();
        profile.end("disabled-again", start);
        assert_eq!(
            CLOCK_VALUE.load(Ordering::Relaxed),
            calls_before_disabled_sample
        );
        assert!(profile.take().is_empty());
    }
}
