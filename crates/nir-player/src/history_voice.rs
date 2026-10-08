//! One foreground history audition. It owns no VM task, checkpoint or story
//! clock. Only the current clip's catalog and decoded buffer are pinned.
use super::*;

pub(super) enum HistoryVoiceStage {
    Catalog {
        request: u32,
    },
    Media {
        request: u32,
        job: PrepareJob,
    },
    Playing {
        task: u32,
        _lease: nir_assets::ReadyLease,
    },
    Failed,
}
pub(super) struct HistoryPlayback {
    pub entry: usize,
    pub current: HistoryVoice,
    character: String,
    remaining: VecDeque<HistoryVoice>,
    session: u32,
    device: u32,
    owner: u32,
    window: Option<String>,
    pub stage: Option<HistoryVoiceStage>,
}
impl Player {
    fn history_voice_alive(&self) -> bool {
        self.history_voice.as_ref().is_some_and(|voice| {
            (match &voice.window {
                None => self.screen == Screen::History,
                Some(window) => self.menu_history_voice_entry(window, voice.entry),
            }) && voice.session == self.generation.session
                && voice.device == self.generation.device
                && voice.owner == self.menu_session.instance
                && !self.slot_restore
                && self.slot_load.is_none()
                && self.candidate.is_none()
                && self.restore_work.is_none()
        })
    }
    pub(super) fn update_history_voice(&mut self) {
        if self.history_voice.is_some() && !self.history_voice_alive() {
            self.stop_history_voice();
        }
    }
    pub(super) fn accepts_history_voice_resource(&self, request: u32) -> bool {
        self.history_voice_alive() && self.history_voice.as_ref().is_some_and(|voice| {
            matches!(voice.stage, Some(HistoryVoiceStage::Media { request: id, .. }) if id == request)
        })
    }
    fn retire_history_voice_stage(&mut self, stage: Option<HistoryVoiceStage>, session: u32) {
        match stage {
            Some(HistoryVoiceStage::Catalog { request }) => {
                self.content.remove(&request);
                self.commands.push(AppCommand::CancelContent { request });
            }
            Some(HistoryVoiceStage::Media { request, job }) => {
                // A cancelled decode may still physically own its allocation.
                // Keep its budget until the host acknowledges cancellation.
                self.media_retired.insert(request, job);
                self.commands.push(AppCommand::CancelAssets { request });
            }
            Some(HistoryVoiceStage::Playing { task, .. }) => {
                self.commands.push(AppCommand::AudioStop {
                    domain: TimeDomain::ForegroundUi,
                    task,
                    session,
                });
            }
            _ => {}
        }
    }
    pub(super) fn stop_history_voice(&mut self) {
        if let Some(mut voice) = self.history_voice.take() {
            self.retire_history_voice_stage(voice.stage.take(), voice.session);
            self.ui_visual_pulse = true;
        }
    }
    pub(super) fn fail_history_voice(&mut self) {
        if let Some(mut voice) = self.history_voice.take() {
            self.retire_history_voice_stage(voice.stage.take(), voice.session);
            voice.stage = Some(HistoryVoiceStage::Failed);
            self.history_voice = Some(voice);
            self.ui_visual_pulse = true;
            self.observe("history_voice_failed", None);
        }
    }
    pub(super) fn start_history_voice(&mut self, entry: usize) {
        if self.screen != Screen::History
            || !Self::history_rows(&self.core, self.history_offset, 3)
                .iter()
                .any(|row| row.key == entry)
        {
            return;
        }
        self.start_history_voice_in(entry, None);
    }
    fn start_history_voice_in(&mut self, entry: usize, window: Option<String>) {
        if self.is_loading()
            || self.locale_pending()
            || self.slot_restore
            || self.slot_load.is_some()
            || self.candidate.is_some()
            || self.restore_work.is_some()
        {
            return;
        }
        let Some(record) = self
            .core
            .state()
            .history
            .get(entry)
            .filter(|record| !record.voices.is_empty())
        else {
            return;
        };
        let mut voices: VecDeque<_> = record.voices.clone().into();
        let current = voices.pop_front().unwrap();
        let character = record.speaker_id.clone();
        self.stop_history_voice();
        self.history_voice = Some(HistoryPlayback {
            entry,
            current,
            character,
            remaining: voices,
            session: self.generation.session,
            device: self.generation.device,
            owner: self.menu_session.instance,
            window,
            stage: None,
        });
        if self.prepare_history_voice().is_err() {
            self.fail_history_voice();
        }
    }
    fn menu_history_voice_entry(&self, window: &str, entry: usize) -> bool {
        if !matches!(self.screen, Screen::Title | Screen::Menu)
            || self.menu_peek
            || self.save_confirmation.is_some()
            || self.menu_effects.closing.is_some()
            || self.error.is_some()
        {
            return false;
        }
        let Some(menu) = self
            .active_menu_id()
            .and_then(|id| self.core.program().theme.image_menus.get(id))
        else {
            return false;
        };
        if menu.element_state(
            window,
            &self.menu_session.locals,
            &self.profile,
            &self.menu_reading_modes(),
            &self.menu_story_values(),
            !self.core.state().history.is_empty(),
        ) != (true, true)
        {
            return false;
        }
        let Some(element) = menu.elements.iter().find(|e| e.id == window) else {
            return false;
        };
        match &element.content {
            MenuContent::HistoryWindow {
                voice_controls: true,
                offset_local,
                limit,
                ..
            } => {
                let Some(MenuValue::Int(offset)) = self.menu_session.locals.get(offset_local)
                else {
                    return false;
                };
                let total = self.core.state().history.len();
                let offset = (*offset).max(0) as usize;
                let end = total.saturating_sub(offset.min(total.saturating_sub(1)));
                entry < end
                    && entry >= end.saturating_sub(*limit as usize)
                    && self
                        .core
                        .state()
                        .history
                        .get(entry)
                        .is_some_and(|h| !h.voices.is_empty())
            }
            MenuContent::HistoryFlow {
                voice_controls: true,
                ..
            } => self
                .menu_history_flow_model(self.screen)
                .is_some_and(|rows| {
                    rows.iter()
                        .any(|row| row.key == entry && row.entry.voice_count > 0)
                }),
            _ => false,
        }
    }
    /// Called only after Engine validates this action against a fresh, visible
    /// semantic control. Layout/clip authority belongs to the shared presenter.
    pub fn audition_menu_history(&mut self, action: &UiAction) {
        let UiAction::MenuHistoryVoice {
            instance,
            revision,
            window,
            entry,
            stop,
            ..
        } = action
        else {
            return;
        };
        if *instance != self.menu_session.instance
            || *revision != self.menu_session.revision
            || self.is_loading()
            || self.locale_pending()
            || !self.menu_history_voice_entry(window, *entry)
        {
            return;
        }
        if *stop {
            if self
                .history_voice
                .as_ref()
                .is_some_and(|voice| voice.entry == *entry && voice.window.as_ref() == Some(window))
            {
                self.stop_history_voice();
            }
        } else {
            self.start_history_voice_in(*entry, Some(window.clone()));
        }
    }
    pub fn stop_menu_history_voice(&mut self, window: &str) {
        if self
            .history_voice
            .as_ref()
            .is_some_and(|voice| voice.window.as_deref() == Some(window))
        {
            self.stop_history_voice();
        }
    }
    /// Recheck controls after scrolling, reflow, clipping or visibility changes.
    /// Incomplete/offscreen layouts must not retain an invisible audition.
    pub fn validate_history_voice_controls(
        &mut self,
        packet: &nir_presentation::DrawPacket,
    ) -> bool {
        if self.history_voice.as_ref().is_some_and(|voice| {
            voice.window.as_ref().is_some_and(|window| {
                !packet.semantics.iter().any(|node| {
                    node.enabled
                        && matches!(&node.action,
                UiAction::MenuHistoryVoice { instance,window:target,entry,.. }
                    if *instance==voice.owner && target==window && *entry==voice.entry)
                })
            })
        }) {
            self.stop_history_voice();
            return true;
        }
        false
    }
    pub(super) fn prepare_history_voice(&mut self) -> Result<()> {
        if !self.history_voice_alive() {
            self.stop_history_voice();
            return Ok(());
        }
        let asset = self.history_voice.as_ref().unwrap().current.asset.clone();
        let ids = BTreeSet::from([asset]);
        let objects = self.asset_content_requirements(&ids)?;
        if !objects.is_empty() {
            self.begin_content(ContentPurpose::HistoryVoice, objects)?;
            self.history_voice.as_mut().unwrap().stage = Some(HistoryVoiceStage::Catalog {
                request: self.request,
            });
            self.ui_visual_pulse = true;
            return Ok(());
        }
        let costs = self.costs(&ids)?;
        let job = PrepareJob::new(0, self.generation, costs, &self.ledger)?;
        self.request = self
            .request
            .checked_add(1)
            .ok_or_else(|| Diagnostic::new("E_LIMIT", "history", "request counter"))?;
        let request = self.request;
        let descriptors = self.describe_assets(&ids)?;
        self.history_voice.as_mut().unwrap().stage =
            Some(HistoryVoiceStage::Media { request, job });
        self.commands.push(AppCommand::GetAssets {
            request,
            session: self.generation.session,
            device: self.generation.device,
            assets: ids.into_iter().collect(),
            descriptors,
            priority: PreparePriority::Required,
        });
        self.ui_visual_pulse = true;
        Ok(())
    }
    pub(super) fn history_voice_asset_ready(&mut self, request: u32, asset: &str) -> Result<()> {
        if !self.accepts_history_voice_resource(request) {
            return Ok(());
        }
        let voice = self.history_voice.as_mut().unwrap();
        let Some(HistoryVoiceStage::Media { job, .. }) = voice.stage.as_mut() else {
            return Ok(());
        };
        // Audio-only preparation is independent of viewport and typography.
        // Session/device identity is checked above before accepting readiness.
        job.ready(asset, job.generation);
        if !job.missing.is_empty() {
            return Ok(());
        }
        let Some(HistoryVoiceStage::Media { job, .. }) = voice.stage.take() else {
            unreachable!();
        };
        let lease = job.finish()?;
        if self.menu_effects.next_task == u32::MAX {
            self.fail_history_voice();
            return Ok(());
        }
        let task = self.menu_effects.alloc_task();
        voice.stage = Some(HistoryVoiceStage::Playing {
            task,
            _lease: lease,
        });
        self.commands.push(AppCommand::AudioStart {
            domain: TimeDomain::ForegroundUi,
            task,
            session: voice.session,
            asset: voice.current.asset.clone(),
            bus: AudioBus::Voice,
            looped: false,
            loop_region: None,
            position_us: Micros(0),
            gain: voice.current.gain,
            envelope: 1.,
            character: voice.character.clone(),
        });
        self.ui_visual_pulse = true;
        Ok(())
    }
    pub(super) fn history_voice_audio_event(
        &mut self,
        domain: TimeDomain,
        task: u32,
        session: u32,
        failed: bool,
    ) -> Result<bool> {
        if domain != TimeDomain::ForegroundUi || !self.history_voice_alive()
            || !self.history_voice.as_ref().is_some_and(|voice| voice.session == session
                && matches!(voice.stage, Some(HistoryVoiceStage::Playing { task: id, .. }) if id == task))
        { return Ok(false); }
        if failed {
            self.fail_history_voice();
            return Ok(true);
        }
        let voice = self.history_voice.as_mut().unwrap();
        voice.stage = None; // Natural end releases this clip's reservation.
        if let Some(next) = voice.remaining.pop_front() {
            voice.current = next;
            if self.prepare_history_voice().is_err() {
                self.fail_history_voice();
            }
        } else {
            self.history_voice = None;
        }
        self.ui_visual_pulse = true;
        Ok(true)
    }
    pub fn history_voice_model(&self) -> Option<nir_presentation::HistoryVoiceView> {
        self.history_voice
            .as_ref()
            .map(|voice| nir_presentation::HistoryVoiceView {
                window: voice.window.clone(),
                entry: voice.entry,
                failed: matches!(voice.stage, Some(HistoryVoiceStage::Failed)),
                preparing: matches!(
                    voice.stage,
                    None | Some(
                        HistoryVoiceStage::Catalog { .. } | HistoryVoiceStage::Media { .. }
                    )
                ),
            })
    }
}
