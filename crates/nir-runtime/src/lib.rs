//! Unified single-owner Galgame runtime combining VM coordination, reading modes
//! (Auto/Skip/Rollback/Replay), predictive asset preloading, exact dirty-gated UI
//! projection, lightweight Toast notices, and live Hot-Reload.
//!
//! Replaces 26,400+ lines across legacy `nir-player` + `nir-engine` + `nir-assets`.
#![forbid(unsafe_code)]

use nir_ir::*;
use nir_ui::{DrawPacket, Messages, Screen, SlotSummary, UiContext};
use nir_vm::{ProfileData, RollbackCheckpoint, SaveSnapshot, Vm, VmIntent, WaitReason};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

const MAX_ROLLBACK_CHECKPOINTS: usize = 32;
const SKIP_INTERVAL_US: u64 = 55_000;
const PROFILE_FLUSH_INTERVAL_US: u64 = 5_000_000;

/// Typed commands sent from `Runtime` to the platform host (Native / Web / Mobile).
#[derive(Debug, Clone, PartialEq)]
pub enum HostCommand {
    /// Request the host to load/decode an asset (image/audio/font). `urgent` = needed now.
    LoadAsset { id: AssetId, urgent: bool },
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
    SaveProfile(ProfileData),
    SavePreferences(Preferences),
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
    voice_playing: bool,
    checkpoints: VecDeque<RollbackCheckpoint>,
    pub slots: Vec<SlotSummary>,
    history_scroll: usize,
    hovered_hit: Option<u32>,
    toast: Option<(String, u64)>,
    loaded_assets: BTreeSet<AssetId>,
    /// Tracks in-flight asset requests and whether they have already been promoted to `urgent`.
    requested_assets: BTreeMap<AssetId, bool>,
    commands: Vec<HostCommand>,
    last_packet: DrawPacket,
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
            voice_playing: false,
            checkpoints: VecDeque::new(),
            slots: norm_slots,
            history_scroll: 0,
            hovered_hit: None,
            toast: None,
            loaded_assets: BTreeSet::new(),
            requested_assets: BTreeMap::new(),
            commands: Vec::new(),
            last_packet: DrawPacket::default(),
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
        self.dirty = true;
    }

    pub fn mark_asset_loaded(&mut self, id: AssetId) {
        self.loaded_assets.insert(id);
        self.requested_assets.remove(&id);
        self.dirty = true;
    }

    pub fn mark_asset_failed(&mut self, id: AssetId, err: &str) {
        self.requested_assets.remove(&id);
        self.show_toast(format!("Asset #{} load failed: {err}", id.0), 3500);
    }

    pub fn notify_audio_ended(&mut self, channel: ChannelId) {
        if channel == ChannelId(1) {
            self.voice_playing = false;
        }
        self.vm.notify_audio_ended(channel);
    }

    pub fn notify_video_finished(&mut self) {
        self.vm.notify_video_finished();
        self.step_vm();
    }

    /// Live hot-reload of an updated compiled program without resetting reading state!
    pub fn apply_hot_reload(&mut self, new_program: Arc<NirProgram>) {
        self.program = new_program.clone();
        self.vm.hot_reload(new_program);
        self.loaded_assets.clear();
        self.requested_assets.clear();
        if self.screen == Screen::Story {
            self.step_vm();
        } else {
            self.request_title_assets();
        }
        self.show_toast("Hot-reloaded script & assets", 2000);
        self.dirty = true;
    }

    pub fn needs_clock(&self) -> bool {
        if self.paused_by_host {
            return false;
        }
        if self.toast.is_some() {
            return true;
        }
        if self.screen != Screen::Story {
            return false;
        }
        self.vm.needs_clock() || self.auto || self.skip || self.hold_skip
    }

    pub fn set_hover(&mut self, x: f32, y: f32) {
        let next = self.last_packet.hit_test(x, y).map(|h| h.id);
        if next != self.hovered_hit {
            self.hovered_hit = next;
            self.dirty = true;
        }
    }

    /// Handles primary click / tap at viewport coordinates `(x, y)`.
    pub fn pointer_click(&mut self, x: f32, y: f32) {
        if self.interface_hidden && self.screen == Screen::Story {
            self.interface_hidden = false;
            self.dirty = true;
            return;
        }
        if let Some(hit) = self.last_packet.hit_test(x, y) {
            let action = hit.action.clone();
            self.dispatch(action);
            return;
        }
        if self.screen == Screen::Story && self.vm.choice.is_none() {
            self.dispatch(UiAction::Advance);
        }
    }

    /// Handles secondary click / right-click / Android Back / Escape.
    pub fn cancel_or_menu(&mut self) {
        if self.interface_hidden && self.screen == Screen::Story {
            self.interface_hidden = false;
            self.dirty = true;
            return;
        }
        match self.screen {
            Screen::Story => {
                if self.vm.choice.as_ref().is_some_and(|c| c.cancel_pc.is_some()) {
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
                if let Ok(mut vm) = Vm::new(self.program.clone(), Some(self.prefs.text_locale.clone()))
                {
                    vm.set_profile(profile);
                    self.vm = vm;
                    self.screen = Screen::Story;
                    self.step_vm();
                }
            }
            UiAction::Continue => {
                if let Some(latest) = self
                    .slots
                    .iter()
                    .filter(|s| s.exists)
                    .max_by_key(|s| s.saved_at_unix)
                    .map(|s| s.slot)
                {
                    self.commands.push(HostCommand::RequestLoadSlot(latest));
                }
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
                let stop_voice = !self.prefs.continue_voice_on_advance;
                let intents = self.vm.advance(stop_voice);
                self.handle_intents(intents);
                self.step_vm();
            }
            UiAction::Choose(idx) => {
                if self.screen != Screen::Story {
                    return;
                }
                self.auto_elapsed_us = 0;
                let intents = self.vm.choose(idx as usize);
                self.handle_intents(intents);
                self.step_vm();
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
                }
                self.skip_elapsed_us = 0;
            }
            UiAction::HoldSkip(held) => {
                self.hold_skip = held;
                self.skip_elapsed_us = 0;
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
                }
            }
            UiAction::OpenSaves { save_mode } => {
                if self.frozen_vm.is_some() {
                    return;
                }
                if matches!(self.screen, Screen::Title | Screen::Story | Screen::Menu) {
                    self.return_screen = self.screen;
                }
                self.screen = Screen::Saves { save_mode };
            }
            UiAction::OpenSettings => {
                if matches!(self.screen, Screen::Title | Screen::Story | Screen::Menu) {
                    self.return_screen = self.screen;
                }
                self.screen = Screen::Settings;
            }
            UiAction::OpenHistory => {
                if matches!(self.screen, Screen::Story | Screen::Menu) {
                    self.return_screen = self.screen;
                }
                self.history_scroll = 0;
                self.screen = Screen::History;
            }
            UiAction::OpenGallery => {
                self.return_screen = self.screen;
                self.screen = Screen::Gallery;
            }
            UiAction::SaveSlot(slot) => {
                if self.frozen_vm.is_some() {
                    return;
                }
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
            UiAction::LoadSlot(slot) => {
                self.commands.push(HostCommand::RequestLoadSlot(slot));
            }
            UiAction::DeleteSlot(slot) => {
                if let Some(entry) = self.slots.iter_mut().find(|s| s.slot == slot) {
                    entry.exists = false;
                    entry.summary.clear();
                    entry.saved_at_unix = 0;
                }
                self.commands.push(HostCommand::DeleteSlot(slot));
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
                self.flush_pending_saves();
                self.stop_all_audio();
                self.frozen_vm = None;
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
                // Update live channels immediately!
                for ch in &self.vm.audio {
                    if ch.bus == bus {
                        let gain = ch.gain
                            * ch.envelope
                            * self.prefs.effective_bus_gain(bus, ch.character.as_deref());
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
                    self.history_scroll =
                        self.history_scroll.saturating_add(delta as usize).min(max_scroll);
                } else {
                    self.history_scroll =
                        self.history_scroll.saturating_sub(delta.unsigned_abs() as usize);
                }
            }
            UiAction::Quit => {
                self.flush_pending_saves();
                self.commands.push(HostCommand::Quit);
            }
        }
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
        let delta_us = delta_us.min(250_000); // Cap frame hitch catch-up to 250ms.

        // Debounced persistent profile flush (batches rapid read_texts updates into 1 write per 5s).
        if self.profile_dirty {
            self.profile_dirty_elapsed_us = self.profile_dirty_elapsed_us.saturating_add(delta_us);
            if self.profile_dirty_elapsed_us >= PROFILE_FLUSH_INTERVAL_US {
                self.flush_pending_saves();
            }
        }

        if let Some((_, rem)) = &mut self.toast {
            if delta_us >= *rem {
                self.toast = None;
                self.dirty = true;
            } else {
                *rem -= delta_us;
            }
        }

        if self.screen != Screen::Story {
            return;
        }

        let (intents, visual_changed) = self
            .vm
            .tick_with_dirty(delta_us, self.prefs.text_speed, self.prefs.reduced_motion);
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
                self.skip_elapsed_us = self.skip_elapsed_us.saturating_add(delta_us);
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
            let voice_ready = !self.prefs.auto_wait_voice || !self.voice_playing;
            if dlg_done && voice_ready {
                self.auto_elapsed_us = self.auto_elapsed_us.saturating_add(delta_us);
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
        let size_changed =
            (self.last_packet.width - width).abs() > 0.5 || (self.last_packet.height - height).abs() > 0.5;
        if !self.dirty && !size_changed {
            return None;
        }
        let ctx = UiContext {
            vm: &self.vm,
            screen: self.screen,
            prefs: &self.prefs,
            auto: self.auto,
            skip: self.skip || self.hold_skip,
            interface_hidden: self.interface_hidden,
            replay_active: self.frozen_vm.is_some(),
            slots: &self.slots,
            history_scroll: self.history_scroll,
            hovered_hit: self.hovered_hit,
            toast: self.toast.as_ref().map(|(m, _)| m.as_str()),
        };
        let next = nir_ui::project(&ctx, width, height, &self.messages);
        let changed = size_changed || !self.last_packet.visual_eq(&next);
        self.last_packet = next;
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
                    if channel == ChannelId(1) && bus == AudioBus::Voice {
                        self.voice_playing = true;
                    }
                    let eff = gain * self.prefs.effective_bus_gain(bus, character.as_deref());
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
                    if channel == ChannelId(1) {
                        self.voice_playing = false;
                    }
                    self.commands.push(HostCommand::StopAudio {
                        channel,
                        fade_out_us,
                    });
                }
                VmIntent::SetAudioGain { channel, gain } => {
                    let bus = self
                        .vm
                        .audio
                        .iter()
                        .find(|c| c.channel == channel)
                        .map_or(AudioBus::Bgm, |c| c.bus);
                    let eff = gain * self.prefs.effective_bus_gain(bus, None);
                    self.commands
                        .push(HostCommand::SetAudioGain { channel, gain: eff });
                }
                VmIntent::PauseAudioBus { bus, paused } => {
                    self.commands
                        .push(HostCommand::PauseAudioBus { bus, paused });
                }
                VmIntent::PlayVideo { asset, skippable } => {
                    self.request_asset(asset, true);
                    self.commands.push(HostCommand::PlayVideo { asset, skippable });
                }
                VmIntent::StopVideo => {
                    self.commands.push(HostCommand::StopVideo);
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
        self.voice_playing = false;
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
                self.commands.push(HostCommand::LoadAsset { id, urgent: true });
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
            let gain = self.prefs.effective_bus_gain(AudioBus::Bgm, None);
            self.commands.push(HostCommand::PlayAudio {
                channel: ChannelId(0),
                bus: AudioBus::Bgm,
                asset: bgm,
                gain,
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
        // Predictive background lookahead over next 24 bytecode ops!
        for id in self.vm.predict_assets(24) {
            pinned.insert(id);
            self.request_asset(id, false);
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
        })
    }

    #[test]
    fn runtime_flow_save_rollback_and_dirty_gating() {
        let mut rt = Runtime::new(test_prog(), Preferences::default(), ProfileData::default(), vec![])
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
            !mid_cmds.iter().any(|c| matches!(c, HostCommand::SaveProfile(_))),
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
        let mut rt = Runtime::new(test_prog(), Preferences::default(), ProfileData::default(), vec![])
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
}
