//! The only assembly point that knows Player, WebHost and WgpuRenderer.
#![forbid(unsafe_code)]
#![cfg(target_arch = "wasm32")]
use nir_render_wgpu::{wgpu, Renderer, RendererBackend};
use wasm_bindgen::prelude::*;
fn js(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}
#[derive(Clone, Copy)]
enum BackendSelection {
    Auto,
    WebGpu,
    WebGl2,
}
impl BackendSelection {
    fn parse(value: &str) -> std::result::Result<Self, JsValue> {
        match value {
            "auto" => Ok(Self::Auto),
            "webgpu" => Ok(Self::WebGpu),
            "webgl2" => Ok(Self::WebGl2),
            _ => Err(js("E_RENDER_BACKEND: expected auto, webgpu, or webgl2")),
        }
    }
}
async fn selected_backend(selection: BackendSelection) -> RendererBackend {
    match selection {
        BackendSelection::WebGpu => RendererBackend::WebGpu,
        BackendSelection::WebGl2 => RendererBackend::WebGl2,
        BackendSelection::Auto => {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::BROWSER_WEBGPU,
                ..Default::default()
            });
            if instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .is_ok()
            {
                RendererBackend::WebGpu
            } else {
                RendererBackend::WebGl2
            }
        }
    }
}
#[wasm_bindgen]
pub async fn probe_backend() -> String {
    selected_backend(BackendSelection::Auto)
        .await
        .as_str()
        .into()
}
async fn renderer(
    canvas_id: &str,
    selection: BackendSelection,
) -> std::result::Result<Renderer, JsValue> {
    let backend = selected_backend(selection).await;
    let canvas = nir_platform_web::canvas(canvas_id)?;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: match backend {
            RendererBackend::WebGpu => wgpu::Backends::BROWSER_WEBGPU,
            RendererBackend::WebGl2 => wgpu::Backends::GL,
            RendererBackend::Dx12 => unreachable!(),
        },
        ..Default::default()
    });
    let w = canvas.width();
    let h = canvas.height();
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(js)?;
    Renderer::new(&instance, surface, w, h, backend)
        .await
        .map_err(js)
}
#[wasm_bindgen]
pub struct GpuReplacement {
    renderer: Renderer,
}
#[wasm_bindgen]
pub async fn create_gpu(
    canvas_id: String,
    backend: String,
) -> std::result::Result<GpuReplacement, JsValue> {
    Ok(GpuReplacement {
        renderer: renderer(&canvas_id, BackendSelection::parse(&backend)?).await?,
    })
}
#[wasm_bindgen]
pub struct Engine {
    inner: nir_engine::Engine,
}
#[wasm_bindgen]
impl Engine {
    pub async fn create(
        executable_json: String,
        release: String,
        title: String,
        canvas_id: String,
        preferences_json: String,
        backend: String,
    ) -> std::result::Result<Engine, JsValue> {
        console_error_panic_hook::set_once();
        let executable =
            nir_content::parse(executable_json.as_bytes(), "runtime-executable").map_err(js)?;
        let preferences =
            nir_content::parse(preferences_json.as_bytes(), "preferences").map_err(js)?;
        let renderer = renderer(&canvas_id, BackendSelection::parse(&backend)?).await?;
        Ok(Self {
            inner: nir_engine::Engine::new(executable, release, title, Some(preferences), renderer)
                .map_err(js)?,
        })
    }
    pub fn begin_turn(&mut self) {
        self.inner.begin_turn()
    }
    pub fn set_profiling(&mut self, enabled: bool) {
        self.inner.set_profiling(enabled)
    }
    pub fn take_profile(&mut self) -> String {
        self.inner.take_profile()
    }
    pub fn text_cache_stats(&self) -> String {
        self.inner.text_cache_stats()
    }
    pub fn continue_turn(&mut self) -> std::result::Result<(), JsValue> {
        self.inner.continue_turn().map_err(js)
    }
    pub fn pending_events(&self) -> usize {
        self.inner.pending_events()
    }
    pub fn commands(&mut self) -> String {
        self.inner.commands()
    }
    pub fn accepts(&self, request: u32) -> bool {
        self.inner.accepts(request)
    }
    pub fn accepts_content(&self, request: u32) -> bool {
        self.inner.accepts_content(request)
    }
    pub fn content_ready(
        &mut self,
        request: u32,
        objects: js_sys::Array,
    ) -> std::result::Result<(), JsValue> {
        if !self.inner.accepts_content(request) {
            return Ok(());
        }
        let mut total = 0usize;
        let mut data = Vec::new();
        if objects.length() > 128 {
            return self.content_failed(request, "E_CONTENT_LIMIT".into());
        }
        for object in objects.iter() {
            let array = js_sys::Uint8Array::new(&object);
            total = total.saturating_add(array.length() as usize);
            if total > nir_format::MAX_INPUT_BYTES {
                return self.content_failed(request, "E_CONTENT_LIMIT".into());
            }
            data.push(array.to_vec());
        }
        self.inner.content_ready(request, data).map_err(js)
    }
    pub fn content_failed(
        &mut self,
        request: u32,
        message: String,
    ) -> std::result::Result<(), JsValue> {
        self.inner.content_failed(request, message).map_err(js)
    }
    pub fn content_skipped(
        &mut self,
        request: u32,
        code: String,
        detail: String,
    ) -> std::result::Result<(), JsValue> {
        self.inner
            .content_skipped(request, code, detail)
            .map_err(js)
    }
    pub fn retained(&self) -> String {
        self.inner.retained()
    }
    pub fn retained_descriptors(&self) -> String {
        self.inner.retained_descriptors()
    }
    pub fn resource(
        &mut self,
        request: u32,
        id: String,
        bytes: &[u8],
    ) -> std::result::Result<bool, JsValue> {
        self.inner.resource(request, id, bytes).map_err(js)
    }
    pub fn resource_fault(
        &mut self,
        request: u32,
        asset: String,
        code: String,
        stage: String,
        cause: String,
    ) -> std::result::Result<(), JsValue> {
        self.inner
            .resource_fault(request, asset, code, stage, cause)
            .map_err(js)
    }
    pub fn resource_failed(
        &mut self,
        request: u32,
        message: String,
    ) -> std::result::Result<(), JsValue> {
        self.inner.resource_failed(request, message).map_err(js)
    }
    pub fn action(
        &mut self,
        json: String,
        interaction: u32,
        sequence: u32,
        session: u32,
    ) -> std::result::Result<(), JsValue> {
        self.inner
            .action(json, interaction, sequence, session)
            .map_err(js)
    }
    pub fn hover(&mut self, x: f32, y: f32) -> std::result::Result<(), JsValue> {
        self.inner.hover(x, y).map_err(js)
    }
    pub fn hit(&self, x: f32, y: f32) -> String {
        self.inner.hit(x, y)
    }
    pub fn tick(&mut self, delta_us: u32) -> std::result::Result<(), JsValue> {
        self.inner.tick(delta_us).map_err(js)
    }
    pub fn hidden(&mut self, value: bool) -> std::result::Result<(), JsValue> {
        self.inner.hidden(value).map_err(js)
    }
    pub fn audio_ended(&mut self, task: u32, session: u32) -> std::result::Result<(), JsValue> {
        self.inner.audio_ended(task, session).map_err(js)
    }
    pub fn audio_failed(
        &mut self,
        task: u32,
        session: u32,
        message: String,
    ) -> std::result::Result<(), JsValue> {
        self.inner.audio_failed(task, session, message).map_err(js)
    }
    pub fn host_event(&mut self, kind: String, json: String) -> std::result::Result<(), JsValue> {
        self.inner.host_event(kind, json).map_err(js)
    }
    pub fn draw(
        &mut self,
        width: f32,
        height: f32,
        dpr: f32,
    ) -> std::result::Result<String, JsValue> {
        self.inner.draw(width, height, dpr).map_err(js)
    }
    pub fn needs_clock(&self) -> bool {
        self.inner.needs_clock()
    }
    pub fn state(&self) -> String {
        let mut state: serde_json::Value = serde_json::from_str(&self.inner.state()).unwrap();
        state["wasm_memory_bytes"] = serde_json::json!(js_sys::Reflect::get(
            &wasm_bindgen::memory(),
            &JsValue::from_str("buffer")
        )
        .ok()
        .map(|b| js_sys::ArrayBuffer::from(b).byte_length()));
        state.to_string()
    }
    pub fn host_state(&mut self) -> String {
        self.inner.host_state()
    }
    pub fn gpu_error(&self) -> Option<String> {
        self.inner.gpu_error()
    }
    pub fn backend(&self) -> String {
        self.inner.backend()
    }
    pub fn device_lost(&self) -> bool {
        self.inner.device_lost()
    }
    pub fn simulate_device_loss(&self) {
        self.inner.simulate_device_loss()
    }
    pub fn begin_recovery(&mut self) -> std::result::Result<(), JsValue> {
        self.inner.begin_recovery().map_err(js)
    }
    pub fn replace_gpu(&mut self, replacement: GpuReplacement) -> std::result::Result<(), JsValue> {
        self.inner.replace_gpu(replacement.renderer).map_err(js)
    }
}
