//! Shared rendered player, used by browser and native hosts.
#![forbid(unsafe_code)]
use nir_format::*;
use nir_player::{AppCommand, AppEvent, Player, SaveEnvelope};
use nir_presentation::{DrawPacket, Messages, ReadingState, SlotView};
use nir_render_wgpu::Renderer;
use std::collections::{BTreeSet, VecDeque};
fn js(error: impl std::fmt::Display) -> String {
    error.to_string()
}
pub struct Engine {
    player: Player,
    renderer: Renderer,
    messages: Messages,
    packet: DrawPacket,
    keyboard_focus: nir_presentation::KeyboardFocus,
    reading: ReadingState,
    view_sequence: (u32, u32),
    fonts: BTreeSet<String>,
    pending_locale: Option<u32>,
    outbox: Vec<AppCommand>,
    width: f32,
    height: f32,
    dpr: f32,
    ready: bool,
    work_remaining: u32,
    upload_remaining: usize,
    upload_start_us: u64,
    visual_invalidated: bool,
    /// The last projection may be stale: a pumped event did work, a view
    /// mutation set `visual_invalidated`, or the safety valve tripped.
    /// Direct view mutators (focus, hover, gestures, scrolling) mark that
    /// flag, which the draw gate treats as dirty alongside this one.
    state_dirty: bool,
    /// Semantics JSON from the last projection; cloned while the packet
    /// is still clean instead of being rebuilt every frame.
    semantics: String,
    /// Frames served from the cache since the last projection; the safety
    /// valve reprojects periodically to bound a missed dirty signal.
    cached_draws: u32,
    profiling: bool,
    profile_records: VecDeque<HostProfile>,
}
#[derive(Clone, Copy)]
struct HostProfile {
    stage: &'static str,
    start_us: u64,
    end_us: u64,
}
const MAX_PENDING_HOST_PROFILES: usize = 128;
/// Clean frames served before the projection is rebuilt unconditionally,
/// bounding visual drift if a change ever slips past the dirty signals.
const REPROJECT_SAFETY_FRAMES: u32 = 16;
fn profile_clock_us() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        (web_sys::window().unwrap().performance().unwrap().now() * 1000.) as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_micros() as u64
    }
}
impl Engine {
    pub fn new(
        executable: RuntimeExecutable,
        release: String,
        title: String,
        preferences: Option<Preferences>,
        renderer: Renderer,
    ) -> std::result::Result<Self, String> {
        if executable.format != 2 {
            return Err(js("E_RUNTIME_VERSION: expected RuntimeExecutable v2"));
        }
        let player =
            Player::new_runtime(executable.program, release, title, preferences).map_err(js)?;
        let mut e = Self {
            player,
            renderer,
            messages: Messages::default(),
            packet: DrawPacket::default(),
            keyboard_focus: Default::default(),
            reading: ReadingState::default(),
            view_sequence: (0, 0),
            fonts: BTreeSet::new(),
            pending_locale: None,
            outbox: vec![],
            width: 1280.,
            height: 720.,
            dpr: 1.,
            ready: false,
            work_remaining: 10_000,
            upload_remaining: 2 * 1024 * 1024,
            upload_start_us: 0,
            visual_invalidated: true,
            state_dirty: true,
            semantics: String::new(),
            cached_draws: 0,
            profiling: false,
            profile_records: VecDeque::new(),
        };
        e.pump(vec![])?;
        Ok(e)
    }
    /// Called once by the host owner task, never by individual completions.
    pub fn begin_turn(&mut self) {
        self.work_remaining = 10_000;
        self.upload_remaining = 2 * 1024 * 1024;
        self.upload_start_us = 0;
    }
    pub fn set_profiling(&mut self, enabled: bool) {
        self.profiling = enabled;
        self.profile_records.clear();
        self.renderer.set_profiling_clock(if enabled {
            Some(profile_clock_us)
        } else {
            None
        });
    }
    pub fn take_profile(&mut self) -> String {
        let mut records: Vec<HostProfile> = self.profile_records.drain(..).collect();
        records.extend(
            self.renderer
                .take_profile()
                .into_iter()
                .map(|record| HostProfile {
                    stage: record.stage,
                    start_us: record.start_us,
                    end_us: record.end_us,
                }),
        );
        records.sort_by_key(|record| (record.start_us, record.end_us));
        serde_json::to_string(
            &records
                .into_iter()
                .map(|record| {
                    serde_json::json!({
                        "stage": record.stage,
                        "start_us": record.start_us,
                        "end_us": record.end_us,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }
    pub fn text_cache_stats(&self) -> String {
        let stats = self.renderer.text.cache_stats();
        serde_json::json!({
            "hits": stats.hits,
            "misses": stats.misses,
            "evictions": stats.evictions,
            "entries": stats.entries,
        })
        .to_string()
    }
    pub fn continue_turn(&mut self) -> std::result::Result<(), String> {
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
    pub fn accepts_resource(&self, request: u32) -> bool {
        self.player.accepts_resource(request)
    }
    pub fn accepts_content(&self, request: u32) -> bool {
        self.player.accepts_content(request)
    }
    pub fn content_ready(
        &mut self,
        request: u32,
        objects: Vec<Vec<u8>>,
    ) -> std::result::Result<(), String> {
        if !self.player.accepts_content(request) {
            return Ok(());
        }
        if objects.len() > 128 || objects.iter().map(Vec::len).sum::<usize>() > MAX_INPUT_BYTES {
            return self.content_failed(request, "E_CONTENT_LIMIT".into());
        }
        self.pump(vec![AppEvent::ContentReady { request, objects }])
    }
    pub fn take_commands(&mut self) -> Vec<AppCommand> {
        std::mem::take(&mut self.outbox)
    }
    pub fn event(&mut self, event: AppEvent) -> std::result::Result<(), String> {
        self.pump(vec![event])
    }
    pub fn preferences(&self) -> &Preferences {
        &self.player.preferences
    }
    pub fn input(&mut self, action: UiAction, sequence: u32) -> std::result::Result<(), String> {
        self.action(
            serde_json::to_string(&action).unwrap(),
            self.player.current_interaction(),
            sequence,
            self.player.generation.session,
        )
    }
    pub fn input_identity(&self) -> (u32, u32) {
        (
            self.player.generation.session,
            self.player.current_interaction(),
        )
    }
    pub fn scroll_action(
        &self,
        point: Option<(f32, f32)>,
        delta: i32,
        page: bool,
    ) -> Option<UiAction> {
        self.packet
            .scrolls
            .iter()
            .find(|v| {
                point.is_none_or(|(x, y)| {
                    x >= v.rect[0]
                        && x <= v.rect[0] + v.rect[2]
                        && y >= v.rect[1]
                        && y <= v.rect[1] + v.rect[3]
                })
            })
            .map(|view| view.action(delta, page))
    }
    pub fn pointer_action(&self, x: f32, y: f32, button: u8) -> Option<UiAction> {
        nir_presentation::pointer_action(&self.packet, &self.player.model(), x, y, button)
    }
    pub fn primary_action(&self) -> Option<UiAction> {
        if let Some(node) =
            self.keyboard_focus
                .node(&self.packet, self.input_identity(), self.player.screen)
        {
            return Some(node.action.clone());
        }
        nir_presentation::primary_action(&self.packet, &self.player.model())
    }
    pub fn focus_value_action(&self, direction: u8) -> Option<UiAction> {
        let node =
            self.keyboard_focus
                .node(&self.packet, self.input_identity(), self.player.screen)?;
        nir_presentation::value_action(node, direction)
    }
    pub fn control_value_action(
        &self,
        id: u32,
        expected: &UiAction,
        direction: u8,
    ) -> Option<UiAction> {
        nir_presentation::control_value_action(&self.packet, id, expected, direction)
    }
    pub fn focus_control(&mut self, id: Option<u32>) -> std::result::Result<(), String> {
        self.keyboard_focus
            .select(&self.packet, self.input_identity(), self.player.screen, id);
        self.visual_invalidated = true;
        self.sync_focus_selection()
    }
    /// Keyboard focus that lands on a choice row of a typed-result interaction
    /// moves the semantic selection cursor. The observation rides the normal
    /// action path; it carries no input identity and cannot progress the story.
    fn sync_focus_selection(&mut self) -> std::result::Result<(), String> {
        let option = self
            .keyboard_focus
            .node(&self.packet, self.input_identity(), self.player.screen)
            .and_then(|node| match &node.action {
                UiAction::Choose { option } => Some(option.clone()),
                _ => None,
            });
        let Some(option) = option else {
            return Ok(());
        };
        if !self
            .player
            .core()
            .state()
            .choice
            .as_ref()
            .is_some_and(|c| c.result.is_some())
        {
            return Ok(());
        }
        self.pump(vec![AppEvent::Action {
            action: UiAction::SelectChoice { option },
            interaction: self.player.current_interaction(),
            sequence: 0,
            session: self.player.generation.session,
        }])
    }
    pub fn navigate_focus(&mut self, direction: u8) -> std::result::Result<Option<u32>, String> {
        let identity = self.input_identity();
        let screen = self.player.screen;
        let current = self
            .keyboard_focus
            .node(&self.packet, identity, screen)
            .cloned();
        let backwards = matches!(direction, 0 | 4);
        if matches!(direction, 0 | 1 | 4 | 5) {
            if let Some(current) = current {
                let inside = |n: &nir_presentation::SemanticNode, rect: [f32; 4]| {
                    let cy = n.rect[1] + n.rect[3] / 2.;
                    let cx = n.rect[0] + n.rect[2] / 2.;
                    n.enabled
                        && !matches!(n.action, UiAction::Scroll { .. })
                        && cx >= rect[0]
                        && cx <= rect[0] + rect[2]
                        && cy >= rect[1]
                        && cy <= rect[1] + rect[3]
                };
                if let Some(view) = self
                    .packet
                    .scrolls
                    .iter()
                    .find(|v| inside(&current, v.rect))
                    .cloned()
                {
                    let visible: Vec<_> = self
                        .packet
                        .semantics
                        .iter()
                        .filter(|n| inside(n, view.rect))
                        .collect();
                    let cy = current.rect[1] + current.rect[3] / 2.;
                    let at_edge = if direction == 4 {
                        !visible
                            .iter()
                            .any(|n| n.rect[1] + n.rect[3] / 2. < cy - 0.5)
                    } else if direction == 5 {
                        !visible
                            .iter()
                            .any(|n| n.rect[1] + n.rect[3] / 2. > cy + 0.5)
                    } else if backwards {
                        visible.first().map(|n| n.id) == Some(current.id)
                    } else {
                        visible.last().map(|n| n.id) == Some(current.id)
                    };
                    if at_edge
                        && ((backwards && view.offset > 0.)
                            || (!backwards && view.offset < view.max))
                    {
                        let old_actions: Vec<_> =
                            visible.iter().map(|n| n.action.clone()).collect();
                        self.reading.scroll(
                            view.region,
                            if backwards { -1 } else { 1 },
                            &self.packet,
                        );
                        let projected = self.reading.project(
                            &self.player.model(),
                            identity,
                            self.width,
                            self.height,
                            &self.messages,
                            &mut self.renderer.text,
                        );
                        let candidates: Vec<_> = projected
                            .semantics
                            .iter()
                            .filter(|n| inside(n, view.rect) && !old_actions.contains(&n.action))
                            .collect();
                        let next = if backwards {
                            candidates.last()
                        } else {
                            candidates.first()
                        };
                        let id = next.map(|n| n.id).or_else(|| {
                            projected
                                .semantics
                                .iter()
                                .find(|n| n.enabled && n.action == current.action)
                                .map(|n| n.id)
                        });
                        self.keyboard_focus.select(&projected, identity, screen, id);
                        self.packet = projected;
                        self.visual_invalidated = true;
                        self.sync_focus_selection()?;
                        return Ok(id);
                    }
                }
            }
        }
        let id = self
            .keyboard_focus
            .navigate(&self.packet, identity, screen, direction);
        self.visual_invalidated = true;
        self.sync_focus_selection()?;
        Ok(id)
    }
    pub fn focused_center(&self) -> Option<(f32, f32)> {
        let n =
            self.keyboard_focus
                .node(&self.packet, self.input_identity(), self.player.screen)?;
        Some((n.rect[0] + n.rect[2] / 2., n.rect[1] + n.rect[3] / 2.))
    }
    pub fn hit_action(&self, x: f32, y: f32) -> Option<UiAction> {
        self.packet.hit(x, y)
    }
    pub fn hover(&mut self, x: f32, y: f32) -> std::result::Result<(), String> {
        if self.reading.hover_history_bar(x, y) {
            self.visual_invalidated = true;
        }
        let model = self.player.model();
        if !model.authored_menu {
            return Ok(());
        }
        let id = self
            .packet
            .hit_node(x, y)
            .filter(|node| node.enabled)
            .and_then(|node| self.packet.menu_controls.get(&node.id))
            .cloned();
        if id != model.hovered_image {
            self.input(UiAction::HoverImage { id }, 0)?;
        }
        Ok(())
    }
    pub fn pointer_gesture(
        &mut self,
        phase: u8,
        x: f32,
        y: f32,
        button: u8,
    ) -> std::result::Result<bool, String> {
        let identity = self.input_identity();
        if phase == 3 || !self.ready || self.player.is_loading() {
            let consumed =
                self.reading
                    .history_bar_gesture(3, x, y, button, identity, &self.packet);
            self.visual_invalidated |= consumed;
            return Ok(consumed);
        }
        let packet = self.reading.project(
            &self.player.model(),
            identity,
            self.width,
            self.height,
            &self.messages,
            &mut self.renderer.text,
        );
        let consumed = self
            .reading
            .history_bar_gesture(phase, x, y, button, identity, &packet);
        self.visual_invalidated |= consumed;
        Ok(consumed)
    }
    pub fn focus_actions(&self) -> Vec<UiAction> {
        self.packet
            .semantics
            .iter()
            .filter(|n| n.enabled)
            .map(|n| n.action.clone())
            .collect()
    }
    pub fn is_ready(&self) -> bool {
        self.ready
    }
    pub fn content_failed(
        &mut self,
        request: u32,
        message: String,
    ) -> std::result::Result<(), String> {
        self.pump(vec![AppEvent::ContentFailed { request, message }])
    }
    pub fn content_skipped(
        &mut self,
        request: u32,
        code: String,
        detail: String,
    ) -> std::result::Result<(), String> {
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
    ) -> std::result::Result<bool, String> {
        if self.upload_start_us == 0 {
            self.upload_start_us = Micros(profile_clock_us()).0;
        }
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
                    let start = Micros(profile_clock_us());
                    let result = self.renderer.prepare_image(request, &id, bytes);
                    self.resource_stage("decode_allocate", request, &id, start, bytes.len());
                    result.map_err(js)?;
                }
                return self.finish_image_upload(request, &id);
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
        self.asset_ready(request, &id)
    }
    /// Admit an image that a host worker thread decoded and premultiplied
    /// off the owner thread. The host attests that the encoded object bytes
    /// were digest-verified before decoding; dimensions and pixel length
    /// are still re-checked when the pixels are first staged, and the
    /// row-budgeted upload tail is shared with the bytes path.
    pub fn resource_decoded(
        &mut self,
        request: u32,
        id: String,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> std::result::Result<bool, String> {
        if self.upload_start_us == 0 {
            self.upload_start_us = Micros(profile_clock_us()).0;
        }
        if !self.player.accepts_resource(request) {
            return Ok(true);
        }
        let asset = self
            .player
            .asset_descriptor(&id)
            .cloned()
            .ok_or_else(|| js("E_ASSET: unknown resource"))?;
        if asset.kind != AssetKind::Image {
            return Err(js("E_ASSET: decoded delivery accepts images only"));
        }
        if !self.renderer.image_started(request, &id) {
            let start = Micros(profile_clock_us());
            let result =
                self.renderer
                    .prepare_image_decoded(request, &id, width, height, pixels.to_vec());
            self.resource_stage("decode_admit", request, &id, start, pixels.len());
            result.map_err(js)?;
        }
        self.finish_image_upload(request, &id)
    }
    pub fn resource_fault(
        &mut self,
        request: u32,
        asset: String,
        code: String,
        stage: String,
        cause: String,
    ) -> std::result::Result<(), String> {
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
    ) -> std::result::Result<(), String> {
        self.pump(vec![AppEvent::AssetFailed { request, message }])
    }
    pub fn action(
        &mut self,
        json: String,
        interaction: u32,
        sequence: u32,
        session: u32,
    ) -> std::result::Result<(), String> {
        let mut action: UiAction = nir_content::parse(json.as_bytes(), "action").map_err(js)?;
        let identity = (
            self.player.generation.session,
            self.player.current_interaction(),
        );
        let navigation_packet = if self.ready
            && !self.player.is_loading()
            && session == identity.0
            && interaction == identity.1
            && matches!(
                action,
                UiAction::Advance | UiAction::Scroll { .. } | UiAction::MenuHistoryScroll { .. }
            ) {
            // Earlier inputs in this same owner turn may have revealed more text.
            // Navigation must use current layout, not the previous submitted frame.
            let start = self.profile_start();
            let packet = self.reading.project(
                &self.player.model(),
                identity,
                self.width,
                self.height,
                &self.messages,
                &mut self.renderer.text,
            );
            self.profile_end("projection", start);
            Some(packet)
        } else {
            None
        };

        if self.view_sequence.0 == session && sequence <= self.view_sequence.1 {
            return Ok(());
        }
        let valid = session == identity.0
            && interaction == identity.1
            && sequence > self.player.core().state().last_input
            && self.reading.matches(identity)
            && !self.player.is_loading();
        if matches!(action, UiAction::MenuHistoryScroll { .. }) {
            if valid {
                self.reading.scroll_menu_history(
                    &action,
                    navigation_packet.as_ref().unwrap_or(&self.packet),
                );
                self.view_sequence = (session, sequence);
                self.visual_invalidated = true;
            }
            return Ok(());
        }
        if let UiAction::Scroll { region, delta } = action {
            if !valid {
                return Ok(());
            }
            self.reading.scroll(
                region,
                delta,
                navigation_packet.as_ref().unwrap_or(&self.packet),
            );
            self.view_sequence = (session, sequence);
        } else if action == UiAction::Advance
            && valid
            && !self.player.paused()
            && !self.player.interface_hidden()
            && self.player.screen == nir_presentation::Screen::Story
            && self.player.core().state().choice.is_none()
        {
            if let Some((_, dialogue)) = self.player.core().dialogue() {
                if (dialogue.awaiting_advance || dialogue.at_gate)
                    && navigation_packet
                        .as_ref()
                        .unwrap_or(&self.packet)
                        .scrolls
                        .iter()
                        .any(|s| s.region == ScrollRegion::Dialogue && s.offset < s.max - 0.5)
                {
                    self.reading.scroll(
                        ScrollRegion::Dialogue,
                        1,
                        navigation_packet.as_ref().unwrap_or(&self.packet),
                    );
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
    pub fn tick(&mut self, delta_us: u32) -> std::result::Result<(), String> {
        self.pump(vec![AppEvent::Tick {
            delta_us: delta_us as u64,
        }])
    }
    pub fn tick_domains(
        &mut self,
        story_us: u32,
        foreground_us: u32,
    ) -> std::result::Result<(), String> {
        self.pump(vec![AppEvent::TickDomains {
            story_us: story_us as u64,
            foreground_us: foreground_us as u64,
        }])
    }
    pub fn audio_positions_in(
        &mut self,
        domain: TimeDomain,
        session: u32,
        positions: Vec<AudioPosition>,
    ) -> std::result::Result<(), String> {
        if positions.len() > MAX_TASKS {
            return Err("E_AUDIO_POSITION: observation limit".into());
        }
        self.player
            .observe_audio_positions(domain, session, &positions)
            .map_err(js)
    }
    pub fn hidden(&mut self, value: bool) -> std::result::Result<(), String> {
        self.pump(vec![AppEvent::Hidden(value)])
    }
    pub fn audio_ended(&mut self, task: u32, session: u32) -> std::result::Result<(), String> {
        self.audio_ended_in(TimeDomain::Story, task, session)
    }
    pub fn audio_ended_in(
        &mut self,
        domain: TimeDomain,
        task: u32,
        session: u32,
    ) -> std::result::Result<(), String> {
        self.pump(vec![AppEvent::AudioEnded {
            domain,
            task,
            session,
        }])
    }
    pub fn audio_failed(
        &mut self,
        task: u32,
        session: u32,
        message: String,
    ) -> std::result::Result<(), String> {
        self.audio_failed_in(TimeDomain::Story, task, session, message)
    }
    pub fn audio_failed_in(
        &mut self,
        domain: TimeDomain,
        task: u32,
        session: u32,
        message: String,
    ) -> std::result::Result<(), String> {
        self.pump(vec![AppEvent::AudioFailed {
            domain,
            task,
            session,
            message,
        }])
    }
    pub fn host_event(&mut self, kind: String, json: String) -> std::result::Result<(), String> {
        let event = match kind.as_str() {
            "assets_cancelled" => {
                let v: serde_json::Value =
                    nir_content::parse(json.as_bytes(), "assets_cancelled").map_err(js)?;
                let request = v["request"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| js("E_HOST_PROTOCOL: invalid assets_cancelled request"))?;
                AppEvent::AssetsCancelled { request }
            }
            "preferences" => AppEvent::Preferences(
                nir_content::parse(json.as_bytes(), "preferences").map_err(js)?,
            ),
            "profile" => {
                AppEvent::Profile(nir_content::parse(json.as_bytes(), "profile").map_err(js)?)
            }
            "slot_loaded" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Reply {
                    job: u32,
                    envelope: Box<SaveEnvelope>,
                }
                let reply: Reply =
                    nir_content::parse(json.as_bytes(), "slot_loaded").map_err(js)?;
                AppEvent::SlotLoaded {
                    job: reply.job,
                    envelope: reply.envelope,
                }
            }
            "slot_load_failed" => {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Reply {
                    job: u32,
                    message: String,
                }
                let reply: Reply =
                    nir_content::parse(json.as_bytes(), "slot_load_failed").map_err(js)?;
                AppEvent::SlotLoadFailed {
                    job: reply.job,
                    message: reply.message,
                }
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
        if let AppEvent::AssetsCancelled { request } = &event {
            self.renderer.cancel_upload(*request);
            self.renderer.retain(&self.player.retained_assets());
        }
        self.pump(vec![event])
    }
    pub fn draw(
        &mut self,
        width: f32,
        height: f32,
        dpr: f32,
    ) -> std::result::Result<String, String> {
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
            self.visual_invalidated = true;
            self.renderer
                .resize((width * dpr) as u32, (height * dpr) as u32);
            self.player.viewport_changed().map_err(js)?;
            self.pump(vec![])?;
        }
        // Title changes immediately, before its replacement media is ready.
        // Other screens still need layout updates while a cue is preparing
        // (history, scrolling and a dialogue paused at an authored gate).
        let waiting_for_title = (self.player.screen == nir_presentation::Screen::Title
            || (self.player.active_menu_id().is_some() && self.player.error.is_none()))
            && self.player.is_loading();
        if self.ready && !waiting_for_title {
            // The projection is a pure function of the player model and the
            // reading state, so it only needs rebuilding when something could
            // have changed them: a pumped event that did work, a direct view
            // mutation (tracked by `visual_invalidated`), history still
            // settling, or the safety valve that bounds a missed signal.
            let dirty = self.state_dirty
                || self.visual_invalidated
                || self.reading.history_pending()
                || self.cached_draws >= REPROJECT_SAFETY_FRAMES;
            if dirty {
                let projection_start = self.profile_start();
                let mut projected = self.reading.project(
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
                if let Some(node) =
                    self.keyboard_focus
                        .node(&projected, self.input_identity(), self.player.screen)
                {
                    let [x, y, w, h] = node.rect;
                    for rect in [
                        [x, y, w, 2.],
                        [x, y + h - 2., w, 2.],
                        [x, y, 2., h],
                        [x + w - 2., y, 2., h],
                    ] {
                        projected.quads.push(nir_presentation::Quad {
                            rect,
                            color: [1., 0.85, 0.35, 1.],
                            asset: None,
                            clip: None,
                        });
                    }
                } else {
                    self.keyboard_focus.clear();
                }
                self.profile_end("projection", projection_start);
                let draw_start = self.profile_start();
                let needs_render = self.visual_invalidated || !self.packet.visual_eq(&projected);
                self.packet = projected;
                self.profile_end("draw", draw_start);
                if needs_render {
                    self.visual_invalidated = true;
                    self.renderer.render(&self.packet, dpr).map_err(js)?;
                    self.visual_invalidated = false;
                }
                self.renderer.retain(&self.player.retained_assets());
                self.state_dirty = false;
                self.cached_draws = 0;
                let semantics_start = self.profile_start();
                self.semantics = self.build_semantics();
                self.profile_end("semantics", semantics_start);
            } else {
                self.cached_draws += 1;
            }
        } else {
            // Boot and title swaps still report live semantics from the
            // default packet until the first projection runs.
            self.semantics = self.build_semantics();
        }
        Ok(self.semantics.clone())
    }
    fn build_semantics(&self) -> String {
        serde_json::json!({"nodes":self.packet.semantics,"announcement":self.packet.announcement,"announcement_locale":self.packet.announcement_locale,"locale":self.packet.locale,"ready":self.ready}).to_string()
    }
    pub fn needs_clock(&self) -> bool {
        self.ready && (self.player.needs_clock() || self.reading.history_pending())
    }
    pub fn state(&self) -> String {
        let c = self.player.core();
        let ui_plan = &c.program().locale_config.ui[&self.player.effective_ui_locale];
        let text_plan = &c.program().locale_config.text[&self.player.effective_text_locale];
        let residency = self.player.content_residency();
        let mut state = serde_json::json!({"ready":self.ready,"session":self.player.generation.session,"device":self.player.generation.device,"interaction":self.player.current_interaction(),"sequence":c.state().last_input,"screen":format!("{:?}",self.player.presentation_screen()),"locale":self.player.effective_ui_locale,"ui_locale":self.player.effective_ui_locale,"text_locale":self.player.effective_text_locale,"ui_font_plan_digest":ui_plan.digest,"text_font_plan_digest":text_plan.digest,"ui_fonts":ui_plan.fonts,"text_fonts":text_plan.fonts,"locale_pending":self.player.locale_pending(),"locale_error":self.player.locale_error(),"preferences":self.player.preferences,"paused":self.player.paused(),"loading":self.player.is_loading(),"status":self.player.status,"error":self.player.error,"diagnostic":self.player.diagnostic,"outcome":c.state().outcome,"variables":c.state().variables,"dialogue":c.dialogue().map(|(_,d)|serde_json::json!({"id":d.text_id,"locale":d.locale,"font_plan_digest":d.font_plan_digest,"visible":d.visible_text(),"ready":d.awaiting_advance,"gate":d.at_gate})),"choice":c.state().choice,"tick_us":c.state().tick_us,"transition":c.transition().map(|(_,p)|p),"position":c.location(),"history_count":c.state().history.len(),"frames":self.renderer.submitted,"shapes":self.renderer.text.shapes,"resident_bytes":self.player.memory_used(),"content_residency":{"resident_blocks":residency.resident_blocks,"pinned_blocks":residency.pinned_blocks,"resident_bytes":residency.resident_bytes,"pinned_bytes":residency.pinned_bytes,"budget_bytes":residency.budget_bytes,"lease_count":residency.lease_count},"wasm_memory_bytes":Option::<u32>::None,"upload_steps":self.renderer.upload_steps,"turn_upload_bytes":2*1024*1024-self.upload_remaining,"scrolls":self.packet.scrolls,"pending_events":self.player.pending_events(),"turn_work":10_000-self.work_remaining,"adapter":self.renderer.adapter_info,"backend":self.renderer.backend.as_str()});
        state["history_scrollbar"] = serde_json::json!(self.packet.history_bar);
        state["window"] = serde_json::json!(c.window_reveal().map(|(_, _, p)| p));
        state["menu_depth"] = serde_json::json!(self.player.menu_depth());
        state["history_pending"] = serde_json::json!(self.reading.history_pending());
        state["history_error"] = serde_json::json!(self.reading.history_error());
        state["interface_hidden"] = serde_json::json!(self.player.interface_hidden());
        state["foreground_clock_us"] = serde_json::json!(self.player.foreground_clock());
        state["menu_opacity"] = serde_json::json!(self.player.menu_opacity());
        state["menu_transition"] =
            serde_json::json!(self.player.menu_transition().map(|(_, _, p)| p));
        state["replay"] = serde_json::json!(self.player.replay_phase());
        state["foreground_paused"] =
            serde_json::json!(self.player.domain_paused(TimeDomain::ForegroundUi));
        state["dialogue_appearance"] = serde_json::json!(c.sample_dialogue_appearance());
        state.to_string()
    }
    pub fn host_state(&mut self) -> String {
        let start = self.profile_start();
        let c = self.player.core();
        let result = serde_json::json!({
            "ready": self.ready,
            "session": self.player.generation.session,
            "device": self.player.generation.device,
            "interaction": self.player.current_interaction(),
            "sequence": c.state().last_input,
            "screen": format!("{:?}", self.player.presentation_screen()),
            "locale": self.player.effective_ui_locale,
            "paused": self.player.paused(),
            "loading": self.player.is_loading(),
            "has_dialogue": c.dialogue().is_some(),
            "choice_cancellable": c
                .state()
                .choice
                .as_ref()
                .is_some_and(|choice| choice.on_cancel.is_some()),
            "frames": self.renderer.submitted,
            "backend": self.renderer.backend.as_str(),
            "resident_bytes": self.player.memory_used(),
            "upload_steps": self.renderer.upload_steps,
            "turn_upload_bytes": 2 * 1024 * 1024 - self.upload_remaining,
            "scrolls": self.packet.scrolls,
            "history_scrollbar": self.packet.history_bar,
            "menu_depth": self.player.menu_depth(),
            "pending_events": self.player.pending_events(),
            "turn_work": 10_000 - self.work_remaining,
        })
        .to_string();
        self.profile_end("compact_state", start);
        result
    }
    pub fn gpu_error(&self) -> Option<String> {
        self.renderer.validation_error()
    }
    pub fn backend(&self) -> String {
        self.renderer.backend.as_str().into()
    }
    pub fn device_lost(&self) -> bool {
        self.renderer.is_lost()
    }
    pub fn simulate_device_loss(&self) {
        self.renderer.destroy();
    }
    pub fn begin_recovery(&mut self) -> std::result::Result<(), String> {
        self.ready = false;
        self.pump(vec![AppEvent::DeviceLost])
    }
    /// Releases the presentation surface for hosts whose window can be
    /// destroyed (Android suspend). Rendering is skipped until a new surface
    /// is bound; the device, queues and resources all stay live.
    pub fn release_surface(&mut self) {
        self.renderer.release_surface();
    }
    /// Binds a recreated window's surface onto the live device without the
    /// full asset replay `replace_gpu` performs; the next draw repaints.
    pub fn rebind_surface(
        &mut self,
        surface: nir_render_wgpu::wgpu::Surface<'static>,
    ) -> std::result::Result<(), String> {
        self.renderer.rebind_surface(surface);
        self.visual_invalidated = true;
        Ok(())
    }
    pub fn replace_gpu(&mut self, replacement: Renderer) -> std::result::Result<(), String> {
        if replacement.backend != self.renderer.backend {
            return Err(js("E_RENDER_BACKEND: recovery must use the active backend"));
        }
        self.renderer = replacement;
        self.renderer.set_profiling_clock(if self.profiling {
            Some(profile_clock_us)
        } else {
            None
        });
        self.fonts.clear();
        self.visual_invalidated = true;
        self.pump(vec![AppEvent::DeviceReady])
    }
}
impl Engine {
    fn profile_start(&self) -> Option<u64> {
        self.profiling.then(profile_clock_us)
    }
    fn profile_end(&mut self, stage: &'static str, start_us: Option<u64>) {
        if let Some(start_us) = start_us {
            if self.profile_records.len() >= MAX_PENDING_HOST_PROFILES {
                self.profile_records.pop_front();
            }
            self.profile_records.push_back(HostProfile {
                stage,
                start_us,
                end_us: profile_clock_us(),
            });
        }
    }
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
            end_us: Micros(profile_clock_us()),
            bytes,
        });
    }
    /// Row-budgeted image upload with the soft time-slice check, followed by
    /// the shared ready tail. Returns `false` when more rows remain.
    fn finish_image_upload(&mut self, request: u32, id: &str) -> std::result::Result<bool, String> {
        // PNG decode is atomic. Yield before the next incremental
        // upload step when it used this turn's soft time allowance.
        if Micros(profile_clock_us())
            .0
            .saturating_sub(self.upload_start_us)
            >= 4_000
        {
            return Ok(false);
        }
        let start = Micros(profile_clock_us());
        let (complete, used) = self
            .renderer
            .upload_image_step(id, self.upload_remaining)
            .map_err(js)?;
        self.resource_stage("upload_enqueued", request, id, start, used);
        self.upload_remaining -= used;
        if !complete {
            return Ok(false);
        }
        self.asset_ready(request, id)
    }
    fn asset_ready(&mut self, request: u32, id: &str) -> std::result::Result<bool, String> {
        self.pump(vec![AppEvent::AssetReady {
            request,
            asset: id.to_string(),
        }])?;
        if let Some(request) = self.pending_locale {
            if let Some(event) = self.prepare_locale_candidate(request)? {
                self.pump(vec![event])?;
            }
        }
        Ok(true)
    }
    fn pump(&mut self, events: Vec<AppEvent>) -> std::result::Result<(), String> {
        // Bare clock events cost one admission unit each inside the player
        // and only dirty the view when the turn did work beyond admission;
        // every other event changes evaluation state unconditionally.
        let clock_only = events.iter().all(|event| {
            matches!(
                event,
                AppEvent::Tick { .. }
                    | AppEvent::TickDomains { .. }
                    | AppEvent::ContinueStoryTime { .. }
            )
        });
        let admitted = events.len() as u32;
        let mut work = 0u32;
        let mut commands = self.player.pump(events, self.work_remaining);
        let used = self.player.work_used();
        work += used;
        self.work_remaining -= used;
        let mut rounds = 0;
        while !commands.is_empty() {
            rounds += 1;
            if rounds > 32 {
                return Err(js("E_HOST_BUDGET: command cycle"));
            }
            let mut events = vec![];
            for command in commands {
                if let AppCommand::PreparePresentation { request } = command {
                    let start = self.profile_start();
                    let preview = ReadingState::default().project(
                        &self.player.preview(),
                        (0, request),
                        self.width,
                        self.height,
                        &self.messages,
                        &mut self.renderer.text,
                    );
                    self.profile_end("projection", start);
                    let start = Micros(profile_clock_us());
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
            let used = self.player.work_used();
            work += used;
            self.work_remaining -= used;
        }
        self.state_dirty |= !clock_only || work > admitted;
        // Foreground-owned visuals (menu page fades) complete inside
        // clock-only ticks and release their clock token at that instant;
        // without this the settled frame may never be projected.
        self.state_dirty |= self.player.take_ui_visual_pulse();
        Ok(())
    }

    /// Locale preferences can arrive while the boot resource job is still
    /// loading fonts. Keep the candidate paused and shape it only after every
    /// face in both candidate plans has been installed in the renderer.
    fn prepare_locale_candidate(
        &mut self,
        request: u32,
    ) -> std::result::Result<Option<AppEvent>, String> {
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
        let start = self.profile_start();
        let preview = ReadingState::default().project(
            &candidate,
            (self.player.generation.session, request),
            self.width,
            self.height,
            &self.messages,
            &mut self.renderer.text,
        );
        self.profile_end("projection", start);
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
