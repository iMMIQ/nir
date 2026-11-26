//! Unified single-owner Galgame runtime combining VM coordination, reading modes
//! (Auto/Skip/Rollback/Replay), predictive asset preloading, exact dirty-gated UI
//! projection, lightweight Toast notices, and live Hot-Reload.
//!
//! Replaces 26,400+ lines across legacy `nir-player` + `nir-engine` + `nir-assets`.
#![forbid(unsafe_code)]

pub mod gesture;

use gesture::{TouchAction, TouchGestureEngine};
use nir_ir::*;
use nir_ui::{
    ConfirmKind, DrawPacket, InputMode, Messages, SafeInsets, Screen, SliderKind, SliderTrack,
    SlotSummary, UiContext,
};
use nir_vm::{ProfileData, RollbackCheckpoint, SaveSnapshot, Vm, VmIntent, WaitReason};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

const MAX_ROLLBACK_CHECKPOINTS: usize = 32;
const SKIP_INTERVAL_US: u64 = 55_000;
const PROFILE_FLUSH_INTERVAL_US: u64 = 5_000_000;
const VOICE_REPLAY_CHANNEL: ChannelId = ChannelId(7);
const AUTOSAVE_LINE_INTERVAL: u32 = 8;
const LOADING_INDICATOR_DELAY_US: u64 = 150_000;
const OVERLAY_ANIM_US: u64 = 165_000;
const TOAST_FADE_US: u64 = 160_000;
const DIALOGUE_FADE_US: u64 = 85_000;
const HOVER_ANIM_US: u64 = 110_000;
const CHOICE_ANIM_US: u64 = 220_000;
/// Waiting-for-click indicator breathes softly for ~2.4s then rests cleanly so idle CPU/GPU hits 0%!
const WAIT_PULSE_DURATION_US: u64 = 2_400_000;
const WAIT_PULSE_STEP_US: u64 = 55_000;

/// Typed commands sent from `Runtime` to the platform host (Native / Web / Mobile).
#[derive(Debug, Clone, PartialEq)]
pub enum HostCommand {
    /// Request the host to load/decode an asset (image/audio/font). `urgent` = needed now.
    LoadAsset {
        id: AssetId,
        urgent: bool,
    },
    /// Hint the renderer/audio host to LRU-evict textures outside this active pinned set.
    HintPinnedAssets(BTreeSet<AssetId>),
    PlayAudio {
        channel: ChannelId,
        bus: AudioBus,
        asset: AssetId,
        gain: f32,
        looped: bool,
        fade_in_us: u64,
        loop_region: Option<AudioLoopRegion>,
        character: Option<String>,
    },
    StopAudio {
        channel: ChannelId,
        fade_out_us: u64,
    },
    SetAudioGain {
        channel: ChannelId,
        gain: f32,
    },
    PauseAudioBus {
        bus: AudioBus,
        paused: bool,
    },
    PlayVideo {
        asset: AssetId,
        skippable: bool,
    },
    StopVideo,
    SaveSlot {
        slot: u32,
        snapshot: Box<SaveSnapshot>,
    },
    RequestLoadSlot(u32),
    DeleteSlot(u32),
    SaveAuto {
        snapshot: Box<SaveSnapshot>,
    },
    RequestLoadAuto,
    SaveQuick {
        snapshot: Box<SaveSnapshot>,
    },
    RequestLoadQuick,
    SaveProfile(ProfileData),
    SavePreferences(Preferences),
    ExportSaves,
    ImportSaves,
    ReturnToShelf,
    Quit,
}

pub struct Runtime {
    program: Arc<NirProgram>,
    vm: Vm,
    frozen_vm: Option<(Vm, Screen)>,
    pub screen: Screen,
    return_screen: Screen,
    pub prefs: Preferences,
    messages: Messages,
    pub auto: bool,
    pub skip: bool,
    pub hold_skip: bool,
    pub interface_hidden: bool,
    auto_elapsed_us: u64,
    skip_elapsed_us: u64,
    active_voice_channels: BTreeSet<ChannelId>,
    checkpoints: VecDeque<RollbackCheckpoint>,
    pub slots: Vec<SlotSummary>,
    pub autosave: Option<SlotSummary>,
    pub quicksave: Option<SlotSummary>,
    lines_since_autosave: u32,
    saves_page: usize,
    history_scroll: usize,
    gallery_page: usize,
    viewing_cg: Option<(usize, usize)>,
    settings_tab: u8,
    pending_confirm: Option<ConfirmKind>,
    pub host_has_shelf: bool,
    pub tz_offset_mins: i32,
    urgent_loading_us: u64,
    hovered_hit: Option<u32>,
    pressed_hit: Option<u32>,
    pressed_started_on_hit: bool,
    active_slider: Option<SliderTrack>,
    cg_zoom: f32,
    cg_pan: [f32; 2],
    haptic_pulse_ms: u16,
    hud_toolbar_hovered: bool,
    pub safe_insets: SafeInsets,
    pub input_mode: InputMode,
    gestures: TouchGestureEngine,
    toast: Option<(String, u64)>,
    toast_elapsed_us: u64,
    overlay_anim_us: u64,
    dialogue_fade_us: u64,
    hover_anim_us: u64,
    choice_anim_us: u64,
    last_choice_active: bool,
    last_dialogue_key: Option<(TextId, usize)>,
    dialogue_epoch: u64,
    wait_pulse_us: u64,
    was_waiting_click: bool,
    ui_time_us: u64,
    low_power_hint: bool,
    host_top_left_pad: f32,
    host_top_right_pad: f32,
    title_analysis: Option<TitleAnalysis>,
    skin_analysis: Option<SkinSample>,
    scene_tint: Option<[f32; 3]>,
    style_sampled_bg: Option<AssetId>,
    resolved_style: ResolvedStyle,
    theme_version: u64,
    loaded_assets: BTreeSet<AssetId>,
    /// Tracks in-flight asset requests and whether they have already been promoted to `urgent`.
    requested_assets: BTreeMap<AssetId, bool>,
    commands: Vec<HostCommand>,
    last_packet: DrawPacket,
    asset_epoch: u64,
    last_rendered_asset_epoch: u64,
    pub dirty: bool,
    pub paused_by_host: bool,
    profile_dirty: bool,
    profile_dirty_elapsed_us: u64,
    unix_clock: u64,
}

impl Runtime {
    pub fn new(
        program: Arc<NirProgram>,
        mut prefs: Preferences,
        profile: ProfileData,
        slots: Vec<SlotSummary>,
    ) -> Result<Self> {
        prefs.sanitize();
        if !program.meta.locales.contains(&prefs.text_locale) {
            prefs.text_locale = program.meta.default_locale.clone();
        }
        let mut vm = Vm::new(program.clone(), Some(prefs.text_locale.clone()))?;
        vm.set_profile(profile);

        let slot_count = program.meta.save_slots.clamp(1, 64) as usize;
        let mut norm_slots = Vec::with_capacity(slot_count);
        for i in 0..slot_count as u32 {
            if let Some(existing) = slots.iter().find(|s| s.slot == i) {
                norm_slots.push(existing.clone());
            } else {
                norm_slots.push(SlotSummary {
                    slot: i,
                    ..Default::default()
                });
            }
        }

        let initial_style = ResolvedStyle::from_theme(&program.ui);
        let mut rt = Self {
            program,
            vm,
            frozen_vm: None,
            screen: Screen::Title,
            return_screen: Screen::Title,
            prefs,
            messages: Messages::default(),
            auto: false,
            skip: false,
            hold_skip: false,
            interface_hidden: false,
            auto_elapsed_us: 0,
            skip_elapsed_us: 0,
            active_voice_channels: BTreeSet::new(),
            checkpoints: VecDeque::new(),
            slots: norm_slots,
            autosave: None,
            quicksave: None,
            lines_since_autosave: 0,
            saves_page: 0,
            history_scroll: 0,
            gallery_page: 0,
            viewing_cg: None,
            settings_tab: 0,
            pending_confirm: None,
            host_has_shelf: false,
            tz_offset_mins: 0,
            urgent_loading_us: 0,
            hovered_hit: None,
            pressed_hit: None,
            pressed_started_on_hit: false,
            active_slider: None,
            cg_zoom: 1.0,
            cg_pan: [0.0, 0.0],
            haptic_pulse_ms: 0,
            hud_toolbar_hovered: false,
            safe_insets: SafeInsets::default(),
            input_mode: InputMode::Pointer,
            gestures: TouchGestureEngine::default(),
            toast: None,
            toast_elapsed_us: 0,
            overlay_anim_us: OVERLAY_ANIM_US,
            dialogue_fade_us: DIALOGUE_FADE_US,
            hover_anim_us: HOVER_ANIM_US,
            choice_anim_us: CHOICE_ANIM_US,
            last_choice_active: false,
            last_dialogue_key: None,
            dialogue_epoch: 0,
            wait_pulse_us: 0,
            was_waiting_click: false,
            ui_time_us: 0,
            low_power_hint: false,
            host_top_left_pad: 0.0,
            host_top_right_pad: 0.0,
            title_analysis: None,
            skin_analysis: None,
            scene_tint: None,
            style_sampled_bg: None,
            resolved_style: initial_style,
            theme_version: 1,
            loaded_assets: BTreeSet::new(),
            requested_assets: BTreeMap::new(),
            commands: Vec::new(),
            last_packet: DrawPacket::default(),
            asset_epoch: 0,
            last_rendered_asset_epoch: 0,
            dirty: true,
            paused_by_host: false,
            profile_dirty: false,
            profile_dirty_elapsed_us: 0,
            unix_clock: 0,
        };
        rt.request_title_assets();
        Ok(rt)
    }

    /// Flushes any debounced persistent profile updates (`read_texts`, flags, values) to host storage.
    /// Hosts should call this on window close, suspend, or `pagehide`.
    pub fn flush_pending_saves(&mut self) {
        if self.profile_dirty && self.frozen_vm.is_none() {
            self.profile_dirty = false;
            self.profile_dirty_elapsed_us = 0;
            self.commands
                .push(HostCommand::SaveProfile(self.vm.profile.clone()));
        }
    }

    pub fn program(&self) -> &Arc<NirProgram> {
        &self.program
    }

    pub fn vm(&self) -> &Vm {
        &self.vm
    }

    pub fn last_packet(&self) -> &DrawPacket {
        &self.last_packet
    }

    pub fn set_unix_time(&mut self, unix_secs: u64) {
        self.unix_clock = unix_secs;
    }

    pub fn take_commands(&mut self) -> Vec<HostCommand> {
        std::mem::take(&mut self.commands)
    }

    pub fn show_toast(&mut self, message: impl Into<String>, duration_ms: u64) {
        self.toast = Some((message.into(), duration_ms.saturating_mul(1000)));
        self.toast_elapsed_us = if self.prefs.reduced_motion {
            TOAST_FADE_US
        } else {
            0
        };
        self.dirty = true;
    }

    pub fn set_host_top_chrome_pads(&mut self, left: f32, right: f32) {
        let l = left.clamp(0.0, 220.0);
        let r = right.clamp(0.0, 220.0);
        if (self.host_top_left_pad - l).abs() > 0.5 || (self.host_top_right_pad - r).abs() > 0.5 {
            self.host_top_left_pad = l;
            self.host_top_right_pad = r;
            self.dirty = true;
        }
    }

    pub fn has_hovered_hit(&self) -> bool {
        self.hovered_hit.is_some()
    }

    pub fn is_story_reading(&self) -> bool {
        self.screen == Screen::Story
            && self.viewing_cg.is_none()
            && self.pending_confirm.is_none()
    }

    pub fn is_interface_hidden(&self) -> bool {
        self.screen == Screen::Story && self.interface_hidden
    }

    pub fn is_shaking(&self) -> bool {
        !self.vm.shakes.is_empty()
    }

    /// Returns whether a back/cancel gesture has an in-app layer to dismiss (modal, CG viewer, confirm, hidden UI).
    pub fn can_go_back(&self) -> bool {
        self.pending_confirm.is_some()
            || self.viewing_cg.is_some()
            || (self.interface_hidden && self.screen == Screen::Story)
            || !matches!(self.screen, Screen::Story | Screen::Title)
    }

    /// Consumes any queued subtle haptic feedback pulse duration in milliseconds (0 = none).
    pub fn take_haptic_ms(&mut self) -> u16 {
        std::mem::take(&mut self.haptic_pulse_ms)
    }

    fn queue_haptic(&mut self, ms: u16) {
        if self.input_mode == InputMode::Touch {
            self.haptic_pulse_ms = self.haptic_pulse_ms.max(ms);
        }
    }

    pub fn cg_zoom(&self) -> f32 {
        self.cg_zoom
    }

    pub fn cg_pan(&self) -> [f32; 2] {
        self.cg_pan
    }

    fn reset_cg_view(&mut self) {
        self.cg_zoom = 1.0;
        self.cg_pan = [0.0, 0.0];
    }

    fn clamp_cg_pan(&mut self) {
        if self.cg_zoom <= 1.01 {
            self.cg_zoom = 1.0;
            self.cg_pan = [0.0, 0.0];
            return;
        }
        let max_x = (self.last_packet.width * (self.cg_zoom - 1.0) * 0.55).max(24.0);
        let max_y = (self.last_packet.height * (self.cg_zoom - 1.0) * 0.55).max(24.0);
        self.cg_pan[0] = self.cg_pan[0].clamp(-max_x, max_x);
        self.cg_pan[1] = self.cg_pan[1].clamp(-max_y, max_y);
    }

