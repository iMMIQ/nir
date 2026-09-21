//! The only assembly point that knows Player, WebHost and WgpuRenderer.
#![forbid(unsafe_code)]
#![cfg(target_arch = "wasm32")]
use nir_format::*;
use nir_player::{AppCommand, AppEvent, Player, SaveEnvelope};
use nir_presentation::{project, DrawPacket, Messages, SlotView};
use nir_render_wgpu::{wgpu, Renderer};
use std::collections::BTreeSet;
use wasm_bindgen::prelude::*;
fn js(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}
async fn renderer(canvas_id: &str) -> std::result::Result<Renderer, JsValue> {
    let canvas = nir_platform_web::canvas(canvas_id)?;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU,
        ..Default::default()
    });
    let w = canvas.width();
    let h = canvas.height();
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(js)?;
    Renderer::new(&instance, surface, w, h).await.map_err(js)
}
#[wasm_bindgen]
pub struct GpuReplacement {
    renderer: Renderer,
}
#[wasm_bindgen]
pub async fn create_gpu(canvas_id: String) -> std::result::Result<GpuReplacement, JsValue> {
    Ok(GpuReplacement {
        renderer: renderer(&canvas_id).await?,
    })
}
#[wasm_bindgen]
pub struct Engine {
    player: Player,
    renderer: Renderer,
    messages: Messages,
    packet: DrawPacket,
    fonts: BTreeSet<String>,
    outbox: Vec<AppCommand>,
    width: f32,
    height: f32,
    dpr: f32,
    ready: bool,
    signature: String,
}
#[wasm_bindgen]
impl Engine {
    pub async fn create(
        executable_json: String,
        release: String,
        title: String,
        canvas_id: String,
    ) -> std::result::Result<Engine, JsValue> {
        console_error_panic_hook::set_once();
        let executable: Executable =
            nir_content::parse(executable_json.as_bytes(), "executable").map_err(js)?;
        nir_content::validate_executable(&executable).map_err(js)?;
        let player = Player::new(executable.program, release, title).map_err(js)?;
        let renderer = renderer(&canvas_id).await?;
        let mut e = Self {
            player,
            renderer,
            messages: Messages::default(),
            packet: DrawPacket::default(),
            fonts: BTreeSet::new(),
            outbox: vec![],
            width: 1280.,
            height: 720.,
            dpr: 1.,
            ready: false,
            signature: String::new(),
        };
        e.pump(vec![])?;
        Ok(e)
    }
    pub fn commands(&mut self) -> String {
        serde_json::to_string(&std::mem::take(&mut self.outbox)).unwrap()
    }
    pub fn accepts(&self, request: u32) -> bool {
        self.player.accepts(request)
    }
    pub fn retained(&self) -> String {
        serde_json::to_string(&self.player.retained_assets()).unwrap()
    }
    pub fn resource(
        &mut self,
        request: u32,
        id: String,
        bytes: &[u8],
    ) -> std::result::Result<(), JsValue> {
        if !self.player.accepts(request) {
            return Ok(());
        }
        let asset = self
            .player
            .core()
            .program()
            .assets
            .get(&id)
            .ok_or_else(|| js("E_ASSET: unknown resource"))?;
        if bytes.len() as u64 != asset.bytes {
            return Err(js("E_ASSET_SIZE: object length mismatch"));
        }
        nir_content::verify(bytes, &asset.object).map_err(js)?;
        match asset.kind {
            AssetKind::Image => self.renderer.upload_image(&id, bytes).map_err(js)?,
            AssetKind::Font => {
                if self.fonts.insert(id.clone()) {
                    self.renderer.text.add_font(bytes.to_vec());
                }
            }
            AssetKind::Audio => {}
        }
        self.pump(vec![AppEvent::AssetReady { request, asset: id }])
    }
    pub fn resource_failed(
        &mut self,
        request: u32,
        message: String,
    ) -> std::result::Result<(), JsValue> {
        self.pump(vec![AppEvent::AssetFailed { request, message }])
    }
    pub fn action(
        &mut self,
        json: String,
        interaction: u32,
        sequence: u32,
        session: u32,
    ) -> std::result::Result<(), JsValue> {
        let action: UiAction = nir_content::parse(json.as_bytes(), "action").map_err(js)?;
        self.pump(vec![AppEvent::Action {
            action,
            interaction,
            sequence,
            session,
        }])
    }
    pub fn hit(&self, x: f32, y: f32) -> String {
        serde_json::to_string(&self.packet.hit(x, y)).unwrap()
    }
    pub fn tick(&mut self, delta_us: u32) -> std::result::Result<(), JsValue> {
        self.pump(vec![AppEvent::Tick {
            delta_us: delta_us as u64,
        }])
    }
    pub fn hidden(&mut self, value: bool) -> std::result::Result<(), JsValue> {
        self.pump(vec![AppEvent::Hidden(value)])
    }
    pub fn audio_ended(&mut self, task: u32, session: u32) -> std::result::Result<(), JsValue> {
        self.pump(vec![AppEvent::AudioEnded { task, session }])
    }
    pub fn audio_failed(
        &mut self,
        task: u32,
        session: u32,
        message: String,
    ) -> std::result::Result<(), JsValue> {
        self.pump(vec![AppEvent::AudioFailed {
            task,
            session,
            message,
        }])
    }
    pub fn host_event(&mut self, kind: String, json: String) -> std::result::Result<(), JsValue> {
        let event = match kind.as_str() {
            "preferences" => AppEvent::Preferences(
                nir_content::parse(json.as_bytes(), "preferences").map_err(js)?,
            ),
            "profile" => {
                AppEvent::Profile(nir_content::parse(json.as_bytes(), "profile").map_err(js)?)
            }
            "loaded" => AppEvent::Loaded {
                envelope: Box::new(
                    nir_content::parse::<SaveEnvelope>(json.as_bytes(), "save").map_err(js)?,
                ),
            },
            "load_failed" => AppEvent::LoadFailed(json),
            "slots" => {
                let rows: Vec<serde_json::Value> =
                    nir_content::parse(json.as_bytes(), "slots").map_err(js)?;
                let slots = (0..3)
                    .map(|slot| {
                        let row = rows
                            .iter()
                            .find(|r| r["slot"].as_u64() == Some(slot as u64));
                        SlotView {
                            slot,
                            label: row
                                .and_then(|r| r["label"].as_str())
                                .unwrap_or_default()
                                .to_owned(),
                            exists: row.is_some(),
                        }
                    })
                    .collect();
                let revisions = rows
                    .iter()
                    .filter_map(|r| {
                        Some((r["slot"].as_u64()? as u32, r["revision"].as_u64()? as u32))
                    })
                    .collect();
                AppEvent::Slots(slots, revisions)
            }
            "saved" => {
                let v: serde_json::Value =
                    nir_content::parse(json.as_bytes(), "saved").map_err(js)?;
                AppEvent::Saved {
                    job: v["job"].as_u64().unwrap_or(0) as u32,
                    slot: v["slot"].as_u64().unwrap_or(0) as u32,
                    revision: v["revision"].as_u64().unwrap_or(0) as u32,
                }
            }
            "save_failed" => {
                let v: serde_json::Value =
                    nir_content::parse(json.as_bytes(), "save_failed").map_err(js)?;
                AppEvent::SaveFailed {
                    job: v["job"].as_u64().unwrap_or(0) as u32,
                    message: v["message"].as_str().unwrap_or("E_STORAGE").into(),
                }
            }
            _ => return Err(js("E_HOST_PROTOCOL: unknown event")),
        };
        self.pump(vec![event])
    }
    pub fn draw(
        &mut self,
        width: f32,
        height: f32,
        dpr: f32,
    ) -> std::result::Result<String, JsValue> {
        if width < 1. || height < 1. || !width.is_finite() || !height.is_finite() {
            return Err(js("E_VIEWPORT"));
        }
        let dpr = if dpr.is_finite() {
            dpr.clamp(1., 2.)
        } else {
            1.
        };
        if (self.width, self.height, self.dpr) != (width, height, dpr) {
            self.width = width;
            self.height = height;
            self.dpr = dpr;
            self.renderer
                .resize((width * dpr) as u32, (height * dpr) as u32);
            self.player.viewport_changed().map_err(js)?;
            self.pump(vec![])?;
        }
        if self.ready {
            self.packet = project(&self.player.model(), width, height, &self.messages);
            let signature = format!(
                "{:?}:{:?}:{:?}:{}",
                self.packet.quads, self.packet.texts, self.packet.transition_layers, dpr
            );
            if self.signature != signature {
                self.renderer.render(&self.packet, dpr).map_err(js)?;
                self.signature = signature;
            }
            self.renderer.retain(&self.player.retained_assets());
        }
        Ok(serde_json::json!({"nodes":self.packet.semantics,"announcement":self.packet.announcement,"locale":self.packet.locale,"ready":self.ready}).to_string())
    }
    pub fn needs_clock(&self) -> bool {
        self.ready && self.player.needs_clock()
    }
    pub fn state(&self) -> String {
        let c = self.player.core();
        serde_json::json!({"ready":self.ready,"session":self.player.generation.session,"device":self.player.generation.device,"interaction":self.player.current_interaction(),"sequence":c.state().last_input,"screen":format!("{:?}",self.player.screen),"locale":self.player.preferences.locale,"paused":self.player.paused(),"loading":self.player.is_loading(),"status":self.player.status,"error":self.player.error,"outcome":c.state().outcome,"variables":c.state().variables,"dialogue":c.dialogue().map(|(_,d)|serde_json::json!({"id":d.text_id,"locale":d.locale,"visible":d.visible_text(),"ready":d.awaiting_advance,"gate":d.at_gate})),"choice":c.state().choice,"tick_us":c.state().tick_us,"transition":c.transition().map(|(_,p)|p),"position":c.location(),"history_count":c.state().history.len(),"frames":self.renderer.submitted,"shapes":self.renderer.text.shapes,"resident_bytes":self.player.memory_used(),"adapter":self.renderer.adapter_info}).to_string()
    }
    pub fn gpu_error(&self) -> Option<String> {
        self.renderer.validation_error()
    }
    pub fn device_lost(&self) -> bool {
        self.renderer.is_lost()
    }
    pub fn simulate_device_loss(&self) {
        self.renderer.destroy();
    }
    pub fn begin_recovery(&mut self) -> std::result::Result<(), JsValue> {
        self.ready = false;
        self.pump(vec![AppEvent::DeviceLost])
    }
    pub fn replace_gpu(&mut self, replacement: GpuReplacement) -> std::result::Result<(), JsValue> {
        self.renderer = replacement.renderer;
        self.fonts.clear();
        self.signature.clear();
        self.pump(vec![AppEvent::DeviceReady])
    }
}
impl Engine {
    fn pump(&mut self, events: Vec<AppEvent>) -> std::result::Result<(), JsValue> {
        let mut commands = self.player.pump(events, 10_000);
        let mut rounds = 0;
        while !commands.is_empty() {
            rounds += 1;
            if rounds > 32 {
                return Err(js("E_HOST_BUDGET: command cycle"));
            }
            let mut events = vec![];
            for command in commands {
                if let AppCommand::PreparePresentation { request } = command {
                    let preview = project(
                        &self.player.preview(),
                        self.width,
                        self.height,
                        &self.messages,
                    );
                    match self.renderer.prepare(&preview, self.dpr, true) {
                        Ok(()) => {
                            self.ready = true;
                            events.push(AppEvent::PresentationReady { request });
                        }
                        Err(e) => events.push(AppEvent::AssetFailed {
                            request,
                            message: e.to_string(),
                        }),
                    }
                } else {
                    self.outbox.push(command);
                }
            }
            if events.is_empty() {
                break;
            }
            commands = self.player.pump(events, 10_000);
        }
        Ok(())
    }
}
