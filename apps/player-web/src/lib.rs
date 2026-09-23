//! The only assembly point that knows Player, WebHost and WgpuRenderer.
#![forbid(unsafe_code)]
#![cfg(target_arch = "wasm32")]
use nir_format::*;
use nir_player::{AppCommand, AppEvent, Player, SaveEnvelope};
use nir_presentation::{DrawPacket, Messages, ReadingState, SlotView};
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
    reading: ReadingState,
    view_sequence: (u32, u32),
    fonts: BTreeSet<String>,
    pending_locale: Option<u32>,
    outbox: Vec<AppCommand>,
    width: f32,
    height: f32,
    dpr: f32,
    ready: bool,
    signature: String,
    work_remaining: u32,
    upload_remaining: usize,
}
#[wasm_bindgen]
impl Engine {
    pub async fn create(
        executable_json: String,
        release: String,
        title: String,
        canvas_id: String,
        preferences_json: String,
    ) -> std::result::Result<Engine, JsValue> {
        console_error_panic_hook::set_once();
        let executable: RuntimeExecutable =
            nir_content::parse(executable_json.as_bytes(), "runtime-executable").map_err(js)?;
        if executable.format != 2 {
            return Err(js("E_RUNTIME_VERSION: expected RuntimeExecutable v2"));
        }
        let preferences: Preferences =
            nir_content::parse(preferences_json.as_bytes(), "preferences").map_err(js)?;
        let player = Player::new_runtime(executable.program, release, title, Some(preferences))
            .map_err(js)?;
        let renderer = renderer(&canvas_id).await?;
        let mut e = Self {
            player,
            renderer,
            messages: Messages::default(),
            packet: DrawPacket::default(),
            reading: ReadingState::default(),
            view_sequence: (0, 0),
            fonts: BTreeSet::new(),
            pending_locale: None,
            outbox: vec![],
            width: 1280.,
            height: 720.,
            dpr: 1.,
            ready: false,
            signature: String::new(),
            work_remaining: 10_000,
            upload_remaining: 2 * 1024 * 1024,
        };
        e.pump(vec![])?;
        Ok(e)
    }
    /// Called once by the host owner task, never by individual completions.
    pub fn begin_turn(&mut self) {
        self.work_remaining = 10_000;
        self.upload_remaining = 2 * 1024 * 1024;
    }
    pub fn continue_turn(&mut self) -> std::result::Result<(), JsValue> {
        self.pump(vec![])
    }
    pub fn pending_events(&self) -> usize {
        self.player.pending_events()
    }
    pub fn commands(&mut self) -> String {
        serde_json::to_string(&std::mem::take(&mut self.outbox)).unwrap()
    }
    pub fn accepts(&self, request: u32) -> bool {
        self.player.accepts(request)
    }
    pub fn accepts_content(&self, request: u32) -> bool {
        self.player.accepts_content(request)
    }
    pub fn content_ready(
        &mut self,
        request: u32,
        objects: js_sys::Array,
    ) -> std::result::Result<(), JsValue> {
        if !self.player.accepts_content(request) {
            return Ok(());
        }
        let mut bytes = 0usize;
        let mut data = Vec::new();
        if objects.length() > 128 {
            return self.content_failed(request, "E_CONTENT_LIMIT".into());
        }
        for object in objects.iter() {
            let array = js_sys::Uint8Array::new(&object);
            bytes = bytes.saturating_add(array.length() as usize);
            if bytes > MAX_INPUT_BYTES {
                return self.content_failed(request, "E_CONTENT_LIMIT".into());
            }
            data.push(array.to_vec());
        }
        self.pump(vec![AppEvent::ContentReady {
            request,
            objects: data,
        }])
    }
    pub fn content_failed(
        &mut self,
        request: u32,
        message: String,
    ) -> std::result::Result<(), JsValue> {
        self.pump(vec![AppEvent::ContentFailed { request, message }])
    }
    pub fn content_skipped(
        &mut self,
        request: u32,
        code: String,
        detail: String,
    ) -> std::result::Result<(), JsValue> {
        self.pump(vec![AppEvent::ContentSkipped {
            request,
            code,
            message: detail,
        }])
    }
    pub fn retained(&self) -> String {
        serde_json::to_string(&self.player.retained_assets()).unwrap()
    }
    pub fn retained_descriptors(&self) -> String {
        serde_json::to_string(&self.player.retained_descriptors()).unwrap()
    }
    pub fn resource(
        &mut self,
        request: u32,
        id: String,
        bytes: &[u8],
    ) -> std::result::Result<bool, JsValue> {
        if !self.player.accepts_resource(request) {
            return Ok(true);
        }
        let asset = self
            .player
            .asset_descriptor(&id)
            .cloned()
            .ok_or_else(|| js("E_ASSET: unknown resource"))?;
        if bytes.len() as u64 != asset.bytes {
            return Err(js("E_ASSET_SIZE: object length mismatch"));
        }
        if asset.kind != AssetKind::Image || !self.renderer.image_started(request, &id) {
            nir_content::verify(bytes, &asset.object).map_err(js)?;
        }
        match asset.kind {
            AssetKind::Image => {
                if !self.renderer.image_started(request, &id) {
                    let start = nir_platform_web::now_us();
                    let result = self.renderer.prepare_image(request, &id, bytes);
                    self.resource_stage("decode_allocate", request, &id, start, bytes.len());
                    result.map_err(js)?;
                }
                let start = nir_platform_web::now_us();
                let (complete, used) = self
                    .renderer
                    .upload_image_step(request, &id, bytes, self.upload_remaining)
                    .map_err(js)?;
                self.resource_stage("upload_enqueued", request, &id, start, used);
                self.upload_remaining -= used;
                if !complete {
                    return Ok(false);
                }
            }
            AssetKind::Font => {
                if self.fonts.insert(id.clone()) {
                    self.renderer
                        .text
                        .add_font_asset(&id, bytes.to_vec())
                        .map_err(js)?;
                }
            }
            AssetKind::Audio => {}
        }
        self.pump(vec![AppEvent::AssetReady { request, asset: id }])?;
        if let Some(request) = self.pending_locale {
            if let Some(event) = self.prepare_locale_candidate(request)? {
                self.pump(vec![event])?;
            }
        }
        Ok(true)
    }
    pub fn resource_fault(
        &mut self,
        request: u32,
        asset: String,
        code: String,
        stage: String,
        cause: String,
    ) -> std::result::Result<(), JsValue> {
        let mut d = Diagnostic::new(&code, self.player.core().location(), cause).classified(
            ErrorDomain::Prepare,
            "prepare",
            &stage,
            vec![Recovery::Retry, Recovery::KeepCurrent, Recovery::Exit],
        );
        d.details.as_mut().unwrap().references.push(asset);
        self.pump(vec![AppEvent::AssetFault {
            request,
            diagnostic: Box::new(d),
        }])
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
        let mut action: UiAction = nir_content::parse(json.as_bytes(), "action").map_err(js)?;
        let identity = (
            self.player.generation.session,
            self.player.current_interaction(),
        );
        if self.ready
            && !self.player.is_loading()
            && session == identity.0
            && interaction == identity.1
            && matches!(action, UiAction::Advance | UiAction::Scroll { .. })
        {
            // Earlier inputs in this same owner turn may have revealed more text.
            // Navigation must use current layout, not the previous submitted frame.
            self.packet = self.reading.project(
                &self.player.model(),
                identity,
                self.width,
                self.height,
                &self.messages,
                &mut self.renderer.text,
            );
        }

        if self.view_sequence.0 == session && sequence <= self.view_sequence.1 {
            return Ok(());
        }
        let valid = session == identity.0
            && interaction == identity.1
            && sequence > self.player.core().state().last_input
            && self.reading.matches(identity)
            && !self.player.is_loading();
        if let UiAction::Scroll { region, delta } = action {
            if !valid {
                return Ok(());
            }
            self.reading.scroll(region, delta, &self.packet);
            self.view_sequence = (session, sequence);
        } else if action == UiAction::Advance
            && valid
            && !self.player.paused()
            && self.player.screen == nir_presentation::Screen::Story
            && self.player.core().state().choice.is_none()
        {
            if let Some((_, dialogue)) = self.player.core().dialogue() {
                if (dialogue.awaiting_advance || dialogue.at_gate)
                    && self
                        .packet
                        .scrolls
                        .iter()
                        .any(|s| s.region == ScrollRegion::Dialogue && s.offset < s.max - 0.5)
                {
                    self.reading.scroll(ScrollRegion::Dialogue, 1, &self.packet);
                    self.view_sequence = (session, sequence);
                    action = UiAction::Scroll {
                        region: ScrollRegion::Dialogue,
                        delta: 1,
                    };
                } else if !dialogue.awaiting_advance && !dialogue.at_gate {
                    // Reveal-to-gate keeps the reader's viewport; subsequent Advance browses it.
                    self.reading.hold_dialogue();
                    self.view_sequence = (session, sequence);
                }
            }
        }
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
            "host_failed" => AppEvent::HostFailed(json),
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
                AppEvent::SaveFault {
                    job: v["job"].as_u64().unwrap_or(0) as u32,
                    diagnostic: Box::new(Diagnostic::new(
                        v["code"].as_str().unwrap_or("E_STORAGE"),
                        "save",
                        v["message"].as_str().unwrap_or("E_STORAGE"),
                    )),
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
        // Title changes immediately, before its replacement media is ready.
        // Other screens still need layout updates while a cue is preparing
        // (history, scrolling and a dialogue paused at an authored gate).
        let waiting_for_title =
            self.player.screen == nir_presentation::Screen::Title && self.player.is_loading();
        if self.ready && !waiting_for_title {
            self.packet = self.reading.project(
                &self.player.model(),
                (
                    self.player.generation.session,
                    self.player.current_interaction(),
                ),
                width,
                height,
                &self.messages,
                &mut self.renderer.text,
            );
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
        Ok(serde_json::json!({"nodes":self.packet.semantics,"announcement":self.packet.announcement,"announcement_locale":self.packet.announcement_locale,"locale":self.packet.locale,"ready":self.ready}).to_string())
    }
    pub fn needs_clock(&self) -> bool {
        self.ready && self.player.needs_clock()
    }
    pub fn state(&self) -> String {
        let c = self.player.core();
        let ui_plan = &c.program().locale_config.ui[&self.player.effective_ui_locale];
        let text_plan = &c.program().locale_config.text[&self.player.effective_text_locale];
        let residency = self.player.content_residency();
        serde_json::json!({"ready":self.ready,"session":self.player.generation.session,"device":self.player.generation.device,"interaction":self.player.current_interaction(),"sequence":c.state().last_input,"screen":format!("{:?}",self.player.screen),"locale":self.player.effective_ui_locale,"ui_locale":self.player.effective_ui_locale,"text_locale":self.player.effective_text_locale,"ui_font_plan_digest":ui_plan.digest,"text_font_plan_digest":text_plan.digest,"ui_fonts":ui_plan.fonts,"text_fonts":text_plan.fonts,"locale_pending":self.player.locale_pending(),"locale_error":self.player.model().locale_error,"preferences":self.player.preferences,"paused":self.player.paused(),"loading":self.player.is_loading(),"status":self.player.status,"error":self.player.error,"diagnostic":self.player.diagnostic,"outcome":c.state().outcome,"variables":c.state().variables,"dialogue":c.dialogue().map(|(_,d)|serde_json::json!({"id":d.text_id,"locale":d.locale,"font_plan_digest":d.font_plan_digest,"visible":d.visible_text(),"ready":d.awaiting_advance,"gate":d.at_gate})),"choice":c.state().choice,"tick_us":c.state().tick_us,"transition":c.transition().map(|(_,p)|p),"position":c.location(),"history_count":c.state().history.len(),"frames":self.renderer.submitted,"shapes":self.renderer.text.shapes,"resident_bytes":self.player.memory_used(),"content_residency":{"resident_blocks":residency.resident_blocks,"pinned_blocks":residency.pinned_blocks,"resident_bytes":residency.resident_bytes,"pinned_bytes":residency.pinned_bytes,"budget_bytes":residency.budget_bytes,"lease_count":residency.lease_count},"wasm_memory_bytes":js_sys::Reflect::get(&wasm_bindgen::memory(), &JsValue::from_str("buffer")).ok().map(|b| js_sys::ArrayBuffer::from(b).byte_length()),"upload_steps":self.renderer.upload_steps,"turn_upload_bytes":2*1024*1024-self.upload_remaining,"scrolls":self.packet.scrolls,"pending_events":self.player.pending_events(),"turn_work":10_000-self.work_remaining,"adapter":self.renderer.adapter_info}).to_string()
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
    fn resource_stage(
        &mut self,
        stage: &str,
        request: u32,
        asset: &str,
        start_us: Micros,
        bytes: usize,
    ) {
        self.outbox.push(AppCommand::ResourceStage {
            stage: stage.into(),
            request,
            asset: asset.into(),
            object: self
                .player
                .asset_descriptor(asset)
                .map(|descriptor| descriptor.object.clone()),
            session: self.player.generation.session,
            device: self.player.generation.device,
            start_us,
            end_us: nir_platform_web::now_us(),
            bytes,
        });
    }
    fn pump(&mut self, events: Vec<AppEvent>) -> std::result::Result<(), JsValue> {
        let mut commands = self.player.pump(events, self.work_remaining);
        self.work_remaining -= self.player.work_used();
        let mut rounds = 0;
        while !commands.is_empty() {
            rounds += 1;
            if rounds > 32 {
                return Err(js("E_HOST_BUDGET: command cycle"));
            }
            let mut events = vec![];
            for command in commands {
                if let AppCommand::PreparePresentation { request } = command {
                    let preview = ReadingState::default().project(
                        &self.player.preview(),
                        (0, request),
                        self.width,
                        self.height,
                        &self.messages,
                        &mut self.renderer.text,
                    );
                    let start = nir_platform_web::now_us();
                    let prepared = self.renderer.prepare(&preview, self.dpr, true);
                    self.resource_stage("presentation_prepare", request, "", start, 0);
                    match prepared {
                        Ok(()) => {
                            self.ready = true;
                            events.push(AppEvent::PresentationReady { request });
                        }
                        Err(e) => events.push(AppEvent::AssetFailed {
                            request,
                            message: e.to_string(),
                        }),
                    }
                } else if let AppCommand::PrepareLocale { request, .. } = command {
                    self.pending_locale = Some(request);
                    if let Some(event) = self.prepare_locale_candidate(request)? {
                        events.push(event);
                    }
                } else {
                    if let AppCommand::CancelAssets { request } = &command {
                        self.renderer.cancel_upload(*request);
                    }
                    self.outbox.push(command);
                }
            }
            if events.is_empty() {
                break;
            }
            commands = self.player.pump(events, self.work_remaining);
            self.work_remaining -= self.player.work_used();
        }
        Ok(())
    }

    /// Locale preferences can arrive while the boot resource job is still
    /// loading fonts. Keep the candidate paused and shape it only after every
    /// face in both candidate plans has been installed in the renderer.
    fn prepare_locale_candidate(
        &mut self,
        request: u32,
    ) -> std::result::Result<Option<AppEvent>, JsValue> {
        if !self.player.accepts_locale(request) {
            if self.pending_locale == Some(request) {
                self.pending_locale = None;
            }
            return Ok(None);
        }
        let config = &self.player.core().program().locale_config;
        let Some(candidate) = self.player.locale_preview(request) else {
            self.pending_locale = None;
            return Ok(None);
        };
        let required_fonts: Vec<_> = config.ui[&candidate.ui_locale]
            .fonts
            .iter()
            .chain(&config.text[&candidate.text_locale].fonts)
            .cloned()
            .collect();
        if required_fonts.iter().any(|font| !self.fonts.contains(font)) {
            return Ok(None);
        }
        self.pending_locale = None;
        let preview = ReadingState::default().project(
            &candidate,
            (self.player.generation.session, request),
            self.width,
            self.height,
            &self.messages,
            &mut self.renderer.text,
        );
        Ok(Some(
            match self.renderer.prepare(&preview, self.dpr, true) {
                Ok(()) => AppEvent::LocaleReady { request },
                Err(error) => AppEvent::LocaleFailed {
                    request,
                    message: error.to_string(),
                },
            },
        ))
    }
}