    fn apply_slider_position(&mut self, track: SliderTrack, x: f32) {
        let t = ((x - track.rect[0]) / track.rect[2].max(1.0)).clamp(0.0, 1.0);
        let action = match track.kind {
            SliderKind::TextSpeed => UiAction::SetTextSpeed(((0.5 + t * 3.5) * 2.0).round() * 0.5),
            SliderKind::FontScale => UiAction::SetFontScale(((0.75 + t * 0.75) * 20.0).round() / 20.0),
            SliderKind::AutoDelay => UiAction::SetAutoDelay((((300.0 + t * 4700.0) / 100.0).round() * 100.0) as u32),
            SliderKind::DialogueOpacity => UiAction::SetDialogueOpacity(((0.45 + t * 0.55) * 20.0).round() / 20.0),
            SliderKind::MasterVolume => UiAction::SetMasterVolume((t * 20.0).round() / 20.0),
            SliderKind::AudioBus(bus) => UiAction::SetVolume {
                bus,
                value: (t * 20.0).round() / 20.0,
            },
        };
        self.dispatch(action);
    }

    pub fn current_screen_name(&self) -> &'static str {
        match self.screen {
            Screen::Title => "title",
            Screen::Story => "story",
            Screen::Menu => "menu",
            Screen::Saves { .. } => "saves",
            Screen::Settings => "settings",
            Screen::History => "history",
            Screen::Gallery => "gallery",
            Screen::Ended => "ended",
        }
    }

    pub fn current_dialogue_info(&self) -> Option<(u32, &str, &str)> {
        let d = self.vm.dialogue.as_ref()?;
        Some((d.text_id.0, d.speaker.as_str(), d.full_text.as_str()))
    }

    /// Monotonically incremented whenever the active dialogue line changes so Web hosts
    /// skip per-frame JSON serialization & WASM string copying!
    pub fn dialogue_epoch(&self) -> u64 {
        self.dialogue_epoch
    }

    pub fn set_low_power_hint(&mut self, low_power: bool) {
        self.low_power_hint = low_power;
    }

    pub fn is_eco_mode(&self) -> bool {
        self.prefs.eco_mode
    }

    fn is_wait_pulse_active(&self) -> bool {
        !self.prefs.reduced_motion
            && !self.prefs.eco_mode
            && self.is_waiting_dialogue_click()
            && self.wait_pulse_us < WAIT_PULSE_DURATION_US
    }

    pub fn stage_rect(&self) -> [f32; 4] {
        self.last_packet.stage_rect
    }

    fn restart_overlay_anim(&mut self) {
        self.overlay_anim_us = if self.prefs.reduced_motion {
            OVERLAY_ANIM_US
        } else {
            0
        };
    }

    fn is_waiting_dialogue_click(&self) -> bool {
        self.screen == Screen::Story
            && !self.interface_hidden
            && self.vm.choice.is_none()
            && !self.auto
            && !self.skip
            && !self.hold_skip
            && self
                .vm
                .dialogue
                .as_ref()
                .is_some_and(|d| d.finished || d.paused_for_click)
    }

    pub fn mark_asset_loaded(&mut self, id: AssetId) {
        self.loaded_assets.insert(id);
        self.requested_assets.remove(&id);
        self.asset_epoch = self.asset_epoch.wrapping_add(1);
        self.dirty = true;
    }

    /// Forces a full GPU redraw after shared fallback fonts load asynchronously.
    pub fn mark_fonts_changed(&mut self) {
        self.asset_epoch = self.asset_epoch.wrapping_add(1);
        self.dirty = true;
    }

    /// Removes LRU-evicted GPU texture IDs from `loaded_assets` so returning to a prior scene reloads them cleanly!
    pub fn mark_assets_evicted(&mut self, ids: &[AssetId]) {
        for id in ids {
            self.loaded_assets.remove(id);
        }
    }

    pub fn mark_asset_failed(&mut self, id: AssetId, err: &str) {
        self.requested_assets.remove(&id);
        self.show_toast(format!("Asset #{} load failed: {err}", id.0), 3500);
    }

    pub fn notify_audio_ended(&mut self, channel: ChannelId) {
        self.active_voice_channels.remove(&channel);
        self.vm.notify_audio_ended(channel);
    }

    pub fn set_host_has_shelf(&mut self, has_shelf: bool) {
        if self.host_has_shelf != has_shelf {
            self.host_has_shelf = has_shelf;
            self.dirty = true;
        }
    }

    pub fn set_tz_offset_mins(&mut self, offset_mins: i32) {
        self.tz_offset_mins = offset_mins.clamp(-720, 840);
    }

    pub fn set_autosave_summary(&mut self, summary: Option<SlotSummary>) {
        self.autosave = summary;
        self.dirty = true;
    }

    pub fn set_quicksave_summary(&mut self, summary: Option<SlotSummary>) {
        self.quicksave = summary;
        self.dirty = true;
    }

    /// Replaces in-memory save summaries, profile, and preferences after an external backup import
    /// or cross-platform sync so Saves / Gallery / Settings update immediately without restart.
    pub fn reload_storage_state(
        &mut self,
        prefs: Option<Preferences>,
        profile: Option<ProfileData>,
        slots: Vec<SlotSummary>,
        autosave: Option<SlotSummary>,
        quicksave: Option<SlotSummary>,
    ) {
        if let Some(mut p) = prefs {
            p.sanitize();
            if !self.program.meta.locales.contains(&p.text_locale) {
                p.text_locale = self.program.meta.default_locale.clone();
            }
            self.prefs = p;
        }
        if let Some(pr) = profile {
            self.vm.set_profile(pr);
        }
        let slot_count = self.program.meta.save_slots.clamp(1, 64) as usize;
        let mut norm_slots = Vec::with_capacity(slot_count);
        for i in 0..slot_count as u32 {
            if let Some(existing) = slots.iter().find(|s| s.slot == i) {
                norm_slots.push(existing.clone());
            } else {
                norm_slots.push(SlotSummary {
                    slot: i,
                    ..Default::default()
                });
            }
        }
        self.slots = norm_slots;
        self.autosave = autosave;
        self.quicksave = quicksave;
        self.dirty = true;
    }

    pub fn mark_slot_has_thumb(&mut self, slot: u32, has_thumb: bool) {
        if let Some(entry) = self.slots.iter_mut().find(|s| s.slot == slot) {
            entry.has_thumb = has_thumb;
            self.dirty = true;
        }
    }

    pub fn mark_autosave_has_thumb(&mut self, has_thumb: bool) {
        if let Some(a) = &mut self.autosave {
            a.has_thumb = has_thumb;
            self.dirty = true;
        }
    }

    pub fn mark_quicksave_has_thumb(&mut self, has_thumb: bool) {
        if let Some(q) = &mut self.quicksave {
            q.has_thumb = has_thumb;
            self.dirty = true;
        }
    }

    /// Emits an automatic breakpoint save when actively reading Story (never during replay or title).
    pub fn emit_autosave(&mut self) {
        if self.screen != Screen::Story
            || self.frozen_vm.is_some()
            || self.vm.outcome.is_some()
            || (self.vm.dialogue.is_none() && self.vm.choice.is_none())
        {
            return;
        }
        self.lines_since_autosave = 0;
        let snap = self.vm.snapshot(self.unix_clock);
        self.autosave = Some(SlotSummary {
            slot: u32::MAX,
            exists: true,
            summary: snap.summary.clone(),
            saved_at_unix: snap.saved_at_unix,
            has_thumb: self.autosave.as_ref().is_some_and(|a| a.has_thumb),
        });
        self.commands.push(HostCommand::SaveAuto {
            snapshot: Box::new(snap),
        });
    }

    /// Loads whichever saved state (AutoSave, QuickSave, or manual slots) is most recent.
    pub fn try_resume_latest(&mut self) -> bool {
        enum Latest {
            Auto(u64),
            Quick(u64),
            Slot(u32, u64),
        }
        let mut best: Option<Latest> = None;
        let ts_of = |l: &Latest| match *l {
            Latest::Auto(t) | Latest::Quick(t) | Latest::Slot(_, t) => t,
        };
        if let Some(a) = &self.autosave {
            if a.exists {
                best = Some(Latest::Auto(a.saved_at_unix));
            }
        }
        if let Some(q) = &self.quicksave {
            if q.exists && best.as_ref().is_none_or(|b| q.saved_at_unix >= ts_of(b)) {
                best = Some(Latest::Quick(q.saved_at_unix));
            }
        }
        for s in &self.slots {
            if s.exists && best.as_ref().is_none_or(|b| s.saved_at_unix > ts_of(b)) {
                best = Some(Latest::Slot(s.slot, s.saved_at_unix));
            }
        }
        match best {
            Some(Latest::Auto(_)) => {
                self.commands.push(HostCommand::RequestLoadAuto);
                true
            }
            Some(Latest::Quick(_)) => {
                self.commands.push(HostCommand::RequestLoadQuick);
                true
            }
            Some(Latest::Slot(slot, _)) => {
                self.commands.push(HostCommand::RequestLoadSlot(slot));
                true
            }
            None => false,
        }
    }

    /// Unified mouse wheel routing across Story, History, Saves, Gallery, and CG Viewer.
    pub fn scroll_wheel(&mut self, delta_y: f32) {
        if delta_y.abs() < 0.01 {
            return;
        }
        let up = delta_y < 0.0;
        if let Some((item_idx, var_idx)) = self.viewing_cg {
            let next_v = if up {
                var_idx.saturating_sub(1)
            } else {
                var_idx + 1
            };
            self.dispatch(UiAction::ViewCg {
                item: item_idx as u32,
                variant: next_v as u16,
            });
            return;
        }
        match self.screen {
            Screen::History => self.dispatch(UiAction::ScrollHistory(if up { 2 } else { -2 })),
            Screen::Saves { .. } => self.dispatch(UiAction::SavesPage(if up { -1 } else { 1 })),
            Screen::Gallery => self.dispatch(UiAction::GalleryPage(if up { -1 } else { 1 })),
            Screen::Story => self.dispatch(if up {
                UiAction::Rollback
            } else {
                UiAction::Advance
            }),
            _ => {}
        }
    }

    /// Dynamically injects an external JSON translation table (`{"texts":{"0":"..."}}` or `{"0":"..."}`)
    /// into the loaded `NirProgram` as `locale` without modifying `.nirpak`!
    pub fn load_translation_patch(
        &mut self,
        locale: &str,
        patch_json: &str,
    ) -> std::result::Result<usize, String> {
        let locale = locale.trim();
        if locale.is_empty() {
            return Err("empty locale".into());
        }
        let val: serde_json::Value =
            serde_json::from_str(patch_json).map_err(|e| format!("invalid patch json: {e}"))?;
        let map = val
            .get("texts")
            .and_then(|v| v.as_object())
            .or_else(|| val.as_object())
            .ok_or_else(|| "expected JSON object mapping text_id -> string".to_owned())?;

        let mut next_prog = (*self.program).clone();
        let base_table = next_prog
            .texts
            .get(&next_prog.meta.default_locale)
            .cloned()
            .unwrap_or_default();
        let mut table = next_prog
            .texts
            .get(locale)
            .cloned()
            .unwrap_or(base_table);
        let mut applied = 0usize;
        for (k, v) in map {
            let Ok(idx) = k.parse::<usize>() else {
                continue;
            };
            let Some(text) = v.as_str() else {
                continue;
            };
            if idx < table.len() {
                table[idx] = TextEntry::plain(text);
                applied += 1;
            }
        }
        if applied == 0 {
            return Ok(0);
        }
        next_prog.texts.insert(locale.to_owned(), table);
        if !next_prog.meta.locales.iter().any(|l| l == locale) {
            next_prog.meta.locales.push(locale.to_owned());
        }
        let arc = Arc::new(next_prog);
        self.program = Arc::clone(&arc);
        self.prefs.text_locale = locale.to_owned();
        self.vm.apply_translation_program(arc, locale.to_owned());
        self.dirty = true;
        Ok(applied)
    }

    fn has_urgent_image_loading(&self) -> bool {
        self.requested_assets.iter().any(|(&id, &urgent)| {
            urgent
                && self
                    .program
                    .asset(id)
                    .is_some_and(|a| a.kind == AssetKind::Image)
        })
    }

    pub fn notify_video_finished(&mut self) {
        self.vm.notify_video_finished();
        self.step_vm();
    }

    fn channel_character_gain(&self, bus: AudioBus, character: Option<&str>) -> f32 {
        if bus == AudioBus::Voice {
            if let Some(ch) = character {
                if let Some(&mult) = self.prefs.character_volumes.get(ch) {
                    return mult.clamp(0.0, 1.0);
                }
            }
        }
        1.0
    }

    /// Live hot-reload of an updated compiled program without resetting reading state!
    pub fn apply_hot_reload(&mut self, new_program: Arc<NirProgram>) {
        self.program = new_program.clone();
        self.vm.hot_reload(new_program);
        self.loaded_assets.clear();
        self.requested_assets.clear();
        self.asset_epoch = self.asset_epoch.wrapping_add(1);
        if self.screen == Screen::Story {
            self.step_vm();
        } else {
            self.request_title_assets();
        }
        self.show_toast("Hot-reloaded script & assets", 2000);
        self.dirty = true;
    }

    /// Invalidates cached image textures (e.g. after WebGL2 context loss or mobile memory purge)
    /// and re-requests active + lookahead assets cleanly.
    pub fn invalidate_all_textures(&mut self) {
        let program = self.program.clone();
        self.loaded_assets.retain(|id| {
            program
                .asset(*id)
                .is_some_and(|a| a.kind != AssetKind::Image)
        });
        self.requested_assets.retain(|id, _| {
            program
                .asset(*id)
                .is_some_and(|a| a.kind != AssetKind::Image)
        });
        self.asset_epoch = self.asset_epoch.wrapping_add(1);
        if self.screen == Screen::Story {
            self.request_active_and_lookahead_assets();
        } else {
            self.request_title_assets();
        }
        self.dirty = true;
    }

    pub fn set_safe_insets(&mut self, insets: SafeInsets) {
        if self.safe_insets != insets {
            self.safe_insets = insets;
            self.dirty = true;
        }
    }

    pub fn set_input_mode(&mut self, mode: InputMode) {
        if self.input_mode != mode {
            self.input_mode = mode;
            if mode == InputMode::Touch {
                self.hovered_hit = None;
            }
            self.dirty = true;
        }
    }

    /// Pauses or resumes runtime clocks and all audio buses for mobile background/lockscreen lifecycle.
    pub fn set_host_paused(&mut self, paused: bool) {
        if self.paused_by_host == paused {
            return;
        }
        self.paused_by_host = paused;
        if paused {
            self.emit_autosave();
            self.flush_pending_saves();
            if self.hold_skip {
                self.hold_skip = false;
            }
            if let Some(TouchAction::HoldSkip(false)) = self.gestures.reset() {
                self.hold_skip = false;
            }
            self.pressed_hit = None;
            self.pressed_started_on_hit = false;
            self.active_slider = None;
            for bus in [AudioBus::Bgm, AudioBus::Voice, AudioBus::Sfx] {
                self.commands
                    .push(HostCommand::PauseAudioBus { bus, paused: true });
            }
        } else {
            self.commands.push(HostCommand::PauseAudioBus {
                bus: AudioBus::Bgm,
                paused: false,
            });
            self.commands.push(HostCommand::PauseAudioBus {
                bus: AudioBus::Sfx,
                paused: false,
            });
            // Keep Voice paused if currently inside the Menu overlay.
            if self.screen == Screen::Story {
                self.commands.push(HostCommand::PauseAudioBus {
                    bus: AudioBus::Voice,
                    paused: false,
                });
            }
            self.dirty = true;
        }
    }

    /// Returns whether Auto or Skip reading is currently active (hosts should keep screen awake).
    pub fn is_auto_or_skip_active(&self) -> bool {
        !self.paused_by_host
            && self.screen == Screen::Story
            && (self.auto || self.skip || self.hold_skip)
    }

    pub fn messages(&self) -> &Messages {
        &self.messages
    }

    pub fn needs_clock(&self) -> bool {
        self.next_wakeup_delay_us().is_some()
    }

    /// Computes exact microseconds until the next visual, timer, or debounced-save event.
    /// Returns `None` when completely idle so hosts can sleep at 0% CPU & 0% GPU!
    pub fn next_wakeup_delay_us(&self) -> Option<u64> {
        if self.paused_by_host {
            return None;
        }
        let mut soonest: Option<u64> = None;
        let mut consider = |us: u64| {
            soonest = Some(soonest.map_or(us, |cur| cur.min(us)));
        };

        if let Some((_, rem)) = &self.toast {
            if !self.prefs.reduced_motion
                && !self.prefs.eco_mode
                && (self.toast_elapsed_us < TOAST_FADE_US || *rem <= TOAST_FADE_US)
            {
                return Some(0);
            }
            if !self.prefs.reduced_motion && !self.prefs.eco_mode && *rem > TOAST_FADE_US {
                consider(rem.saturating_sub(TOAST_FADE_US));
            } else {
                consider(*rem);
            }
        }

        if let Some(g_us) = self.gestures.next_wakeup_delay_us() {
            consider(g_us);
        }

        if !self.prefs.reduced_motion {
            if self.overlay_anim_us < OVERLAY_ANIM_US {
                return Some(0);
            }
            if self.hover_anim_us < HOVER_ANIM_US {
                return Some(0);
            }
            if self.screen == Screen::Story && self.dialogue_fade_us < DIALOGUE_FADE_US {
                return Some(0);
            }
            if self.screen == Screen::Story && self.choice_anim_us < CHOICE_ANIM_US {
                return Some(0);
            }
            if self.is_wait_pulse_active() {
                let rem_step = WAIT_PULSE_STEP_US - (self.ui_time_us % WAIT_PULSE_STEP_US);
                let rem_total = WAIT_PULSE_DURATION_US.saturating_sub(self.wait_pulse_us);
                consider(rem_step.min(rem_total).max(8_000));
            }
        }

        if self.has_urgent_image_loading() && self.urgent_loading_us < LOADING_INDICATOR_DELAY_US {
            consider(LOADING_INDICATOR_DELAY_US.saturating_sub(self.urgent_loading_us));
        }

        if self.profile_dirty && self.frozen_vm.is_none() {
            consider(
                PROFILE_FLUSH_INTERVAL_US
                    .saturating_sub(self.profile_dirty_elapsed_us)
                    .max(10_000),
            );
        }

        if self.screen == Screen::Story {
            if let Some(vm_us) = self.vm.next_wakeup_delay_us(self.prefs.text_speed) {
                consider(vm_us);
            }
            let skipping = self.hold_skip || self.skip;
            if skipping && self.vm.choice.is_none() {
                let can_skip = self.prefs.skip_unread
                    || self.vm.is_current_dialogue_read()
                    || self.vm.wait != WaitReason::Dialogue;
                if can_skip {
                    consider(SKIP_INTERVAL_US.saturating_sub(self.skip_elapsed_us));
                }
            } else if self.auto && self.vm.wait == WaitReason::Dialogue {
                let dlg_done = self.vm.dialogue.as_ref().is_some_and(|d| d.finished);
                let voice_ready =
                    !self.prefs.auto_wait_voice || self.active_voice_channels.is_empty();
                if dlg_done && voice_ready {
                    let target_us = self.prefs.auto_delay_ms as u64 * 1000;
                    consider(target_us.saturating_sub(self.auto_elapsed_us));
                }
            }
        }

        soonest
    }

    pub fn set_hover(&mut self, x: f32, y: f32) {
        self.set_input_mode(InputMode::Pointer);
        let hit = self.last_packet.hit_test(x, y);
        let next = hit.map(|h| h.id);
        let is_choice_hit = hit.is_some_and(|h| matches!(h.action, UiAction::Choose(_)));
        let is_toolbar_hit = hit.is_some_and(|h| {
            matches!(
                h.action,
                UiAction::ToggleAuto
                    | UiAction::ToggleSkip
                    | UiAction::OpenHistory
                    | UiAction::Rollback
                    | UiAction::OpenSaves { save_mode: true }
                    | UiAction::ToggleHideUi
                    | UiAction::OpenMenu
            )
        });
        if next != self.hovered_hit || is_toolbar_hit != self.hud_toolbar_hovered {
            if next != self.hovered_hit
                && next.is_some()
                && !self.prefs.reduced_motion
                && !self.prefs.eco_mode
            {
                self.hover_anim_us = 0;
            }
            self.hovered_hit = next;
            self.hud_toolbar_hovered = is_toolbar_hit;
            if is_choice_hit {
                self.play_choice_hover_se();
            }
            self.dirty = true;
        }
    }

    fn style_probe_ids(&self) -> (Option<AssetId>, Option<AssetId>) {
        let title_id = self
            .program
            .ui
            .title_screen
            .as_ref()
            .and_then(|s| s.background)
            .or(self.program.meta.title_bg);
        let skin_id = self
            .program
            .ui
            .dialogue_styles
            .get(self.vm.dialogue_style as usize)
            .and_then(|s| s.dialogue_bg)
            .or(self.program.ui.dialogue_bg);
        (title_id, skin_id)
    }

    fn active_background_asset(&self) -> Option<AssetId> {
        self.vm
            .layers
            .iter()
            .filter(|l| l.asset.is_some() && l.transform.opacity > 0.2)
            .min_by_key(|l| (l.transform.z_order, l.id.0))
            .and_then(|l| l.asset)
    }

    /// Returns true when the host should supply a tiny RGBA sample of `id` to derive adaptive style DNA.
    pub fn wants_style_sample(&self, id: AssetId) -> bool {
        let (title_id, skin_id) = self.style_probe_ids();
        if title_id == Some(id) && self.title_analysis.is_none() {
            return true;
        }
        if skin_id == Some(id) && self.skin_analysis.is_none() {
            return true;
        }
        if self.screen == Screen::Story
            && self.active_background_asset() == Some(id)
            && self.style_sampled_bg != Some(id)
        {
            return true;
        }
        false
    }

    /// Analyzes a small RGBA pixel sample from `title_bg`, `dialogue_bg`, or active story background
    /// to automatically inherit the original game's visual DNA & ambient lighting (< 0.5ms).
    pub fn analyze_asset_rgba(&mut self, id: AssetId, w: u32, h: u32, rgba: &[u8]) {
        let (title_id, skin_id) = self.style_probe_ids();
        let mut changed = false;
        if title_id == Some(id) && self.title_analysis.is_none() {
            if let Some(ta) = analyze_title_rgba(w, h, rgba) {
                self.title_analysis = Some(ta);
                changed = true;
            }
        }
        if skin_id == Some(id) && self.skin_analysis.is_none() {
            if let Some(sk) = analyze_dialogue_skin_rgba(w, h, rgba) {
                self.skin_analysis = Some(sk);
                changed = true;
            }
        }
        if self.screen == Screen::Story && self.active_background_asset() == Some(id) {
            self.style_sampled_bg = Some(id);
            let tint = analyze_scene_tint_rgba(w, h, rgba);
            if tint != self.scene_tint {
                self.scene_tint = tint;
                changed = true;
            }
        }
        if changed {
            self.resolved_style = synthesize_resolved_style(
                &self.program.ui,
                self.title_analysis.as_ref(),
                self.skin_analysis.as_ref(),
                self.scene_tint,
            );
            self.theme_version = self.theme_version.wrapping_add(1);
            self.dirty = true;
        }
    }

    pub fn resolved_style(&self) -> &ResolvedStyle {
        &self.resolved_style
    }

    pub fn theme_version(&self) -> u64 {
        self.theme_version
    }

    pub fn theme_css_json(&self) -> String {
        self.resolved_style.to_web_css_vars_json()
    }

    fn play_choice_hover_se(&mut self) {
        if let Some(se) = self.vm.choice.as_ref().and_then(|c| c.hover_se) {
            self.request_asset(se, true);
            self.commands.push(HostCommand::PlayAudio {
                channel: ChannelId(6),
                bus: AudioBus::Sfx,
                asset: se,
                gain: 1.0,
                looped: false,
                fade_in_us: 0,
                loop_region: None,
                character: None,
            });
        }
    }

    /// Highlights a pressed control (or captures a slider track for smooth scrubbing)
    /// without triggering buttons until release (`pointer_up`).
    pub fn pointer_down(&mut self, x: f32, y: f32) {
        if let Some(track) = self.last_packet.slider_at(x, y) {
            self.active_slider = Some(track);
            self.pressed_hit = None;
            self.pressed_started_on_hit = true;
            self.apply_slider_position(track, x);
            self.queue_haptic(8);
            return;
        }
        let hit = self.last_packet.hit_test(x, y).map(|h| h.id);
        self.pressed_hit = hit;
        self.pressed_started_on_hit = hit.is_some();
        if hit.is_some() {
            self.dirty = true;
        }
    }

    /// Updates an active pointer-dragged slider track (desktop mouse scrubbing).
    pub fn pointer_drag(&mut self, x: f32, _y: f32) {
        if let Some(track) = self.active_slider {
            self.apply_slider_position(track, x);
        }
    }

    /// Clears active press highlight without firing an action.
    pub fn pointer_cancel(&mut self) {
        if self.pressed_hit.take().is_some() {
            self.dirty = true;
        }
        self.pressed_started_on_hit = false;
        self.active_slider = None;
    }

    /// Releases a primary pointer / tap at viewport coordinates `(x, y)`.
    pub fn pointer_up(&mut self, x: f32, y: f32) {
        if let Some(track) = self.active_slider.take() {
            self.pressed_hit = None;
            self.pressed_started_on_hit = false;
            self.apply_slider_position(track, x);
            return;
        }
        let started_id = self.pressed_hit.take();
        let started_on_hit = self.pressed_started_on_hit;
        self.pressed_started_on_hit = false;
        if started_id.is_some() {
            self.dirty = true;
        }

        if self.interface_hidden && self.screen == Screen::Story {
            self.interface_hidden = false;
            self.dirty = true;
            return;
        }
        if let Some(hit) = self.last_packet.hit_test(x, y) {
            // If press started on a different control, treat release as cancelled.
            if started_on_hit && started_id != Some(hit.id) {
                return;
            }
            // In Touch mode, image choices with a `preview` image use two-tap confirm:
            // tap 1 selects & previews, tap 2 confirms selection.
            if self.input_mode == InputMode::Touch {
                if let UiAction::Choose(idx) = hit.action {
                    let has_preview = self
                        .vm
                        .choice
                        .as_ref()
                        .and_then(|c| c.items.get(idx as usize))
                        .is_some_and(|it| it.preview.is_some());
                    if has_preview && self.hovered_hit != Some(hit.id) {
                        self.hovered_hit = Some(hit.id);
                        self.play_choice_hover_se();
                        self.queue_haptic(8);
                        self.dirty = true;
                        return;
                    }
                }
            }
            let action = hit.action.clone();
            // While zoomed into a CG, ignore the full-image tap-to-advance hitbox (bottom bar buttons still work!)
            if self.viewing_cg.is_some()
                && self.cg_zoom > 1.05
                && hit.label.is_empty()
                && matches!(action, UiAction::ViewCg { .. } | UiAction::CloseCgViewer)
            {
                return;
            }
            self.hovered_hit = None;
            self.queue_haptic(8);
            self.dispatch(action);
            return;
        }
        // Dragged off a pressed button -> cancel cleanly without advancing story!
        if started_on_hit {
            return;
        }
        // One-handed ergonomic overlay dismissal: tapping the dimmed backdrop outside the modal card closes it!
        if self.viewing_cg.is_none()
            && self.pending_confirm.is_none()
            && !matches!(self.screen, Screen::Story | Screen::Title | Screen::Ended)
        {
            if let Some(panel) = self.last_packet.overlay_panel_rect {
                let inside = x >= panel[0]
                    && x <= panel[0] + panel[2]
                    && y >= panel[1]
                    && y <= panel[1] + panel[3];
                if !inside {
                    self.queue_haptic(8);
                    self.dispatch(UiAction::CloseOverlay);
                    return;
                }
            }
        }
        if self.screen == Screen::Story && self.vm.choice.is_none() {
            self.dispatch(UiAction::Advance);
        }
    }

    /// Handles primary click / tap at viewport coordinates `(x, y)` immediately (legacy / desktop helper).
    pub fn pointer_click(&mut self, x: f32, y: f32) {
        self.pressed_hit = None;
        self.pressed_started_on_hit = false;
        self.active_slider = None;
        self.pointer_up(x, y);
    }

    /// Multi-touch input entrypoints shared across Android Native and Mobile Web.
    pub fn touch_start(&mut self, id: u64, x: f32, y: f32) {
        self.set_input_mode(InputMode::Touch);
        if let Some(act) = self.gestures.touch_start(id, x, y) {
            self.apply_touch_action(act);
        }
    }

    pub fn touch_move(&mut self, id: u64, x: f32, y: f32) {
        let slider_active = self.active_slider.is_some();
        let viewing_cg = self.viewing_cg.is_some();
        let cg_zoomed = self.cg_zoom > 1.02;
        if let Some(act) = self.gestures.touch_move(
            id,
            x,
            y,
            self.screen,
            slider_active,
            viewing_cg,
            cg_zoomed,
        ) {
            self.apply_touch_action(act);
        }
    }

    pub fn touch_end(&mut self, id: u64, x: f32, y: f32) {
        let allow_story_swipes = self.vm.choice.is_none() && !self.pressed_started_on_hit;
        let viewing_cg = self.viewing_cg.is_some();
        let cg_zoomed = self.cg_zoom > 1.02;
        let was_slider = self.active_slider.is_some();
        if let Some(act) = self.gestures.touch_end(
            id,
            x,
            y,
            self.screen,
            allow_story_swipes,
            viewing_cg,
            cg_zoomed,
        ) {
            self.apply_touch_action(act);
        } else if was_slider {
            self.active_slider = None;
            self.pressed_started_on_hit = false;
        }
    }

    pub fn touch_cancel(&mut self, id: u64) {
        if let Some(act) = self.gestures.touch_cancel(id) {
            self.apply_touch_action(act);
        }
    }

    fn apply_touch_action(&mut self, act: TouchAction) {
        match act {
            TouchAction::Down(x, y) => self.pointer_down(x, y),
            TouchAction::Tap(x, y) => self.pointer_up(x, y),
            TouchAction::CancelPress => self.pointer_cancel(),
            TouchAction::CancelOrMenu => {
                self.pointer_cancel();
                self.queue_haptic(10);
                self.cancel_or_menu();
            }
            TouchAction::HoldSkip(held) => {
                if held {
                    self.pointer_cancel();
                    self.queue_haptic(14);
                }
                self.dispatch(UiAction::HoldSkip(held));
            }
            TouchAction::Dispatch(ui_act) => {
                self.pointer_cancel();
                self.queue_haptic(8);
                self.dispatch(ui_act);
            }
            TouchAction::SliderDrag(x, _y) => {
                if let Some(track) = self.active_slider {
                    self.apply_slider_position(track, x);
                }
            }
            TouchAction::CgSwipe(delta) => {
                self.pointer_cancel();
                if let Some((item_idx, var_idx)) = self.viewing_cg {
                    let next_v = if delta < 0 {
                        var_idx.saturating_sub(1)
                    } else {
                        var_idx + 1
                    };
                    self.queue_haptic(8);
                    self.dispatch(UiAction::ViewCg {
                        item: item_idx as u32,
                        variant: next_v as u16,
                    });
                }
            }
            TouchAction::CgPinchZoom { factor, .. } => {
                self.pointer_cancel();
                if self.viewing_cg.is_some() {
                    let prev = self.cg_zoom;
                    self.cg_zoom = (self.cg_zoom * factor).clamp(1.0, 3.0);
                    if (self.cg_zoom - prev).abs() > 0.002 {
                        self.clamp_cg_pan();
                        self.dirty = true;
                    }
                }
            }
            TouchAction::CgPan { dx, dy } => {
                self.pointer_cancel();
                if self.viewing_cg.is_some() && self.cg_zoom > 1.02 {
                    self.cg_pan[0] += dx;
                    self.cg_pan[1] += dy;
                    self.clamp_cg_pan();
                    self.dirty = true;
                }
            }
            TouchAction::CgDoubleTap(pos) => {
                self.pointer_cancel();
                if self.viewing_cg.is_some() {
                    if self.cg_zoom > 1.05 {
                        self.reset_cg_view();
                    } else {
                        self.cg_zoom = 2.0;
                        let cx = self.last_packet.width * 0.5;
                        let cy = self.last_packet.height * 0.5;
                        self.cg_pan = [-(pos[0] - cx) * 0.65, -(pos[1] - cy) * 0.65];
                        self.clamp_cg_pan();
                    }
                    self.queue_haptic(10);
                    self.dirty = true;
                }
            }
        }
    }

    /// Handles secondary click / right-click / Android Back / Escape.
    pub fn cancel_or_menu(&mut self) {
        if self.pending_confirm.take().is_some() {
            self.dirty = true;
            return;
        }
        if self.viewing_cg.take().is_some() {
            self.reset_cg_view();
            self.dirty = true;
            return;
        }
        if self.interface_hidden && self.screen == Screen::Story {
            self.interface_hidden = false;
            self.dirty = true;
            return;
        }
        match self.screen {
            Screen::Story => {
                if self
                    .vm
                    .choice
                    .as_ref()
                    .is_some_and(|c| c.cancel_pc.is_some())
                {
                    self.dispatch(UiAction::CancelChoice);
                } else {
                    self.dispatch(UiAction::OpenMenu);
                }
            }
            Screen::Title => {}
            _ => self.dispatch(UiAction::CloseOverlay),
        }
    }

    pub fn dispatch(&mut self, action: UiAction) {
        self.dirty = true;
        match action {
            UiAction::NewGame => {
                self.flush_pending_saves();
                self.stop_all_audio();
                self.frozen_vm = None;
                self.checkpoints.clear();
                self.auto = false;
                self.skip = false;
                self.interface_hidden = false;
                let profile = self.vm.profile.clone();
                if let Ok(mut vm) =
                    Vm::new(self.program.clone(), Some(self.prefs.text_locale.clone()))
                {
                    vm.set_profile(profile);
                    self.vm = vm;
                    self.screen = Screen::Story;
                    self.step_vm();
                }
            }
            UiAction::Continue => {
                self.try_resume_latest();
            }
            UiAction::Advance => {
                if self.screen != Screen::Story {
                    return;
                }
                if self.interface_hidden {
                    self.interface_hidden = false;
                    return;
                }
                self.auto_elapsed_us = 0;
                let was_finished = self.vm.dialogue.as_ref().is_some_and(|d| d.finished);
                let stop_voice = !self.prefs.continue_voice_on_advance;
                let intents = self.vm.advance(stop_voice);
                self.handle_intents(intents);
                self.step_vm();
                if was_finished {
                    self.lines_since_autosave = self.lines_since_autosave.saturating_add(1);
                    if self.lines_since_autosave >= AUTOSAVE_LINE_INTERVAL {
                        self.emit_autosave();
                    }
                }
            }
            UiAction::Choose(idx) => {
                if self.screen != Screen::Story {
                    return;
                }
                self.auto_elapsed_us = 0;
                let intents = self.vm.choose(idx as usize);
                self.handle_intents(intents);
                self.step_vm();
                self.emit_autosave();
            }
            UiAction::CancelChoice => {
                if self.screen == Screen::Story {
                    self.vm.cancel_choice();
                    self.step_vm();
                }
            }
            UiAction::ToggleAuto => {
                self.auto = !self.auto;
                if self.auto {
                    self.skip = false;
                }
                self.auto_elapsed_us = 0;
            }
            UiAction::ToggleSkip => {
                self.skip = !self.skip;
                if self.skip {
                    self.auto = false;
                } else if self.screen == Screen::Story {
                    self.request_active_and_lookahead_assets();
                }
                self.skip_elapsed_us = 0;
            }
            UiAction::HoldSkip(held) => {
                let was_held = self.hold_skip;
                self.hold_skip = held;
                self.skip_elapsed_us = 0;
                if was_held && !held && !self.skip && self.screen == Screen::Story {
                    self.request_active_and_lookahead_assets();
                }
            }
            UiAction::ToggleHideUi => {
                if self.screen == Screen::Story {
                    self.interface_hidden = !self.interface_hidden;
                    if self.interface_hidden {
                        self.auto = false;
                        self.skip = false;
                    }
                }
            }
            UiAction::Rollback => {
                if self.screen == Screen::Story {
                    // Pop current checkpoint, restore previous one in O(1) without cloning 500 history strings!
                    if self.checkpoints.len() >= 2 {
                        let _ = self.checkpoints.pop_back();
                        if let Some(prev) = self.checkpoints.back().cloned() {
                            self.restore_checkpoint_internal(prev);
                        }
                    }
                }
            }
            UiAction::OpenMenu => {
                if self.screen == Screen::Story {
                    self.flush_pending_saves();
                    self.auto = false;
                    self.skip = false;
                    self.return_screen = Screen::Story;
                    self.screen = Screen::Menu;
                    self.restart_overlay_anim();
                    self.commands.push(HostCommand::PauseAudioBus {
                        bus: AudioBus::Voice,
                        paused: true,
                    });
                }
            }
            UiAction::CloseOverlay => {
                let target = if self.screen == Screen::Menu {
                    Screen::Story
                } else if self.return_screen == Screen::Title {
                    Screen::Title
                } else {
                    self.return_screen
                };
                self.screen = target;
                if self.screen == Screen::Story {
                    self.commands.push(HostCommand::PauseAudioBus {
                        bus: AudioBus::Voice,
                        paused: false,
                    });
                    if matches!(self.vm.wait, WaitReason::Modal(_)) {
                        self.vm.notify_modal_closed();
                        self.step_vm();
                    }
                }
            }
            UiAction::OpenSaves { save_mode } => {
                if self.frozen_vm.is_some() {
                    return;
                }
                if matches!(self.screen, Screen::Title | Screen::Story | Screen::Menu) {
                    self.return_screen = self.screen;
                }
                self.saves_page = 0;
                self.screen = Screen::Saves { save_mode };
                self.restart_overlay_anim();
            }
            UiAction::OpenSettings => {
                if matches!(self.screen, Screen::Title | Screen::Story | Screen::Menu) {
                    self.return_screen = self.screen;
                }
                self.screen = Screen::Settings;
                self.restart_overlay_anim();
            }
            UiAction::OpenHistory => {
                if matches!(self.screen, Screen::Story | Screen::Menu) {
                    self.return_screen = self.screen;
                }
                self.history_scroll = 0;
                self.screen = Screen::History;
                self.restart_overlay_anim();
            }
            UiAction::OpenGallery => {
                if matches!(self.screen, Screen::Title | Screen::Story | Screen::Menu) {
                    self.return_screen = self.screen;
                }
                self.gallery_page = 0;
                self.viewing_cg = None;
                self.screen = Screen::Gallery;
                self.restart_overlay_anim();
            }
            UiAction::SaveSlot(slot) => {
                if self.frozen_vm.is_some() {
                    return;
                }
                if self.slots.iter().any(|s| s.slot == slot && s.exists) {
                    self.pending_confirm = Some(ConfirmKind::OverwriteSlot(slot));
                    self.restart_overlay_anim();
                    return;
                }
                self.commit_save_slot(slot);
            }
            UiAction::LoadSlot(slot) => {
                self.pending_confirm = None;
                self.commands.push(HostCommand::RequestLoadSlot(slot));
            }
            UiAction::DeleteSlot(slot) => {
                if self.slots.iter().any(|s| s.slot == slot && s.exists) {
                    self.pending_confirm = Some(ConfirmKind::DeleteSlot(slot));
                    self.restart_overlay_anim();
                }
            }
            UiAction::ConfirmPending => {
                if let Some(kind) = self.pending_confirm.take() {
                    match kind {
                        ConfirmKind::OverwriteSlot(slot) => self.commit_save_slot(slot),
                        ConfirmKind::DeleteSlot(slot) => self.commit_delete_slot(slot),
                    }
                }
            }
            UiAction::CancelPending => {
                self.pending_confirm = None;
            }
            UiAction::QuickSave => {
                if self.frozen_vm.is_some() || (self.screen != Screen::Story && self.screen != Screen::Menu) {
                    return;
                }
                self.flush_pending_saves();
                let snap = self.vm.snapshot(self.unix_clock);
                self.quicksave = Some(SlotSummary {
                    slot: u32::MAX - 1,
                    exists: true,
                    summary: snap.summary.clone(),
                    saved_at_unix: snap.saved_at_unix,
                    has_thumb: self.quicksave.as_ref().is_some_and(|q| q.has_thumb),
                });
                self.commands.push(HostCommand::SaveQuick {
                    snapshot: Box::new(snap),
                });
                let msg = self.messages.get(&self.prefs.ui_locale, "quick-saved");
                self.show_toast(msg, 1800);
            }
            UiAction::QuickLoad => {
                if self.frozen_vm.is_some() {
                    return;
                }
                if self.quicksave.as_ref().is_some_and(|q| q.exists) {
                    self.commands.push(HostCommand::RequestLoadQuick);
                } else {
                    let msg = self.messages.get(&self.prefs.ui_locale, "no-quick-save");
                    self.show_toast(msg, 1800);
                }
            }
            UiAction::LoadAutoSave => {
                if self.frozen_vm.is_some() {
                    return;
                }
                if self.autosave.as_ref().is_some_and(|a| a.exists) {
                    self.commands.push(HostCommand::RequestLoadAuto);
                }
            }
            UiAction::ReplayVoice => {
                if let Some(asset) = self.vm.dialogue.as_ref().and_then(|d| d.voice) {
                    self.play_preview_voice(asset);
                } else {
                    let msg = self.messages.get(&self.prefs.ui_locale, "voice-unavailable");
                    self.show_toast(msg, 1500);
                }
            }
            UiAction::ReplayHistoryVoice(asset) => {
                self.play_preview_voice(asset);
            }
            UiAction::SetMasterVolume(v) => {
                self.prefs.master_volume = v.clamp(0.0, 1.0);
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::ToggleAutoWaitVoice => {
                self.prefs.auto_wait_voice = !self.prefs.auto_wait_voice;
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::ToggleContinueVoiceOnAdvance => {
                self.prefs.continue_voice_on_advance = !self.prefs.continue_voice_on_advance;
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::ToggleBilingual => {
                self.prefs.bilingual = !self.prefs.bilingual;
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::GalleryPage(delta) => {
                let max_page = self.program.ui.gallery.len().saturating_sub(1);
                if delta > 0 {
                    self.gallery_page = self.gallery_page.saturating_add(delta as usize).min(max_page);
                } else {
                    self.gallery_page = self
                        .gallery_page
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
            UiAction::ViewCg { item, variant } => {
                if let Some(gi) = self.program.ui.gallery.get(item as usize) {
                    let count = if !gi.cg_assets.is_empty() {
                        gi.cg_assets.len()
                    } else if gi.thumb.is_some() {
                        1
                    } else {
                        0
                    };
                    if count > 0 {
                        let v = (variant as usize).min(count - 1);
                        let asset = gi
                            .cg_assets
                            .get(v)
                            .copied()
                            .or(gi.thumb);
                        if let Some(id) = asset {
                            self.request_asset(id, true);
                        }
                        if self.viewing_cg != Some((item as usize, v)) {
                            self.reset_cg_view();
                        }
                        self.viewing_cg = Some((item as usize, v));
                    }
                }
            }
            UiAction::CloseCgViewer => {
                self.viewing_cg = None;
                self.reset_cg_view();
            }
            UiAction::SettingsTab(tab) => {
                self.settings_tab = tab.min(2);
            }
            UiAction::SetDialogueOpacity(v) => {
                self.prefs.dialogue_opacity = v.clamp(0.45, 1.0);
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::ToggleAmbientBackdrop => {
                self.prefs.ambient_backdrop = !self.prefs.ambient_backdrop;
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::ToggleImageSharpening => {
                self.prefs.image_sharpening = !self.prefs.image_sharpening;
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::ToggleTextOutline => {
                self.prefs.text_outline = !self.prefs.text_outline;
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::CycleUiStyleMode => {
                self.prefs.ui_style_mode = self.prefs.ui_style_mode.next();
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
                let loc = self.prefs.ui_locale.clone();
                let mode_name = self.messages.get(&loc, self.prefs.ui_style_mode.ftl_key());
                let label = self.messages.get(&loc, "ui-style-mode");
                self.show_toast(format!("{label}: {mode_name}"), 1800);
            }
            UiAction::ToggleEcoMode => {
                self.prefs.eco_mode = !self.prefs.eco_mode;
                if self.prefs.eco_mode {
                    self.wait_pulse_us = WAIT_PULSE_DURATION_US;
                    self.ui_time_us = 0;
                    self.hover_anim_us = HOVER_ANIM_US;
                }
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
                let loc = self.prefs.ui_locale.clone();
                let label = self.messages.get(&loc, "eco-mode");
                let state = self
                    .messages
                    .get(&loc, if self.prefs.eco_mode { "on" } else { "off" });
                self.show_toast(format!("{label}: {state}"), 1800);
            }
            UiAction::ReturnShelf => {
                self.emit_autosave();
                self.flush_pending_saves();
                self.stop_all_audio();
                self.commands.push(HostCommand::ReturnToShelf);
            }
            UiAction::ExportSaves => {
                self.flush_pending_saves();
                self.commands.push(HostCommand::ExportSaves);
            }
            UiAction::ImportSaves => {
                self.commands.push(HostCommand::ImportSaves);
            }
            UiAction::StartReplay(func) => {
                if self.frozen_vm.is_some() {
                    return;
                }
                let parent_screen = self.screen;
                let profile = self.vm.profile.clone();
                if let Ok(mut replay_vm) = Vm::new_at(
                    self.program.clone(),
                    func,
                    Some(self.prefs.text_locale.clone()),
                ) {
                    replay_vm.set_profile(profile);
                    let old_vm = std::mem::replace(&mut self.vm, replay_vm);
                    self.frozen_vm = Some((old_vm, parent_screen));
                    self.stop_all_audio();
                    self.screen = Screen::Story;
                    self.step_vm();
                }
            }
            UiAction::ExitReplay => {
                if let Some((parent_vm, parent_screen)) = self.frozen_vm.take() {
                    self.stop_all_audio();
                    self.vm = parent_vm;
                    self.screen = parent_screen;
                    self.request_active_and_lookahead_assets();
                }
            }
            UiAction::ReturnTitle => {
                self.emit_autosave();
                self.flush_pending_saves();
                self.stop_all_audio();
                self.frozen_vm = None;
                self.pending_confirm = None;
                self.viewing_cg = None;
                self.auto = false;
                self.skip = false;
                self.interface_hidden = false;
                self.screen = Screen::Title;
                self.request_title_assets();
            }
            UiAction::SetVolume { bus, value } => {
                let v = value.clamp(0.0, 1.0);
                match bus {
                    AudioBus::Bgm => self.prefs.bgm_volume = v,
                    AudioBus::Voice => self.prefs.voice_volume = v,
                    AudioBus::Sfx => self.prefs.sfx_volume = v,
                }
                // Host mixers apply master & bus gains via SavePreferences; keep channel gain un-attenuated twice.
                for ch in &self.vm.audio {
                    if ch.bus == bus {
                        let gain = ch.gain
                            * ch.envelope
                            * self.channel_character_gain(bus, ch.character.as_deref());
                        self.commands.push(HostCommand::SetAudioGain {
                            channel: ch.channel,
                            gain,
                        });
                    }
                }
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::SetTextSpeed(speed) => {
                self.prefs.text_speed = speed.clamp(0.0, 8.0);
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::SetFontScale(scale) => {
                self.prefs.font_scale = scale.clamp(0.75, 1.50);
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::SetAutoDelay(ms) => {
                self.prefs.auto_delay_ms = ms.min(15_000);
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::ToggleSkipUnread => {
                self.prefs.skip_unread = !self.prefs.skip_unread;
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::ToggleReducedMotion => {
                self.prefs.reduced_motion = !self.prefs.reduced_motion;
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::SetLocale(loc) => {
                self.prefs.ui_locale = loc.clone();
                self.prefs.text_locale = loc.clone();
                self.vm.set_locale(loc);
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::ScrollHistory(delta) => {
                let max_scroll = self.vm.history.len().saturating_sub(1);
                if delta > 0 {
                    self.history_scroll = self
                        .history_scroll
                        .saturating_add(delta as usize)
                        .min(max_scroll);
                } else {
                    self.history_scroll = self
                        .history_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
            UiAction::SavesPage(delta) => {
                let max_page = self.slots.len().saturating_sub(1);
                if delta > 0 {
                    self.saves_page = self.saves_page.saturating_add(delta as usize).min(max_page);
                } else {
                    self.saves_page = self
                        .saves_page
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
            UiAction::TogglePortraitReader => {
                self.prefs.portrait_reader = !self.prefs.portrait_reader;
                self.commands
                    .push(HostCommand::SavePreferences(self.prefs.clone()));
            }
            UiAction::Quit => {
                self.emit_autosave();
                self.flush_pending_saves();
                self.commands.push(HostCommand::Quit);
            }
        }
    }

    fn commit_save_slot(&mut self, slot: u32) {
        self.flush_pending_saves();
        let snap = self.vm.snapshot(self.unix_clock);
        if let Some(entry) = self.slots.iter_mut().find(|s| s.slot == slot) {
            entry.exists = true;
            entry.summary = snap.summary.clone();
            entry.saved_at_unix = snap.saved_at_unix;
        }
        self.commands.push(HostCommand::SaveSlot {
            slot,
            snapshot: Box::new(snap),
        });
        let msg = self.messages.get(&self.prefs.ui_locale, "saved");
        self.show_toast(msg, 1800);
    }

    fn commit_delete_slot(&mut self, slot: u32) {
        if let Some(entry) = self.slots.iter_mut().find(|s| s.slot == slot) {
            entry.exists = false;
            entry.summary.clear();
            entry.saved_at_unix = 0;
            entry.has_thumb = false;
        }
        self.commands.push(HostCommand::DeleteSlot(slot));
    }

    fn play_preview_voice(&mut self, asset: AssetId) {
        self.request_asset(asset, true);
        self.commands.push(HostCommand::PauseAudioBus {
            bus: AudioBus::Voice,
            paused: false,
        });
        self.commands.push(HostCommand::StopAudio {
            channel: VOICE_REPLAY_CHANNEL,
            fade_out_us: 0,
        });
        self.commands.push(HostCommand::PlayAudio {
            channel: VOICE_REPLAY_CHANNEL,
            bus: AudioBus::Voice,
            asset,
            gain: 1.0,
            looped: false,
            fade_in_us: 0,
            loop_region: None,
            character: None,
        });
    }

    /// Applies a loaded save snapshot returned by the host storage layer.
    pub fn apply_loaded_snapshot(&mut self, snapshot: SaveSnapshot) {
        self.flush_pending_saves();
        self.restore_snapshot_internal(snapshot);
        self.checkpoints.clear();
        self.checkpoints.push_back(self.vm.checkpoint());
        self.screen = Screen::Story;
        self.dirty = true;
    }

    fn restore_checkpoint_internal(&mut self, cp: RollbackCheckpoint) {
        self.stop_all_audio();
        let intents = self.vm.restore_checkpoint(cp);
        self.auto = false;
        self.skip = false;
        self.interface_hidden = false;
        self.handle_intents(intents);
        self.request_active_and_lookahead_assets();
    }

    fn restore_snapshot_internal(&mut self, snapshot: SaveSnapshot) {
        self.stop_all_audio();
        let profile = self.vm.profile.clone();
        match Vm::restore(self.program.clone(), snapshot, profile) {
            Ok((vm, intents)) => {
                self.vm = vm;
                self.auto = false;
                self.skip = false;
                self.interface_hidden = false;
                self.handle_intents(intents);
                self.request_active_and_lookahead_assets();
            }
            Err(e) => {
                self.show_toast(format!("Load failed: {}", e.message), 3500);
            }
        }
    }

    /// Advances active story/UI timers by `delta_us` microseconds.
    pub fn tick(&mut self, delta_us: u64) {
        if self.paused_by_host || delta_us == 0 {
            return;
        }
        // Distinguish long sleep-safe timers (Auto / Toast / Wait / Profile flush, up to 6s)
        // from continuous visual animation ticks (capped to 250ms against OS suspend hitches).
        let timer_dt = delta_us.min(6_000_000);
        let anim_dt = delta_us.min(250_000);

        // Debounced persistent profile flush (batches rapid read_texts updates into 1 write per 5s).
        if self.profile_dirty {
            self.profile_dirty_elapsed_us =
                self.profile_dirty_elapsed_us.saturating_add(timer_dt);
            if self.profile_dirty_elapsed_us >= PROFILE_FLUSH_INTERVAL_US {
                self.flush_pending_saves();
            }
        }

        if let Some((_, rem)) = &mut self.toast {
            self.toast_elapsed_us = self.toast_elapsed_us.saturating_add(timer_dt);
            if timer_dt >= *rem {
                self.toast = None;
                self.toast_elapsed_us = 0;
                self.dirty = true;
            } else {
                *rem -= timer_dt;
                if !self.prefs.reduced_motion
                    && !self.prefs.eco_mode
                    && (self.toast_elapsed_us <= TOAST_FADE_US + anim_dt || *rem <= TOAST_FADE_US)
                {
                    self.dirty = true;
                }
            }
        }

        if !self.prefs.reduced_motion && self.overlay_anim_us < OVERLAY_ANIM_US {
            self.overlay_anim_us = self
                .overlay_anim_us
                .saturating_add(anim_dt)
                .min(OVERLAY_ANIM_US);
            self.dirty = true;
        }

        if !self.prefs.reduced_motion && self.hover_anim_us < HOVER_ANIM_US {
            self.hover_anim_us = self
                .hover_anim_us
                .saturating_add(anim_dt)
                .min(HOVER_ANIM_US);
            self.dirty = true;
        }

        if !self.prefs.reduced_motion && self.screen == Screen::Story {
            let has_choice = self.vm.choice.is_some();
            if has_choice && !self.last_choice_active {
                self.choice_anim_us = 0;
                self.dirty = true;
            }
            self.last_choice_active = has_choice;
            if self.choice_anim_us < CHOICE_ANIM_US {
                self.choice_anim_us = self
                    .choice_anim_us
                    .saturating_add(anim_dt)
                    .min(CHOICE_ANIM_US);
                self.dirty = true;
            }
        } else {
            self.last_choice_active = self.vm.choice.is_some();
        }

        if self.screen == Screen::Story {
            let cur_dlg = self
                .vm
                .dialogue
                .as_ref()
                .map(|d| (d.text_id, d.full_text.len(), d.finished));
            let cur_key = cur_dlg.map(|(id, len, _)| (id, len));
            if cur_key != self.last_dialogue_key {
                let had_prev = self.last_dialogue_key.is_some();
                self.last_dialogue_key = cur_key;
                self.dialogue_epoch = self.dialogue_epoch.wrapping_add(1);
                self.wait_pulse_us = 0;
                self.ui_time_us = 0;
                // Only apply subtle line-swap fade when instantly displaying a subsequent line
                // (typewriter lines already reveal smoothly and keep sub-grapheme dirty-gating intact!).
                if !self.prefs.reduced_motion
                    && !self.prefs.eco_mode
                    && had_prev
                    && cur_dlg.is_some_and(|(_, _, fin)| fin)
                    && !self.skip
                    && !self.hold_skip
                {
                    self.dialogue_fade_us = 0;
                    self.dirty = true;
                } else {
                    self.dialogue_fade_us = DIALOGUE_FADE_US;
                }
            }
            if !self.prefs.reduced_motion && self.dialogue_fade_us < DIALOGUE_FADE_US {
                self.dialogue_fade_us = self
                    .dialogue_fade_us
                    .saturating_add(anim_dt)
                    .min(DIALOGUE_FADE_US);
                self.dirty = true;
            }

            let waiting_now = self.is_waiting_dialogue_click();
            if waiting_now && !self.was_waiting_click {
                self.wait_pulse_us = 0;
                self.ui_time_us = 0;
            }
            self.was_waiting_click = waiting_now;

            // Breathe click-to-continue indicator briefly (~2.4s at ~18Hz), then rest cleanly
            // at bob=0 so idle reading reaches true 0% CPU & 0% GPU!
            if !self.prefs.reduced_motion && !self.prefs.eco_mode && waiting_now {
                if self.wait_pulse_us < WAIT_PULSE_DURATION_US {
                    let prev_step = self.ui_time_us / WAIT_PULSE_STEP_US;
                    self.wait_pulse_us = self
                        .wait_pulse_us
                        .saturating_add(anim_dt)
                        .min(WAIT_PULSE_DURATION_US);
                    if self.wait_pulse_us >= WAIT_PULSE_DURATION_US {
                        // Snap cleanly to neutral resting pose and enter deep idle sleep.
                        self.ui_time_us = 0;
                        self.dirty = true;
                    } else {
                        self.ui_time_us = self.ui_time_us.wrapping_add(anim_dt);
                        if self.ui_time_us / WAIT_PULSE_STEP_US != prev_step {
                            self.dirty = true;
                        }
                    }
                }
            } else if self.ui_time_us != 0 {
                self.ui_time_us = 0;
            }
        }

        if self.has_urgent_image_loading() {
            let was_vis = self.urgent_loading_us >= LOADING_INDICATOR_DELAY_US;
            self.urgent_loading_us = self.urgent_loading_us.saturating_add(timer_dt);
            if !was_vis && self.urgent_loading_us >= LOADING_INDICATOR_DELAY_US {
                self.dirty = true;
            }
        } else if self.urgent_loading_us > 0 {
            let was_vis = self.urgent_loading_us >= LOADING_INDICATOR_DELAY_US;
            self.urgent_loading_us = 0;
            if was_vis {
                self.dirty = true;
            }
        }

        // Long-press HoldSkip detection (only on Story blank area without active choices).
        let can_long_press = self.screen == Screen::Story
            && !self.interface_hidden
            && self.vm.choice.is_none()
            && !self.pressed_started_on_hit;
        if let Some(act) = self.gestures.tick(timer_dt, can_long_press) {
            self.apply_touch_action(act);
        }

        if self.screen != Screen::Story {
            return;
        }

        let pure_timer_wait = matches!(self.vm.wait, WaitReason::Timer { .. })
            && self.vm.tweens.is_empty()
            && self.vm.shakes.is_empty()
            && self.vm.transition.is_none()
            && self.vm.window_reveal.is_none();
        let vm_dt = if pure_timer_wait { timer_dt } else { anim_dt };
        let (intents, visual_changed) = self.vm.tick_with_dirty(
            vm_dt,
            self.prefs.text_speed,
            self.prefs.reduced_motion || self.prefs.eco_mode,
        );
        self.handle_intents(intents);
        if visual_changed {
            self.dirty = true;
        }

        if self.vm.wait == WaitReason::None && self.vm.outcome.is_none() {
            self.step_vm();
        }

        // Skip mode (hold Ctrl or toggled Skip)
        let skipping = self.hold_skip || self.skip;
        if skipping && self.vm.choice.is_none() {
            let can_skip = self.prefs.skip_unread
                || self.vm.is_current_dialogue_read()
                || self.vm.wait != WaitReason::Dialogue;
            if can_skip {
                self.skip_elapsed_us = self.skip_elapsed_us.saturating_add(anim_dt);
                if self.skip_elapsed_us >= SKIP_INTERVAL_US {
                    self.skip_elapsed_us = 0;
                    let intents = self.vm.advance(true);
                    self.handle_intents(intents);
                    if self.vm.wait == WaitReason::Dialogue
                        && self.vm.dialogue.as_ref().is_some_and(|d| d.finished)
                    {
                        let intents2 = self.vm.advance(true);
                        self.handle_intents(intents2);
                    }
                    self.step_vm();
                    self.dirty = true;
                }
            }
        } else if self.auto && self.vm.wait == WaitReason::Dialogue {
            let dlg_done = self.vm.dialogue.as_ref().is_some_and(|d| d.finished);
            let voice_ready = !self.prefs.auto_wait_voice || self.active_voice_channels.is_empty();
            if dlg_done && voice_ready {
                self.auto_elapsed_us = self.auto_elapsed_us.saturating_add(timer_dt);
                let target_us = self.prefs.auto_delay_ms as u64 * 1000;
                if self.auto_elapsed_us >= target_us {
                    self.auto_elapsed_us = 0;
                    let intents = self.vm.advance(true);
                    self.handle_intents(intents);
                    self.step_vm();
                    self.dirty = true;
                }
            }
        }
    }

    /// Rebuilds the `DrawPacket` ONLY when `dirty` is true (zero safety-frame polling!).
    /// Returns `Some(&DrawPacket)` if visual state actually changed and GPU should draw.
    pub fn prepare_frame(&mut self, width: f32, height: f32) -> Option<&DrawPacket> {
        let size_changed = (self.last_packet.width - width).abs() > 0.5
            || (self.last_packet.height - height).abs() > 0.5;
        if !self.dirty && !size_changed {
            return None;
        }
        // Keep dialogue_epoch in sync immediately after dispatch/load/rollback even before next tick.
        let cur_dlg_key = self
            .vm
            .dialogue
            .as_ref()
            .map(|d| (d.text_id, d.full_text.len()));
        if cur_dlg_key != self.last_dialogue_key {
            self.last_dialogue_key = cur_dlg_key;
            self.dialogue_epoch = self.dialogue_epoch.wrapping_add(1);
            self.wait_pulse_us = 0;
            self.ui_time_us = 0;
        }
        let overlay_anim = if self.prefs.reduced_motion {
            1.0
        } else {
            (self.overlay_anim_us as f32 / OVERLAY_ANIM_US as f32).clamp(0.0, 1.0)
        };
        let toast_anim = if self.prefs.reduced_motion {
            1.0
        } else if let Some((_, rem)) = &self.toast {
            let enter = (self.toast_elapsed_us as f32 / TOAST_FADE_US as f32).clamp(0.0, 1.0);
            let exit = (*rem as f32 / TOAST_FADE_US as f32).clamp(0.0, 1.0);
            enter.min(exit)
        } else {
            1.0
        };
        let dialogue_fade = if self.prefs.reduced_motion || self.skip || self.hold_skip {
            1.0
        } else {
            (self.dialogue_fade_us as f32 / DIALOGUE_FADE_US as f32).clamp(0.0, 1.0)
        };
        let hover_anim = if self.prefs.reduced_motion {
            1.0
        } else {
            (self.hover_anim_us as f32 / HOVER_ANIM_US as f32).clamp(0.0, 1.0)
        };
        let choice_anim = if self.prefs.reduced_motion {
            1.0
        } else {
            (self.choice_anim_us as f32 / CHOICE_ANIM_US as f32).clamp(0.0, 1.0)
        };
        let ctx = UiContext {
            vm: &self.vm,
            screen: self.screen,
            prefs: &self.prefs,
            auto: self.auto,
            skip: self.skip,
            hold_skip: self.hold_skip,
            interface_hidden: self.interface_hidden,
            replay_active: self.frozen_vm.is_some(),
            slots: &self.slots,
            autosave: self.autosave.as_ref(),
            quicksave: self.quicksave.as_ref(),
            saves_page: self.saves_page,
            history_scroll: self.history_scroll,
            gallery_page: self.gallery_page,
            viewing_cg: self.viewing_cg,
            cg_zoom: self.cg_zoom,
            cg_pan: self.cg_pan,
            settings_tab: self.settings_tab,
            pending_confirm: self.pending_confirm,
            host_has_shelf: self.host_has_shelf,
            loading_visible: self.urgent_loading_us >= LOADING_INDICATOR_DELAY_US,
            tz_offset_mins: self.tz_offset_mins,
            hovered_hit: self.hovered_hit,
            pressed_hit: self.pressed_hit,
            safe_insets: self.safe_insets,
            input_mode: self.input_mode,
            portrait_reader: self.prefs.portrait_reader,
            toast: self.toast.as_ref().map(|(m, _)| m.as_str()),
            overlay_anim,
            toast_anim,
            dialogue_fade,
            ui_time_s: (self.ui_time_us as f64 / 1_000_000.0) as f32,
            hud_toolbar_hovered: self.hud_toolbar_hovered,
            host_top_left_pad: self.host_top_left_pad,
            host_top_right_pad: self.host_top_right_pad,
            resolved_style: Some(&self.resolved_style),
            hover_anim,
            choice_anim,
        };
        let next = nir_ui::project(&ctx, width, height, &self.messages);
        let assets_updated = self.last_rendered_asset_epoch != self.asset_epoch;
        let changed = size_changed || assets_updated || !self.last_packet.visual_eq(&next);
        self.last_packet = next;
        self.last_rendered_asset_epoch = self.asset_epoch;
        self.dirty = false;
        changed.then_some(&self.last_packet)
    }

    fn step_vm(&mut self) {
        match self.vm.step(10_000) {
            Ok(intents) => {
                self.handle_intents(intents);
                if self.vm.outcome.is_some() {
                    if self.frozen_vm.is_some() {
                        self.dispatch(UiAction::ExitReplay);
                    } else {
                        self.auto = false;
                        self.skip = false;
                        self.screen = Screen::Ended;
                    }
                }
                self.request_active_and_lookahead_assets();
                self.dirty = true;
            }
            Err(e) => {
                self.auto = false;
                self.skip = false;
                self.show_toast(format!("Script error: {e}"), 5000);
            }
        }
    }

    fn handle_intents(&mut self, intents: Vec<VmIntent>) {
        for intent in intents {
            match intent {
                VmIntent::PlayAudio {
                    channel,
                    bus,
                    asset,
                    gain,
                    looped,
                    fade_in_us,
                    loop_region,
                    character,
                } => {
                    if bus == AudioBus::Voice {
                        self.active_voice_channels.insert(channel);
                    }
                    let eff = gain * self.channel_character_gain(bus, character.as_deref());
                    self.request_asset(asset, true);
                    self.commands.push(HostCommand::PlayAudio {
                        channel,
                        bus,
                        asset,
                        gain: eff,
                        looped,
                        fade_in_us,
                        loop_region,
                        character,
                    });
                }
                VmIntent::StopAudio {
                    channel,
                    fade_out_us,
                } => {
                    self.active_voice_channels.remove(&channel);
                    self.commands.push(HostCommand::StopAudio {
                        channel,
                        fade_out_us,
                    });
                }
                VmIntent::SetAudioGain { channel, gain } => {
                    let (bus, ch_name) = self
                        .vm
                        .audio
                        .iter()
                        .find(|c| c.channel == channel)
                        .map_or((AudioBus::Bgm, None), |c| (c.bus, c.character.as_deref()));
                    let eff = gain * self.channel_character_gain(bus, ch_name);
                    self.commands
                        .push(HostCommand::SetAudioGain { channel, gain: eff });
                }
                VmIntent::PauseAudioBus { bus, paused } => {
                    self.commands
                        .push(HostCommand::PauseAudioBus { bus, paused });
                }
                VmIntent::PlayVideo { asset, skippable } => {
                    self.request_asset(asset, true);
                    self.commands
                        .push(HostCommand::PlayVideo { asset, skippable });
                }
                VmIntent::StopVideo => {
                    self.commands.push(HostCommand::StopVideo);
                }
                VmIntent::OpenModal(kind) => {
                    if self.frozen_vm.is_some()
                        && matches!(kind, ModalKind::Replay | ModalKind::Gallery)
                    {
                        self.dispatch(UiAction::ExitReplay);
                        continue;
                    }
                    self.auto = false;
                    self.skip = false;
                    self.return_screen = Screen::Story;
                    self.screen = match kind {
                        ModalKind::Load => Screen::Saves { save_mode: false },
                        ModalKind::Save => Screen::Saves { save_mode: true },
                        ModalKind::Gallery | ModalKind::Replay => Screen::Gallery,
                    };
                }
                VmIntent::ProfileChanged => {
                    // Debounce profile writes (flushed every 5s or on menu/save/quit/suspend)
                    // instead of writing disk / IndexedDB on every dialogue line!
                    if self.frozen_vm.is_none() {
                        self.profile_dirty = true;
                    }
                }
                VmIntent::Checkpoint => {
                    if self.frozen_vm.is_none() {
                        if self.checkpoints.len() >= MAX_ROLLBACK_CHECKPOINTS {
                            self.checkpoints.pop_front();
                        }
                        self.checkpoints.push_back(self.vm.checkpoint());
                    }
                }
            }
        }
    }

    fn stop_all_audio(&mut self) {
        self.active_voice_channels.clear();
        self.commands.push(HostCommand::StopAudio {
            channel: VOICE_REPLAY_CHANNEL,
            fade_out_us: 0,
        });
        let channels: Vec<ChannelId> = self.vm.audio.iter().map(|c| c.channel).collect();
        for ch in channels {
            self.commands.push(HostCommand::StopAudio {
                channel: ch,
                fade_out_us: 0,
            });
        }
    }

    fn request_asset(&mut self, id: AssetId, urgent: bool) {
        if self.loaded_assets.contains(&id) {
            return;
        }
        match self.requested_assets.get(&id).copied() {
            None => {
                self.requested_assets.insert(id, urgent);
                self.commands.push(HostCommand::LoadAsset { id, urgent });
            }
            Some(false) if urgent => {
                // Promote an already-queued background prefetch to urgent!
                self.requested_assets.insert(id, true);
                self.commands
                    .push(HostCommand::LoadAsset { id, urgent: true });
            }
            _ => {}
        }
    }

    fn request_title_assets(&mut self) {
        let mut pinned = BTreeSet::new();
        if let Some(bg) = self
            .program
            .ui
            .title_screen
            .as_ref()
            .and_then(|s| s.background)
            .or(self.program.meta.title_bg)
        {
            pinned.insert(bg);
            self.request_asset(bg, true);
        }
        if let Some(bgm) = self
            .program
            .ui
            .title_screen
            .as_ref()
            .and_then(|s| s.bgm)
            .or(self.program.meta.title_bgm)
        {
            pinned.insert(bgm);
            self.request_asset(bgm, true);
            self.commands.push(HostCommand::PlayAudio {
                channel: ChannelId(0),
                bus: AudioBus::Bgm,
                asset: bgm,
                gain: 1.0,
                looped: true,
                fade_in_us: 300_000,
                loop_region: None,
                character: None,
            });
        }
        let fonts: Vec<AssetId> = self
            .program
            .meta
            .ui_fonts
            .iter()
            .chain(self.program.meta.text_fonts.iter())
            .copied()
            .collect();
        for f in fonts {
            pinned.insert(f);
            self.request_asset(f, true);
        }
        self.commands.push(HostCommand::HintPinnedAssets(pinned));
    }

    fn request_active_and_lookahead_assets(&mut self) {
        let mut pinned = self.vm.active_assets();
        for &id in &pinned {
            self.request_asset(id, true);
        }
        // Suppress speculative background lookahead while fast-skipping so transient CGs & voices
        // never flood HTTP Range requests, decode workers, or GPU LRU cache!
        if !(self.skip || self.hold_skip) {
            let lookahead_ops = if self.prefs.eco_mode || self.low_power_hint {
                10
            } else {
                32
            };
            for id in self.vm.predict_assets(lookahead_ops) {
                pinned.insert(id);
                self.request_asset(id, false);
            }
        }
        self.commands.push(HostCommand::HintPinnedAssets(pinned));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn test_prog() -> Arc<NirProgram> {
        Arc::new(NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.rt".into(),
                title: "Runtime Test".into(),
                version: "0.2".into(),
                stage_width: 1280,
                stage_height: 720,
                default_locale: "zh-Hans".into(),
                locales: vec!["zh-Hans".into(), "en".into()],
                entry_func: FuncId(0),
                save_slots: 6,
                default_reveal_us: 10_000,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec!["main".into(), "Aki".into(), "done".into()],
            vars: vec![],
            funcs: vec![FuncMeta {
                name: StrId(0),
                entry_pc: 0,
                param_count: 0,
                local_count: 0,
            }],
            labels: vec![],
            ops: vec![
                Op::Say {
                    speaker: Some(StrId(1)),
                    speaker_color: None,
                    text: TextId(0),
                    voice: None,
                    reveal_us: 0,
                    append: false,
                },
                Op::Say {
                    speaker: Some(StrId(1)),
                    speaker_color: None,
                    text: TextId(1),
                    voice: None,
                    reveal_us: 0,
                    append: false,
                },
                Op::End { outcome: StrId(2) },
            ],
            texts: BTreeMap::from([(
                "zh-Hans".into(),
                vec![TextEntry::plain("第一句"), TextEntry::plain("第二句")],
            )]),
            ui: UiTheme::default(),
            assets: vec![],
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        })
    }

    #[test]
    fn runtime_flow_save_rollback_and_dirty_gating() {
        let mut rt = Runtime::new(
            test_prog(),
            Preferences::default(),
            ProfileData::default(),
            vec![],
        )
        .unwrap();
        assert_eq!(rt.screen, Screen::Title);
        assert!(rt.prepare_frame(1280.0, 720.0).is_some());
        // Clean frame produces None (zero redundant GPU draws, no 16-frame polling!)
        assert!(rt.prepare_frame(1280.0, 720.0).is_none());

        rt.dispatch(UiAction::NewGame);
        assert_eq!(rt.screen, Screen::Story);
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "第一句");
        assert!(rt.prepare_frame(1280.0, 720.0).is_some());

        // Advance to line 2 — profile write should be debounced (NOT emitted per line!)
        rt.dispatch(UiAction::Advance); // reveal
        rt.dispatch(UiAction::Advance); // next
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "第二句");
        assert_eq!(rt.vm().history.len(), 2);
        let mid_cmds = rt.take_commands();
        assert!(
            !mid_cmds
                .iter()
                .any(|c| matches!(c, HostCommand::SaveProfile(_))),
            "SaveProfile must be debounced rather than written synchronously on every line"
        );

        // Opening menu or calling flush_pending_saves flushes the debounced profile write!
        rt.flush_pending_saves();
        let flushed = rt.take_commands();
        assert!(flushed
            .iter()
            .any(|c| matches!(c, HostCommand::SaveProfile(_))));

        // Save slot 0
        rt.dispatch(UiAction::SaveSlot(0));
        let cmds = rt.take_commands();
        let saved = cmds.into_iter().find_map(|c| match c {
            HostCommand::SaveSlot { snapshot, .. } => Some(*snapshot),
            _ => None,
        });
        assert!(saved.is_some());

        // Rollback returns to line 1 and truncates history back to 1 entry in O(1)!
        rt.dispatch(UiAction::Rollback);
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "第一句");
        assert_eq!(rt.vm().history.len(), 1);

        // Load slot 0 restores line 2 and its full 2-entry history
        rt.apply_loaded_snapshot(saved.unwrap());
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "第二句");
        assert_eq!(rt.vm().history.len(), 2);
    }

    #[test]
    fn dirty_gating_ignores_sub_grapheme_and_wait_ticks() {
        let mut rt = Runtime::new(
            test_prog(),
            Preferences::default(),
            ProfileData::default(),
            vec![],
        )
        .unwrap();
        rt.dispatch(UiAction::NewGame);
        assert!(rt.prepare_frame(1280.0, 720.0).is_some());
        // default_reveal_us = 10_000us; a 3_000us tick doesn't reveal a new grapheme -> no redraw!
        rt.tick(3_000);
        assert!(!rt.dirty);
        assert!(rt.prepare_frame(1280.0, 720.0).is_none());
        // Another 8_000us crosses 10_000us -> reveals 1 grapheme -> marks dirty!
        rt.tick(8_000);
        assert!(rt.dirty);
        assert!(rt.prepare_frame(1280.0, 720.0).is_some());
    }

    #[test]
    fn runtime_handles_open_modal_and_replay_exit_cleanly() {
        let prog = Arc::new(NirProgram {
            features: 0,
            meta: GameMeta {
                game_id: "org.nir.modal".into(),
                title: "Modal Test".into(),
                version: "1.0".into(),
                stage_width: 1280,
                stage_height: 720,
                default_locale: "en".into(),
                locales: vec!["en".into()],
                entry_func: FuncId(0),
                save_slots: 4,
                default_reveal_us: 0,
                title_bg: None,
                title_bgm: None,
                ui_fonts: vec![],
                text_fonts: vec![],
            },
            strings: vec!["main".into(), "replay".into(), "done".into()],
            vars: vec![],
            funcs: vec![
                FuncMeta {
                    name: StrId(0),
                    entry_pc: 0,
                    param_count: 0,
                    local_count: 0,
                },
                FuncMeta {
                    name: StrId(1),
                    entry_pc: 3,
                    param_count: 0,
                    local_count: 0,
                },
            ],
            labels: vec![],
            ops: vec![
                // main: OpenModal(Load) -> Say -> End
                Op::OpenModal(ModalKind::Load),
                Op::Say {
                    speaker: None,
                    speaker_color: None,
                    text: TextId(0),
                    voice: None,
                    reveal_us: 0,
                    append: false,
                },
                Op::End { outcome: StrId(2) },
                // replay func: Say -> OpenModal(Replay) -> should exit replay!
                Op::Say {
                    speaker: None,
                    speaker_color: None,
                    text: TextId(1),
                    voice: None,
                    reveal_us: 0,
                    append: false,
                },
                Op::OpenModal(ModalKind::Replay),
                Op::Ret { has_value: false },
            ],
            texts: BTreeMap::from([(
                "en".into(),
                vec![
                    TextEntry::plain("After modal"),
                    TextEntry::plain("Replay line"),
                ],
            )]),
            ui: UiTheme::default(),
            assets: vec![],
            asset_lookup: BTreeMap::new(),
            timelines: vec![],
            menus: vec![],
        });

        let mut rt =
            Runtime::new(prog, Preferences::default(), ProfileData::default(), vec![]).unwrap();
        rt.dispatch(UiAction::NewGame);
        assert_eq!(rt.screen, Screen::Saves { save_mode: false });
        rt.dispatch(UiAction::CloseOverlay);
        assert_eq!(rt.screen, Screen::Story);
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "After modal");

        // Start replay from gallery -> advance past replay line -> OpenModal(Replay) exits replay!
        rt.dispatch(UiAction::OpenGallery);
        assert_eq!(rt.screen, Screen::Gallery);
        rt.dispatch(UiAction::StartReplay(FuncId(1)));
        assert_eq!(rt.screen, Screen::Story);
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "Replay line");
        rt.dispatch(UiAction::Advance);
        assert_eq!(rt.screen, Screen::Gallery);
    }

    #[test]
    fn async_asset_and_font_loads_always_trigger_redraw_and_eviction_reloads() {
        let mut rt = Runtime::new(
            test_prog(),
            Preferences::default(),
            ProfileData::default(),
            vec![],
        )
        .unwrap();
        assert!(rt.prepare_frame(1280.0, 720.0).is_some());
        assert!(rt.prepare_frame(1280.0, 720.0).is_none());

        // Texture finishing decode asynchronously does NOT change DrawPacket fields,
        // yet prepare_frame MUST emit a redraw!
        rt.mark_asset_loaded(AssetId(7));
        assert!(rt.prepare_frame(1280.0, 720.0).is_some());
        assert!(rt.prepare_frame(1280.0, 720.0).is_none());

        // Fallback font load MUST also emit a redraw!
        rt.mark_fonts_changed();
        assert!(rt.prepare_frame(1280.0, 720.0).is_some());

        // Evicted assets are removed from loaded_assets so re-requesting emits LoadAsset again.
        rt.take_commands();
        rt.mark_assets_evicted(&[AssetId(7)]);
        rt.request_asset(AssetId(7), true);
        let cmds = rt.take_commands();
        assert!(cmds.iter().any(|c| matches!(
            c,
            HostCommand::LoadAsset {
                id: AssetId(7),
                urgent: true
            }
        )));

        // SetFontScale updates prefs cleanly and persists them.
        rt.dispatch(UiAction::SetFontScale(1.2));
        assert!((rt.prefs.font_scale - 1.2).abs() < 0.001);
        let pcmds = rt.take_commands();
        assert!(pcmds
            .iter()
            .any(|c| matches!(c, HostCommand::SavePreferences(_))));
    }

    #[test]
    fn touch_gestures_and_host_lifecycle_pause_work_end_to_end() {
        let mut rt = Runtime::new(
            test_prog(),
            Preferences::default(),
            ProfileData::default(),
            vec![],
        )
        .unwrap();
        rt.dispatch(UiAction::NewGame);
        assert!(rt.prepare_frame(390.0, 844.0).is_some());
        rt.take_commands();

        // 1. Clean single-finger tap on blank area advances dialogue (reveal -> next line)
        rt.touch_start(1, 180.0, 200.0);
        rt.touch_end(1, 182.0, 201.0); // finishes reveal
        rt.touch_start(2, 180.0, 200.0);
        rt.touch_end(2, 180.0, 200.0); // advances to line 2
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "第二句");
        assert_eq!(rt.input_mode, InputMode::Touch);

        // 2. Swipe right rolls back to line 1
        rt.touch_start(3, 80.0, 200.0);
        rt.touch_move(3, 160.0, 205.0);
        rt.touch_end(3, 180.0, 205.0);
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "第一句");

        // 3. Swipe up opens History, drag scrolls, two-finger tap closes overlay
        rt.touch_start(4, 180.0, 400.0);
        rt.touch_move(4, 180.0, 320.0);
        rt.touch_end(4, 180.0, 300.0);
        assert_eq!(rt.screen, Screen::History);

        rt.touch_start(5, 100.0, 300.0);
        rt.touch_start(6, 220.0, 300.0);
        rt.touch_end(5, 102.0, 302.0);
        rt.touch_end(6, 221.0, 299.0);
        assert_eq!(rt.screen, Screen::Story);

        // 4. Swipe down hides UI, tap restores UI without advancing
        rt.touch_start(7, 180.0, 200.0);
        rt.touch_end(7, 182.0, 300.0);
        assert!(rt.interface_hidden);
        rt.touch_start(8, 180.0, 200.0);
        rt.touch_end(8, 180.0, 200.0);
        assert!(!rt.interface_hidden);
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "第一句");

        // 5. Long-press (>= 420ms) enables HoldSkip + WakeLock, finger release stops HoldSkip
        rt.touch_start(9, 180.0, 200.0);
        assert!(rt.needs_clock());
        rt.tick(250_000);
        assert!(!rt.hold_skip);
        rt.tick(200_000);
        assert!(rt.hold_skip);
        assert!(rt.is_auto_or_skip_active());
        rt.touch_end(9, 180.0, 200.0);
        assert!(!rt.hold_skip);

        // 6. Host background pause pauses all 3 audio buses & clocks + emits SaveAuto; resume restores cleanly
        rt.take_commands();
        rt.set_host_paused(true);
        assert!(!rt.needs_clock());
        let paused_cmds = rt.take_commands();
        assert!(paused_cmds
            .iter()
            .any(|c| matches!(c, HostCommand::SaveAuto { .. })));
        assert_eq!(
            paused_cmds
                .iter()
                .filter(|c| matches!(c, HostCommand::PauseAudioBus { paused: true, .. }))
                .count(),
            3
        );
        rt.set_host_paused(false);
        let resumed_cmds = rt.take_commands();
        assert_eq!(
            resumed_cmds
                .iter()
                .filter(|c| matches!(c, HostCommand::PauseAudioBus { paused: false, .. }))
                .count(),
            3
        );
    }

    #[test]
    fn autosave_quicksave_confirm_wheel_voice_and_translation_patch_work() {
        let mut rt = Runtime::new(
            test_prog(),
            Preferences::default(),
            ProfileData::default(),
            vec![],
        )
        .unwrap();
        rt.set_unix_time(1_760_000_100);
        rt.set_host_has_shelf(true);
        rt.dispatch(UiAction::NewGame);
        rt.take_commands();

        // 1. Save into empty slot 0 commits immediately; overwriting occupied slot 0 asks for confirm!
        rt.dispatch(UiAction::SaveSlot(0));
        let cmds = rt.take_commands();
        assert!(cmds.iter().any(|c| matches!(c, HostCommand::SaveSlot { slot: 0, .. })));
        rt.dispatch(UiAction::SaveSlot(0));
        assert!(rt.take_commands().is_empty(), "overwrite should wait for confirm");
        rt.dispatch(UiAction::ConfirmPending);
        assert!(rt
            .take_commands()
            .iter()
            .any(|c| matches!(c, HostCommand::SaveSlot { slot: 0, .. })));

        // 2. Delete slot 0 also asks for confirm
        rt.dispatch(UiAction::DeleteSlot(0));
        assert!(rt.take_commands().is_empty(), "delete should wait for confirm");
        rt.dispatch(UiAction::CancelPending);
        assert!(rt.slots[0].exists);

        // 3. QuickSave & AutoSave + try_resume_latest picks newest timestamp
        rt.set_unix_time(1_760_000_200);
        rt.dispatch(UiAction::QuickSave);
        assert!(rt
            .take_commands()
            .iter()
            .any(|c| matches!(c, HostCommand::SaveQuick { .. })));
        rt.set_unix_time(1_760_000_300);
        rt.emit_autosave();
        assert!(rt
            .take_commands()
            .iter()
            .any(|c| matches!(c, HostCommand::SaveAuto { .. })));
        assert!(rt.try_resume_latest());
        assert!(rt
            .take_commands()
            .iter()
            .any(|c| matches!(c, HostCommand::RequestLoadAuto)));

        // 4. History wheel routing scrolls history instead of rolling back Story!
        rt.dispatch(UiAction::Advance);
        rt.dispatch(UiAction::Advance);
        rt.dispatch(UiAction::OpenHistory);
        rt.scroll_wheel(-100.0);
        assert_eq!(rt.screen, Screen::History);
        assert!(rt.history_scroll > 0);
        rt.dispatch(UiAction::CloseOverlay);

        // 5. External translation patch adds a new locale without rebuilding .nirpak!
        let applied = rt
            .load_translation_patch("ja", r#"{"texts":{"0":"最初の一行","1":"二行目"}}"#)
            .unwrap();
        assert_eq!(applied, 2);
        assert!(rt.program().meta.locales.contains(&"ja".to_owned()));
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "二行目");
        rt.dispatch(UiAction::ToggleBilingual);
        assert!(rt.prefs.bilingual);
    }

    #[test]
    fn mobile_ergonomics_slider_drag_backdrop_dismiss_edge_guard_and_cg_zoom_work() {
        let mut prog_owned = (*test_prog()).clone();
        prog_owned.assets.push(AssetMeta {
            id: AssetId(0),
            name: StrId(0),
            kind: AssetKind::Image,
            codec: MediaCodec::Png,
            width: 800,
            height: 600,
            trim: None,
            duration_us: 0,
            sample_rate: 0,
            channels: 0,
            loop_region: None,
            offset: 0,
            size: 16,
            delta: None,
        });
        prog_owned.ui.gallery.push(GalleryItem {
            id: StrId(0),
            title: StrId(0),
            unlock_flag: None,
            thumb: Some(AssetId(0)),
            cg_assets: vec![AssetId(0), AssetId(0)],
            replay_func: None,
        });
        let mut rt = Runtime::new(
            Arc::new(prog_owned),
            Preferences::default(),
            ProfileData::default(),
            vec![],
        )
        .unwrap();
        rt.set_input_mode(InputMode::Touch);
        rt.dispatch(UiAction::NewGame);
        rt.dispatch(UiAction::Advance); // reveal line 1
        rt.dispatch(UiAction::Advance); // advance to line 2
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "第二句");
        assert!(rt.prepare_frame(390.0, 844.0).is_some());

        // 1. Left-edge (< 24px) right swipe is guarded for OS back navigation and must NOT rollback!
        rt.touch_start(10, 8.0, 260.0);
        rt.touch_move(10, 90.0, 262.0);
        rt.touch_end(10, 120.0, 262.0);
        assert_eq!(rt.vm().dialogue.as_ref().unwrap().full_text, "第二句");

        // 2. Settings on narrow 390px phone exposes draggable sliders & backdrop tap closes overlay!
        rt.dispatch(UiAction::OpenSettings);
        assert!(rt.can_go_back());
        assert!(rt.prepare_frame(390.0, 844.0).is_some());
        let first_slider = rt.last_packet().sliders[0];
        let sx_right = first_slider.rect[0] + first_slider.rect[2] * 0.95;
        let sy_mid = first_slider.rect[1] + first_slider.rect[3] * 0.5;
        rt.touch_start(11, first_slider.rect[0] + 4.0, sy_mid);
        rt.touch_move(11, sx_right, sy_mid);
        rt.touch_end(11, sx_right, sy_mid);
        assert!(rt.prefs.text_speed >= 3.5, "slider drag should raise text_speed, got {}", rt.prefs.text_speed);
        assert!(rt.take_haptic_ms() > 0, "touch slider should emit subtle haptic");

        // Tapping dimmed backdrop outside modal panel dismisses Settings one-handedly!
        rt.touch_start(12, 3.0, 3.0);
        rt.touch_end(12, 3.0, 3.0);
        assert_eq!(rt.screen, Screen::Story);

        // 3. CG Viewer supports pinch zoom, pan, double-tap reset, and unzoomed swipe variant switch!
        rt.dispatch(UiAction::OpenGallery);
        rt.dispatch(UiAction::ViewCg { item: 0, variant: 0 });
        assert!(rt.can_go_back());
        assert!(rt.prepare_frame(390.0, 844.0).is_some());

        // Pinch zoom in
        rt.touch_start(20, 150.0, 360.0);
        rt.touch_start(21, 240.0, 360.0);
        rt.touch_move(21, 310.0, 360.0);
        rt.touch_end(20, 150.0, 360.0);
        rt.touch_end(21, 310.0, 360.0);
        assert!(rt.cg_zoom() > 1.2, "pinch should zoom in CG, got {}", rt.cg_zoom());

        // Pan while zoomed
        rt.touch_start(22, 200.0, 360.0);
        rt.touch_move(22, 230.0, 380.0);
        rt.touch_end(22, 230.0, 380.0);
        assert!(rt.cg_pan()[0].abs() > 1.0);

        // Double-tap resets zoom cleanly back to 1.0x
        rt.touch_start(23, 200.0, 360.0);
        rt.touch_end(23, 200.0, 360.0);
        rt.tick(80_000);
        rt.touch_start(24, 202.0, 361.0);
        rt.touch_end(24, 202.0, 361.0);
        assert!((rt.cg_zoom() - 1.0).abs() < 0.01);

        // Swipe left switches CG variant 0 -> 1
        rt.touch_start(25, 280.0, 360.0);
        rt.touch_move(25, 160.0, 362.0);
        rt.touch_end(25, 140.0, 362.0);
        assert_eq!(rt.viewing_cg, Some((0, 1)));
    }

    #[test]
    fn wait_pulse_auto_sleeps_eco_mode_and_exact_wakeup_delay_work() {
        let mut rt = Runtime::new(
            test_prog(),
            Preferences::default(),
            ProfileData::default(),
            vec![],
        )
        .unwrap();
        rt.dispatch(UiAction::NewGame);
        assert!(rt.prepare_frame(1280.0, 720.0).is_some());
        let ep1 = rt.dialogue_epoch();
        // Finish revealing line 1 -> enters click-waiting state
        rt.dispatch(UiAction::Advance);
        assert!(rt.prepare_frame(1280.0, 720.0).is_some());
        assert!(
            rt.next_wakeup_delay_us().is_some(),
            "brief wait pulse should animate immediately after line finishes"
        );
        // Flush any debounced profile timer, render 1 mid-pulse frame, then let the 2.4s pulse finish
        rt.flush_pending_saves();
        rt.take_commands();
        rt.tick(120_000);
        assert!(rt.prepare_frame(1280.0, 720.0).is_some()); // mid-breath frame
        for _ in 0..12 {
            rt.tick(250_000);
        }
        assert!(rt.prepare_frame(1280.0, 720.0).is_some()); // final resting pose frame
        assert_eq!(
            rt.next_wakeup_delay_us(),
            None,
            "after brief pulse, resting on finished dialogue must sleep at 0Hz!"
        );
        assert!(!rt.needs_clock());

        // Auto mode sleeps precisely for auto_delay_ms instead of 60Hz spinning
        rt.dispatch(UiAction::ToggleAuto);
        let auto_delay = rt.next_wakeup_delay_us().expect("auto timer scheduled");
        assert!(
            auto_delay >= 1_000_000,
            "auto wait wakeup should sleep ~1.5s, got {auto_delay}us"
        );
        rt.tick(auto_delay);
        assert!(rt.prepare_frame(1280.0, 720.0).is_some());
        assert!(rt.dialogue_epoch() > ep1, "dialogue_epoch must bump on line change");

        // Eco mode disables wait pulse immediately and toggles cleanly
        rt.dispatch(UiAction::ToggleAuto);
        rt.dispatch(UiAction::ToggleEcoMode);
        assert!(rt.is_eco_mode());
    }
}
