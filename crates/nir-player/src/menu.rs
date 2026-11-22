use super::*;
use std::sync::Arc;
pub(super) struct SaveConfirmation {
    pub token: u32,
    pub control: String,
    pub slot: u32,
    pub revision: u32,
    pub session: u32,
    pub instance: u32,
}
type StorageState = (Vec<SlotView>, BTreeMap<u32, u32>, BTreeSet<u32>, bool);

#[derive(Clone)]
struct MenuFrame {
    menu: String,
    locals: BTreeMap<String, MenuValue>,
}
enum MenuMove {
    Replace,
    Push(MenuFrame),
    Pop(MenuFrame),
}
/// The displayed page remains committed until all target media is ready.
/// Frames are semantic locals only; hidden parents never retain media.
pub(super) struct MenuNavigation {
    target: String,
    source: String,
    screen: Screen,
    session: u32,
    instance: u32,
    movement: MenuMove,
}
#[derive(Clone)]
struct SuspendedTitle {
    page: MenuFrame,
    session: u32,
    parents: Vec<MenuFrame>,
}
/// Navigation-semantic menu state captured when a replay freezes the
/// session: the page, its locals, the parent chain and any suspended title
/// page. Storage, profile and reading mirrors are recomputed by sync and
/// never frozen.
#[derive(Clone)]
pub(super) struct FrozenMenu {
    menu: String,
    screen: Screen,
    locals: BTreeMap<String, MenuValue>,
    parents: Vec<MenuFrame>,
    suspended_title: Option<SuspendedTitle>,
    restore: Option<MenuFrame>,
    instance: u32,
    revision: u32,
}

pub(super) struct MenuSession {
    pub menu: String,
    screen: Screen,
    session: u32,
    profile: BTreeSet<String>,
    preferences: Preferences,
    storage: StorageState,
    reading: BTreeSet<MenuReadingMode>,
    story: BTreeMap<String, MenuValue>,
    history_available: bool,
    suspended_title: Option<SuspendedTitle>,
    parents: Vec<MenuFrame>,
    restore: Option<MenuFrame>,
    pub instance: u32,
    pub revision: u32,
    pub locals: BTreeMap<String, MenuValue>,
    flow: Arc<[nir_presentation::MenuHistoryRow]>,
    history_capacity: BTreeMap<String, (u32, usize)>,
}
impl MenuSession {
    pub fn new(menu: Option<&ImageMenu>, preferences: &Preferences, save_slots: u32) -> Self {
        Self {
            menu: "title".into(),
            screen: Screen::Title,
            session: 1,
            profile: BTreeSet::new(),
            preferences: preferences.clone(),
            history_available: false,
            storage: (
                (0..save_slots)
                    .map(|slot| SlotView {
                        slot,
                        ..Default::default()
                    })
                    .collect(),
                BTreeMap::new(),
                BTreeSet::new(),
                false,
            ),
            suspended_title: None,
            parents: vec![],
            restore: None,
            reading: BTreeSet::new(),
            flow: Arc::from([]),
            history_capacity: BTreeMap::new(),
            story: BTreeMap::new(),
            instance: 1,
            revision: 0,
            locals: menu.map(ImageMenu::initial_locals).unwrap_or_default(),
        }
    }
    fn next(n: u32) -> Result<u32> {
        n.checked_add(1)
            .ok_or_else(|| Diagnostic::new("E_VIEW_LIMIT", "menu", "view identity exhausted"))
    }
    pub(super) fn freeze(&self) -> FrozenMenu {
        FrozenMenu {
            menu: self.menu.clone(),
            screen: self.screen,
            locals: self.locals.clone(),
            parents: self.parents.clone(),
            suspended_title: self.suspended_title.clone(),
            restore: self.restore.clone(),
            instance: self.instance,
            revision: self.revision,
        }
    }
    /// Restore the frozen page under a new session with fresh authority:
    /// pre-freeze menu inputs stay rejected, and the bumped instance makes
    /// the page re-enter, replaying its page effects and music.
    pub(super) fn unfreeze(&mut self, frozen: FrozenMenu, session: u32) -> Result<()> {
        self.menu = frozen.menu;
        self.screen = frozen.screen;
        self.locals = frozen.locals;
        self.parents = frozen.parents;
        self.suspended_title = frozen.suspended_title;
        self.restore = frozen.restore;
        self.session = session;
        self.instance = Self::next(frozen.instance)?;
        self.revision = Self::next(frozen.revision)?;
        Ok(())
    }
    pub fn depth(&self) -> usize {
        self.parents.len()
    }
}
impl Player {
    fn push_menu(&mut self, target: &str) -> Result<()> {
        if self.menu_session.depth() >= nir_format::MAX_MENU_PARENTS {
            return Ok(());
        }
        let Some(current) = self.active_menu_id().map(str::to_owned) else {
            return Ok(());
        };
        self.stage_menu_page(
            target,
            MenuMove::Push(MenuFrame {
                menu: current,
                locals: self.menu_session.locals.clone(),
            }),
        )
    }
    pub(super) fn pop_menu(&mut self) -> Result<bool> {
        let Some(frame) = self.menu_session.parents.last().cloned() else {
            return Ok(false);
        };
        if !matches!(self.screen, Screen::Title | Screen::Menu) {
            return Ok(false);
        }
        self.stage_menu_page(&frame.menu.clone(), MenuMove::Pop(frame))?;
        Ok(true)
    }
    pub(super) fn select_menu_page(&mut self, target: &str) -> Result<()> {
        self.stage_menu_page(target, MenuMove::Replace)
    }
    fn stage_menu_page(&mut self, target: &str, movement: MenuMove) -> Result<()> {
        if self.is_loading()
            || self.pending_menu.is_some()
            || !matches!(self.screen, Screen::Title | Screen::Menu)
            || !self.core.program().theme.image_menus.contains_key(target)
        {
            return Ok(());
        }
        let Some(source) = self.active_menu_id().map(str::to_owned) else {
            return Ok(());
        };
        MenuSession::next(self.menu_session.instance)?;
        self.pending_menu = Some(MenuNavigation {
            target: target.into(),
            source,
            screen: self.screen,
            session: self.generation.session,
            instance: self.menu_session.instance,
            movement,
        });
        self.hovered_image = None;
        self.prepare_active_menu()
    }
    fn menu_navigation_current(&self) -> bool {
        self.pending_menu.as_ref().is_some_and(|n| {
            n.screen == self.screen
                && n.session == self.generation.session
                && n.instance == self.menu_session.instance
                && self.active_menu_id() == Some(n.source.as_str())
        })
    }
    pub(super) fn pending_menu_assets(&self) -> BTreeSet<String> {
        self.pending_menu
            .as_ref()
            .and_then(|n| self.core.program().theme.image_menus.get(&n.target))
            .map(ImageMenu::prepared_assets)
            .unwrap_or_default()
    }
    /// During ordinary navigation the committed page already owns its media.
    /// Boot and device rebuilds still wait for a valid current-generation page.
    pub fn can_project_menu_navigation(&self) -> bool {
        self.pending_menu.is_some()
            && self
                .active_menu_id()
                .is_some_and(|id| self.menu_page_ready(id))
    }
    pub(super) fn menu_navigation_preview(&self) -> Option<UiModel> {
        if !self.menu_navigation_current() {
            return None;
        }
        let next = self.pending_menu.as_ref()?;
        let menu = &self.core.program().theme.image_menus[&next.target];
        let mut model = self.model();
        model.image_menu = next.target.clone();
        model.authored_menu = true;
        model.menu_navigation_pending = false;
        model.menu_instance = self.menu_session.instance.saturating_add(1);
        model.menu_revision = 0;
        model.menu_locals = match &next.movement {
            MenuMove::Pop(frame) => frame.locals.clone(),
            _ => menu.initial_locals(),
        };
        model.menu_depth = match next.movement {
            MenuMove::Push(_) => self.menu_session.depth() + 1,
            MenuMove::Pop(_) => self.menu_session.depth().saturating_sub(1),
            MenuMove::Replace => self.menu_session.depth(),
        };
        model.menu_story = menu.story_values(&self.core.state().variables);
        model.hovered_image = None;
        model.menu_transition = None;
        model.menu_element_animations.clear();
        model.menu_opacity = 1.;
        Some(model)
    }
    pub(super) fn commit_menu_navigation(&mut self) -> Result<()> {
        if !self.menu_navigation_current() {
            self.pending_menu = None;
            return Ok(());
        }
        let next = self.pending_menu.take().unwrap();
        match next.movement {
            MenuMove::Replace => {}
            MenuMove::Push(frame) => self.menu_session.parents.push(frame),
            MenuMove::Pop(frame) => {
                self.menu_session.parents.pop();
                self.menu_session.restore = Some(frame);
            }
        }
        if self.screen == Screen::Title {
            self.image_menu = next.target;
        } else {
            self.overlay_menu = Some(next.target);
        }
        self.menu_session.menu.clear();
        self.hovered_image = None;
        if self.core.program().theme.image_menus[self.active_menu_id().unwrap()].uses_storage() {
            self.commands.push(AppCommand::ListSaves);
        }
        self.sync_menu_state()
    }
    pub(super) fn abandon_menu_navigation(&mut self) -> Result<()> {
        let rebuilding = self.pending_menu.is_some()
            && (self
                .prepare
                .as_ref()
                .is_some_and(|p| p.purpose == Purpose::Device)
                || self
                    .failed_admission
                    .as_ref()
                    .is_some_and(|(p, _, _)| *p == Purpose::Device)
                || self
                    .deferred_prepare
                    .as_ref()
                    .is_some_and(|(_, p, _, _)| *p == Purpose::Device)
                || self.content.values().any(|p| {
                    matches!(
                        p.purpose,
                        ContentPurpose::Media {
                            purpose: Purpose::Device,
                            ..
                        }
                    )
                }));
        let prepared = self.prepared_menu.clone();
        self.cancel_menu_preparation();
        self.prepared_menu = prepared;
        if rebuilding {
            self.begin_prepare(Purpose::Device, 0, self.retained_assets())?;
        }
        Ok(())
    }
    pub(super) fn menu_story_values(&self) -> BTreeMap<String, MenuValue> {
        self.active_menu_id()
            .and_then(|id| self.core.program().theme.image_menus.get(id))
            .map(|menu| menu.story_values(&self.core.state().variables))
            .unwrap_or_default()
    }

    pub(super) fn menu_reading_modes(&self) -> BTreeSet<MenuReadingMode> {
        let mut modes = BTreeSet::new();
        if self.menu_peek
            || self.screen != Screen::Menu
            || self.return_screen != Screen::Story
            || !self.pauses.is_only_named("menu")
            || self.is_loading()
            || self.slot_load.is_some()
            || self.save_confirmation.is_some()
            || self.error.is_some()
            || self.core.state().fault.is_some()
            || self.core.state().outcome.is_some()
            || !self
                .active_menu_id()
                .and_then(|id| self.core.program().theme.image_menus.get(id))
                .is_some_and(ImageMenu::uses_reading)
        {
            return modes;
        }
        modes.insert(MenuReadingMode::PeekStory);
        if self.core.state().choice.is_some() {
            return modes;
        }
        let Some((_, dialogue)) = self.core.dialogue() else {
            return modes;
        };
        modes.insert(MenuReadingMode::Auto);
        if self.profile.contains(&format!(
            "read:{}:{}",
            dialogue.text_id, dialogue.meaning_revision
        )) {
            modes.insert(MenuReadingMode::SkipRead);
        }
        modes
    }
    pub(super) fn menu_history_flow_model(
        &self,
        screen: Screen,
    ) -> Option<Arc<[nir_presentation::MenuHistoryRow]>> {
        if !matches!(screen, Screen::Title | Screen::Menu) {
            return None;
        }
        let menu = self
            .active_menu_id()
            .and_then(|id| self.core.program().theme.image_menus.get(id))?;
        menu.elements
            .iter()
            .any(|e| {
                matches!(e.content, MenuContent::HistoryFlow { .. })
                    && menu
                        .element_state(
                            &e.id,
                            &self.menu_session.locals,
                            &self.profile,
                            &self.menu_reading_modes(),
                            &self.menu_story_values(),
                            !self.core.state().history.is_empty(),
                        )
                        .0
            })
            .then(|| self.menu_session.flow.clone())
    }
    pub(super) fn history_rows(
        core: &Core,
        offset: usize,
        limit: usize,
    ) -> Vec<nir_presentation::MenuHistoryRow> {
        let mut rows: Vec<_> = core
            .state()
            .history
            .iter()
            .enumerate()
            .rev()
            .skip(offset.min(core.state().history.len().saturating_sub(1)))
            .take(limit.min(16))
            .map(|(key, h)| nir_presentation::MenuHistoryRow {
                key,
                entry: nir_presentation::HistoryView {
                    key,
                    voice_count: h.voices.len(),
                    choice: h.choice.as_ref().map(|c| match c.resolution {
                        nir_core::HistoryChoiceResolution::Selected { .. } => {
                            nir_presentation::HistoryChoiceKind::Selected
                        }
                        nir_core::HistoryChoiceResolution::TimedOut { .. } => {
                            nir_presentation::HistoryChoiceKind::TimedOut
                        }
                        nir_core::HistoryChoiceResolution::Cancelled => {
                            nir_presentation::HistoryChoiceKind::Cancelled
                        }
                    }),
                    speaker: h.speaker.clone(),
                    text: h.text.clone(),
                    images: h.images.clone(),
                    locale: h.locale.clone(),
                    font_plan_digest: h.font_plan_digest.clone(),
                    font_assets: core.program().locale_config.text[&h.locale].fonts.clone(),
                },
            })
            .collect();
        rows.reverse();
        rows
    }
    pub(super) fn menu_history_model(
        &self,
        core: &Core,
        screen: Screen,
    ) -> BTreeMap<String, Vec<nir_presentation::MenuHistoryRow>> {
        if !matches!(screen, Screen::Title | Screen::Menu) {
            return BTreeMap::new();
        }
        let Some(menu) = self
            .active_menu_id()
            .and_then(|id| core.program().theme.image_menus.get(id))
        else {
            return BTreeMap::new();
        };
        menu.elements
            .iter()
            .filter_map(|e| {
                let MenuContent::HistoryWindow {
                    offset_local,
                    limit,
                    voice_controls,
                    ..
                } = &e.content
                else {
                    return None;
                };
                if !menu
                    .element_state(
                        &e.id,
                        &self.menu_session.locals,
                        &self.profile,
                        &self.menu_reading_modes(),
                        &self.menu_story_values(),
                        !self.core.state().history.is_empty(),
                    )
                    .0
                {
                    return None;
                }
                let Some(MenuValue::Int(offset)) = self.menu_session.locals.get(offset_local)
                else {
                    return None;
                };
                Some((
                    e.id.clone(),
                    Self::history_rows(
                        core,
                        ((*offset).max(0) as usize).min(
                            core.state()
                                .history
                                .len()
                                .saturating_sub(if *voice_controls { 1 } else { *limit as usize }),
                        ),
                        *limit as usize,
                    ),
                ))
            })
            .collect()
    }
    /// Engine supplies capacities from a fresh projection before scoped menu
    /// controls run. The page instance prevents reuse across navigation.
    pub fn set_menu_history_capacity(&mut self, packet: &nir_presentation::DrawPacket) {
        self.menu_session.history_capacity = packet.menu_history_capacity.clone();
    }
    pub(super) fn menu_asset_stamp(&self, id: &str) -> (String, u32, u32, u32, u32) {
        (
            id.into(),
            self.generation.session,
            self.generation.device,
            self.generation.typography,
            self.generation.language,
        )
    }
    pub(super) fn cancel_menu_preparation(&mut self) {
        let navigation_device = self.pending_menu.is_some();
        let owns = |purpose: Purpose| {
            purpose == Purpose::Menu || (navigation_device && purpose == Purpose::Device)
        };
        self.pending_menu = None;
        self.prepared_menu = None;
        let owns_error = self
            .diagnostic
            .as_ref()
            .and_then(|d| d.details.as_ref())
            .is_some_and(|d| {
                d.request.is_some_and(|request| {
                    self.prepare
                        .as_ref()
                        .is_some_and(|p| p.request == request && owns(p.purpose))
                        || self.content.get(&request).is_some_and(|p| {
                            matches!(
                                p.purpose,
                                ContentPurpose::Media { purpose, .. } if owns(purpose)
                            )
                        })
                }) || (d.request.is_none()
                    && self
                        .failed_admission
                        .as_ref()
                        .is_some_and(|(p, _, _)| owns(*p)))
            });
        if owns_error {
            self.error = None;
            self.diagnostic = None;
            self.status.clear();
        }
        if self.prepare.as_ref().is_some_and(|p| owns(p.purpose))
            || self
                .failed_admission
                .as_ref()
                .is_some_and(|(p, _, _)| owns(*p))
        {
            self.cancel_preparation();
            self.pauses.remove("prepare");
        }
        if self
            .deferred_prepare
            .as_ref()
            .is_some_and(|(_, p, _, _)| owns(*p))
        {
            self.deferred_prepare = None;
            self.pauses.remove("prepare");
        }
        let requests: Vec<_> = self
            .content
            .iter()
            .filter(|(_, p)| {
                matches!(
                    p.purpose,
                    ContentPurpose::Media { purpose, .. } if owns(purpose)
                )
            })
            .map(|(id, _)| *id)
            .collect();
        for request in requests {
            self.content.remove(&request);
            self.commands.push(AppCommand::CancelContent { request });
        }
        if !self
            .content
            .values()
            .any(|p| !matches!(p.purpose, ContentPurpose::Locale | ContentPurpose::Prefetch))
        {
            self.pauses.remove("content");
        }
    }
    pub(super) fn prepare_active_menu(&mut self) -> Result<()> {
        if !matches!(self.screen, Screen::Title | Screen::Menu) {
            self.cancel_menu_preparation();
            return Ok(());
        }
        if self.pauses.contains("device") {
            return Ok(());
        }
        if self.pending_menu.is_some() && !self.menu_navigation_current() {
            self.abandon_menu_navigation()?;
        }
        let Some(id) = self
            .pending_menu
            .as_ref()
            .map(|n| n.target.as_str())
            .or_else(|| self.active_menu_id())
        else {
            return Ok(());
        };
        let stamp = self.menu_asset_stamp(id);
        if (self.pending_menu.is_none() && self.prepared_menu.as_ref() == Some(&stamp))
            || self.is_loading()
            || self.failed_admission.is_some()
        {
            return Ok(());
        }
        let assets = self.core.program().theme.image_menus[id].prepared_assets();
        self.begin_prepare(Purpose::Menu, 0, assets)
    }
    pub fn active_menu_id(&self) -> Option<&str> {
        let id = match self.screen {
            Screen::Title => Some(self.image_menu.as_str()),
            Screen::Menu => {
                self.overlay_menu
                    .as_deref()
                    .or(self.core.program().theme.menu_overlay.as_deref())
            }
            _ => None,
        }?;
        self.core
            .program()
            .theme
            .image_menus
            .contains_key(id)
            .then_some(id)
    }
    pub(super) fn sync_menu_state(&mut self) -> Result<()> {
        let reading = self.menu_reading_modes();
        let story = self.menu_story_values();
        let history_available = !self.core.state().history.is_empty();
        let storage = (
            self.slots.clone(),
            self.slot_revisions.clone(),
            self.save_jobs.values().map(|(slot, _)| *slot).collect(),
            self.slot_load.is_some(),
        );
        let active = self
            .active_menu_id()
            .map(str::to_owned)
            .unwrap_or_else(|| self.menu_session.menu.clone());
        let state = &mut self.menu_session;
        let new_session = state.session != self.generation.session;
        let new_page = state.menu != active
            || new_session
            || (self.screen == Screen::Menu
                && state.screen != Screen::Menu
                && self.core.program().theme.menu_overlay.is_some());
        if new_page || state.screen != self.screen {
            let next = MenuSession::next(state.instance)?;
            if new_session {
                state.suspended_title = None;
                state.parents.clear();
                state.restore = None;
            } else if state.screen == Screen::Title && self.screen != Screen::Title {
                state.suspended_title = Some(SuspendedTitle {
                    page: MenuFrame {
                        menu: state.menu.clone(),
                        locals: state.locals.clone(),
                    },
                    session: state.session,
                    parents: std::mem::take(&mut state.parents),
                });
            }
            if !matches!(self.screen, Screen::Title | Screen::Menu) {
                state.parents.clear();
                state.restore = None;
            }
            state.flow = if matches!(self.screen, Screen::Title | Screen::Menu)
                && self
                    .core
                    .program()
                    .theme
                    .image_menus
                    .get(&active)
                    .is_some_and(ImageMenu::uses_history_flow)
            {
                self.core
                    .state()
                    .history
                    .iter()
                    .enumerate()
                    .map(|(key, h)| nir_presentation::MenuHistoryRow {
                        key,
                        entry: nir_presentation::HistoryView {
                            key,
                            voice_count: h.voices.len(),
                            choice: h.choice.as_ref().map(|c| match c.resolution {
                                nir_core::HistoryChoiceResolution::Selected { .. } => {
                                    nir_presentation::HistoryChoiceKind::Selected
                                }
                                nir_core::HistoryChoiceResolution::TimedOut { .. } => {
                                    nir_presentation::HistoryChoiceKind::TimedOut
                                }
                                nir_core::HistoryChoiceResolution::Cancelled => {
                                    nir_presentation::HistoryChoiceKind::Cancelled
                                }
                            }),
                            speaker: h.speaker.clone(),
                            text: h.text.clone(),
                            images: h.images.clone(),
                            locale: h.locale.clone(),
                            font_plan_digest: h.font_plan_digest.clone(),
                            font_assets: self.core.program().locale_config.text[&h.locale]
                                .fonts
                                .clone(),
                        },
                    })
                    .collect::<Vec<_>>()
                    .into()
            } else {
                Arc::from([])
            };
            state.instance = next;
            state.revision = 0;
            if new_page {
                state.locals = self
                    .core
                    .program()
                    .theme
                    .image_menus
                    .get(&active)
                    .map(ImageMenu::initial_locals)
                    .unwrap_or_default();
            }
            if self.screen == Screen::Title && !new_session {
                if let Some(title) = state.suspended_title.take() {
                    if title.page.menu == active && title.session == self.generation.session {
                        state.locals = title.page.locals;
                        state.parents = title.parents;
                    }
                }
            }
            if let Some(frame) = state.restore.take() {
                if frame.menu == active && !new_session {
                    state.locals = frame.locals;
                }
            }
            state.menu = active;
            state.screen = self.screen;
            state.session = self.generation.session;
            self.hovered_image = None;
        } else if state.profile != self.profile
            || state.preferences != self.preferences
            || state.storage != storage
            || state.reading != reading
            || state.story != story
            || state.history_available != history_available
        {
            state.revision = MenuSession::next(state.revision)?;
        }
        state.profile = self.profile.clone();
        state.preferences = self.preferences.clone();
        state.storage = storage;
        state.reading = reading;
        state.story = story;
        state.history_available = history_available;
        let instance = state.instance;
        if self.save_confirmation.as_ref().is_some_and(|c| {
            c.session != self.generation.session
                || c.instance != instance
                || self.slot_revisions.get(&c.slot).copied().unwrap_or(0) != c.revision
                || self.save_jobs.values().any(|(slot, _)| *slot == c.slot)
                || self.active_menu_id().and_then(|id| self.core.program().theme.image_menus.get(id)).is_none_or(|menu| {
                    !menu.controls().any(|(id, action, guard)| id == c.control
                        && guard.is_none_or(|key| self.profile.contains(key))
                        && matches!(action, ImageMenuAction::SaveSlot {slot} if slot.resolve_in(&self.menu_session.locals, self.save_slots()) == Some(c.slot))
                        && (!menu.elements.iter().any(|e| e.id == id)
                            || menu.element_state(id, &self.menu_session.locals, &self.profile, &self.menu_reading_modes(), &self.menu_story_values(), !self.core.state().history.is_empty()) == (true, true)))
                })
        }) {
            self.save_confirmation = None;
        }
        Ok(())
    }
    pub(super) fn resolve_menu_value(
        &mut self,
        instance: u32,
        revision: u32,
        control: &str,
        value: MenuValueInput,
    ) -> Result<Option<UiAction>> {
        if self.menu_peek
            || self.is_loading()
            || self.save_confirmation.is_some()
            || instance != self.menu_session.instance
            || revision != self.menu_session.revision
        {
            return Ok(None);
        }
        let Some(menu) = self
            .active_menu_id()
            .and_then(|id| self.core.program().theme.image_menus.get(id))
        else {
            return Ok(None);
        };
        if menu.element_state(
            control,
            &self.menu_session.locals,
            &self.profile,
            &self.menu_reading_modes(),
            &self.menu_story_values(),
            !self.core.state().history.is_empty(),
        ) != (true, true)
        {
            return Ok(None);
        }
        let Some(element) = menu.elements.iter().find(|e| e.id == control) else {
            return Ok(None);
        };
        let next = MenuSession::next(self.menu_session.revision)?;
        let action = match (&element.content, value) {
            (MenuContent::Toggle { binding, .. }, MenuValueInput::Bool(value)) => match binding {
                MenuToggleBinding::Local { name } => {
                    self.menu_session
                        .locals
                        .insert(name.clone(), MenuValue::Bool(value));
                    None
                }
                MenuToggleBinding::ReducedMotion => {
                    (self.preferences.reduced_motion != value).then_some(UiAction::ReducedMotion)
                }
            },
            (
                MenuContent::Range {
                    binding,
                    min,
                    max,
                    step,
                    ..
                },
                MenuValueInput::Number(value),
            ) if value.is_finite() && value >= *min && value <= *max => {
                let value = if value == *max {
                    *max
                } else {
                    (*min + ((value - *min) / step).round() * step).clamp(*min, *max)
                };
                match binding {
                    MenuRangeBinding::Local { name } => {
                        self.menu_session
                            .locals
                            .insert(name.clone(), MenuValue::Int(value.round() as i32));
                        None
                    }
                    MenuRangeBinding::Preference { field } => {
                        Some(field.adjust(value - field.value(&self.preferences)))
                    }
                }
            }
            _ => return Ok(None),
        };
        self.menu_session.revision = next;
        self.hovered_image = None;
        Ok(action)
    }
    pub(super) fn resolve_menu_control(
        &mut self,
        instance: u32,
        revision: u32,
        control: &str,
        interaction: u32,
        sequence: u32,
    ) -> Result<Option<UiAction>> {
        if self.menu_peek
            || self.active_menu_id().is_none()
            || self.save_confirmation.is_some()
            || self.is_loading()
            || instance != self.menu_session.instance
            || revision != self.menu_session.revision
        {
            return Ok(None);
        }
        let Some(menu) = self
            .active_menu_id()
            .and_then(|id| self.core.program().theme.image_menus.get(id))
        else {
            return Ok(None);
        };
        let Some((_, action, guard)) = menu.controls().find(|(id, _, _)| *id == control) else {
            return Ok(None);
        };
        if guard.is_some_and(|key| !self.profile.contains(key)) {
            return Ok(None);
        }
        // Replay invariants re-check at dispatch, not just in projection:
        // nested entries and storage traffic die here for any entering or
        // live replay, and an exit exists only while one is actually live.
        if self.replay_work.is_some()
            && matches!(
                action,
                ImageMenuAction::Replay { .. }
                    | ImageMenuAction::SaveSlot { .. }
                    | ImageMenuAction::LoadSlot { .. }
            )
            || (matches!(action, ImageMenuAction::ExitReplay) && !self.replay_live())
        {
            return Ok(None);
        }
        if menu.elements.iter().any(|e| e.id == control)
            && menu.element_state(
                control,
                &self.menu_session.locals,
                &self.profile,
                &self.menu_reading_modes(),
                &self.menu_story_values(),
                !self.core.state().history.is_empty(),
            ) != (true, true)
        {
            return Ok(None);
        }
        let action = action.clone();
        if (matches!(action, ImageMenuAction::PushMenu { .. })
            && self.menu_session.depth() >= nir_format::MAX_MENU_PARENTS)
            || (matches!(action, ImageMenuAction::Back) && self.menu_session.depth() == 0)
        {
            return Ok(None);
        }
        if let ImageMenuAction::Reading { mode } = &action {
            if !self.menu_reading_modes().contains(mode) {
                return Ok(None);
            }
        }
        if let ImageMenuAction::SaveSlot { slot } = &action {
            if slot
                .resolve_in(&self.menu_session.locals, self.save_slots())
                .is_some_and(|slot| {
                    self.slots
                        .iter()
                        .any(|row| row.slot == slot && row.error.is_some())
                })
            {
                // Reject before acceptance feedback or menu revision changes.
                // The other controls in this displayed packet remain valid.
                return Ok(None);
            }
        }
        let next = MenuSession::next(self.menu_session.revision)?;
        // The click effect is acceptance feedback: it plays only on this
        // verified commit, never on rejected input or restore projections.
        let click = self.active_menu_effects().and_then(|e| e.click);
        if let ImageMenuAction::SetLocal { local, value } = &action {
            // Validate again at the mutation boundary, independently of projection.
            if menu
                .locals
                .get(local)
                .is_none_or(|definition| !definition.accepts(value))
            {
                return Ok(None);
            }
            self.menu_session
                .locals
                .insert(local.clone(), value.clone());
        }
        self.menu_session.revision = next;
        self.hovered_image = None;
        if let Some(asset) = click {
            self.play_ui_sound(&asset);
        }
        match &action {
            ImageMenuAction::PushMenu { menu } => {
                self.push_menu(menu)?;
                Ok(None)
            }
            ImageMenuAction::Back => {
                self.pop_menu()?;
                Ok(None)
            }
            ImageMenuAction::Reading { mode } => {
                if *mode == MenuReadingMode::PeekStory {
                    // Retain the overlay, its locals and pause owner. Only its
                    // projection is hidden; restoring the interface returns to it.
                    self.menu_peek = true;
                    self.interface_hidden = true;
                    return Ok(None);
                }
                // Only this verified menu request may combine closing the
                // overlay with a reading action. Existing raw toggles retain
                // their existing semantics and cannot bypass menu identity.
                if self.begin_menu_close(
                    DeferredExitKind::ReadingClose { mode: *mode },
                    interaction,
                    sequence,
                ) {
                    return Ok(None);
                }
                Ok(Some(self.commit_reading_close(*mode)?))
            }
            ImageMenuAction::HistoryPage { window, delta } => {
                let Some(menu) = self
                    .active_menu_id()
                    .and_then(|id| self.core.program().theme.image_menus.get(id))
                else {
                    return Ok(None);
                };
                if menu
                    .element_state(
                        window,
                        &self.menu_session.locals,
                        &self.profile,
                        &self.menu_reading_modes(),
                        &self.menu_story_values(),
                        !self.core.state().history.is_empty(),
                    )
                    .0
                {
                    let capacity = self
                        .menu_session
                        .history_capacity
                        .get(window)
                        .filter(|(instance, _)| *instance == self.menu_session.instance)
                        .map(|(_, limit)| *limit);
                    if let Some((name, value)) = menu.history_page_with_capacity(
                        window,
                        *delta,
                        &self.menu_session.locals,
                        self.core.state().history.len(),
                        capacity,
                    ) {
                        self.menu_session.locals.insert(name, MenuValue::Int(value));
                        self.stop_menu_history_voice(window);
                    }
                }
                Ok(None)
            }
            ImageMenuAction::SaveSlot { slot } | ImageMenuAction::LoadSlot { slot } => {
                let Some(slot) = slot.resolve_in(&self.menu_session.locals, self.save_slots())
                else {
                    return Ok(None);
                };
                if self.save_jobs.values().any(|(busy, _)| *busy == slot) {
                    return Ok(None);
                }
                if matches!(action, ImageMenuAction::LoadSlot { .. }) {
                    return Ok(self
                        .slots
                        .iter()
                        .any(|row| row.slot == slot && (row.exists || row.error.is_some()))
                        .then_some(UiAction::Load { slot }));
                }
                if self.screen == Screen::Title
                    || self.return_screen == Screen::Title
                    || self.slot_load.is_some()
                    || self
                        .slots
                        .iter()
                        .any(|row| row.slot == slot && row.error.is_some())
                {
                    return Ok(None);
                }
                let revision = self.slot_revisions.get(&slot).copied().unwrap_or(0);
                if revision == 0 && !self.slots.iter().any(|row| row.slot == slot && row.exists) {
                    return Ok(Some(UiAction::Save { slot }));
                }
                self.request = self.request.checked_add(1).ok_or_else(|| {
                    Diagnostic::new("E_REQUEST_LIMIT", "save", "confirmation identity exhausted")
                })?;
                self.save_confirmation = Some(SaveConfirmation {
                    token: self.request,
                    control: control.into(),
                    slot,
                    revision,
                    session: self.generation.session,
                    instance: self.menu_session.instance,
                });
                Ok(None)
            }
            _ => Ok(action.ui_action()),
        }
    }

    fn active_menu_effects(&self) -> Option<nir_format::MenuEffects> {
        self.active_menu_id()
            .and_then(|id| self.core.program().theme.image_menus.get(id))
            .and_then(|menu| menu.effects.clone())
    }

    /// A foreground-domain one-shot owned by the menu effects voice table.
    fn play_ui_sound(&mut self, asset: &str) {
        let session = self.generation.session;
        let Some(task) = self.menu_effects.alloc_task() else {
            return;
        };
        while self.menu_effects.sounds.len() >= nir_format::MAX_TASKS {
            if let Some((&oldest, _)) = self.menu_effects.sounds.first_key_value() {
                if let Some(voice) = self.menu_effects.sounds.remove(&oldest) {
                    self.commands.push(AppCommand::AudioStop {
                        session: voice.session,
                        domain: TimeDomain::ForegroundUi,
                        task: voice.task,
                    });
                }
            } else {
                break;
            }
        }
        self.menu_effects
            .sounds
            .insert(task, effects::UiVoice { task, session });
        self.commands.push(AppCommand::AudioStart {
            domain: TimeDomain::ForegroundUi,
            task,
            asset: asset.into(),
            bus: AudioBus::Sfx,
            looped: false,
            loop_region: None,
            position_us: Micros(0),
            gain: 1.,
            envelope: 1.,
            character: String::new(),
            session,
        });
    }

    fn start_menu_music(&mut self, music: &nir_format::MenuMusic) {
        let session = self.generation.session;
        let Some(task) = self.menu_effects.alloc_task() else {
            return;
        };
        self.menu_effects.music = Some(effects::UiVoice { task, session });
        self.commands.push(AppCommand::AudioStart {
            domain: TimeDomain::ForegroundUi,
            task,
            asset: music.asset.clone(),
            bus: music.bus,
            looped: true,
            loop_region: music.loop_region,
            position_us: Micros(0),
            gain: music.gain,
            envelope: 1.,
            character: String::new(),
            session,
        });
    }

    /// Looping page music is never awaited; it stops the moment its page
    /// stops being the active surface.
    fn stop_menu_music(&mut self) {
        if let Some(voice) = self.menu_effects.music.take() {
            self.commands.push(AppCommand::AudioStop {
                session: voice.session,
                domain: TimeDomain::ForegroundUi,
                task: voice.task,
            });
        }
    }

    fn menu_page_ready(&self, id: &str) -> bool {
        matches!(self.screen, Screen::Title | Screen::Menu)
            && self.prepared_menu.as_ref() == Some(&self.menu_asset_stamp(id))
    }

    fn fire_menu_enter(&mut self, id: &str, instance: u32) {
        let config = self
            .core
            .program()
            .theme
            .image_menus
            .get(id)
            .and_then(|menu| menu.effects.clone());
        self.menu_effects.entered = Some((id.to_owned(), instance));
        // Ownership moves to the new page first; its music replaces the old.
        self.stop_menu_music();
        let Some(effects) = config else { return };
        if let Some(enter) = &effects.enter {
            if let Some(sound) = &enter.sound {
                self.play_ui_sound(sound);
            }
            if enter.fade_us.0 > 0 && !self.preferences.reduced_motion {
                // Without a clock lease the fade could never advance; enter at
                // full opacity rather than stall invisible.
                if let Some(token) = self.acquire_foreground_clock() {
                    self.menu_effects.fade = Some(effects::PageFade {
                        closing: false,
                        start_us: self.ui_clock_us.0,
                        duration_us: enter.fade_us.0,
                        style: enter.style.clone(),
                    });
                    self.menu_effects_clock = Some(token);
                }
            }
        }
        if !effects.elements.is_empty() && !self.preferences.reduced_motion {
            // Same lease discipline as the fade: reuse the boundary's token when
            // the enter fade already holds one, otherwise lease a fresh clock.
            // Without a lease the tracks could never advance; the page rests at
            // its authored values instead of stalling mid-animation.
            if let Some(token) = self
                .menu_effects_clock
                .take()
                .or_else(|| self.acquire_foreground_clock())
            {
                let page = self.core.program().theme.image_menus.get(id);
                self.menu_effects
                    .start_elements(page, &effects.elements, self.ui_clock_us.0);
                if self.menu_effects.fade.is_some() || !self.menu_effects.elements.is_empty() {
                    self.menu_effects_clock = Some(token);
                }
            }
        }
        if let Some(music) = &effects.music {
            self.start_menu_music(music);
        }
    }

    /// Starts the finite close transaction. Returns true when the exit was
    /// deferred behind the close fade; the caller must then consume the input.
    pub(super) fn begin_menu_close(
        &mut self,
        kind: DeferredExitKind,
        interaction: u32,
        sequence: u32,
    ) -> bool {
        if self.menu_effects.closing.is_some() {
            return true;
        }
        let close = self.active_menu_effects().and_then(|effects| effects.close);
        let Some(close) = close else { return false };
        if let Some(sound) = &close.sound {
            self.play_ui_sound(sound);
        }
        if close.fade_us.0 > 0 && !self.preferences.reduced_motion {
            let Some(token) = self.acquire_foreground_clock() else {
                return false;
            };
            self.menu_effects.fade = Some(effects::PageFade {
                closing: true,
                start_us: self.ui_clock_us.0,
                duration_us: close.fade_us.0,
                style: close.style.clone(),
            });
            self.menu_effects.closing = Some(effects::DeferredExit {
                kind,
                interaction,
                sequence,
            });
            self.menu_effects_clock = Some(token);
            return true;
        }
        false
    }

    pub(super) fn story_modal(&self) -> Option<(u32, u32, nir_format::StoryModalTarget)> {
        self.core.state().tasks.values().find_map(|task| {
            if task.state != nir_core::TaskState::Running {
                return None;
            }
            if let Effect::StoryModal { target } = &task.effect {
                Some((task.id, task.modal_interaction?, target.clone()))
            } else {
                None
            }
        })
    }

    pub(super) fn open_story_modal(&mut self) -> Result<()> {
        if self.screen != Screen::Story || self.core.state().fault.is_some() {
            return Ok(());
        }
        let Some((_, _, target)) = self.story_modal() else {
            return Ok(());
        };
        self.return_screen = Screen::Story;
        self.held_skip = false;
        self.set_interface_hidden(false);
        self.menu_session.menu.clear();
        self.overlay_menu = None;
        self.screen = match target {
            nir_format::StoryModalTarget::ImageMenu { menu } => {
                self.overlay_menu = Some(menu);
                Screen::Menu
            }
            nir_format::StoryModalTarget::LoadSaves | nir_format::StoryModalTarget::SaveSaves => {
                Screen::Saves
            }
        };
        self.pauses.insert_barrier("menu".into());
        if self.screen == Screen::Saves {
            self.commands.push(AppCommand::ListSaves);
        }
        Ok(())
    }

    pub(super) fn story_modal_load(&self) -> bool {
        self.story_modal()
            .is_some_and(|(_, _, target)| matches!(target, nir_format::StoryModalTarget::LoadSaves))
    }

    pub(super) fn finish_story_modal(&mut self, budget: &mut u32) -> Result<()> {
        if let Some((task, interaction, _)) = self.story_modal() {
            self.step(CoreInput::ModalClosed { task, interaction }, budget)?;
        }
        Ok(())
    }

    pub(super) fn commit_close(
        &mut self,
        cancelled_slot_restore: bool,
        budget: &mut u32,
    ) -> Result<()> {
        // Complete the owner while the old page is still visible. Its
        // continuation may request content, but cannot reopen the old modal.
        if self.return_screen == Screen::Story {
            self.finish_story_modal(budget)?;
        }
        self.cancel_menu_preparation();
        self.screen = self.return_screen;
        self.pauses.remove("menu");
        self.status.clear();
        if cancelled_slot_restore && self.screen == Screen::Story && self.prepare.is_none() {
            if let Some(pending) = &self.core.state().pending {
                let cue = pending.cue.clone();
                let id = pending.id;
                let assets = self.validated.cue_assets(&cue);
                self.begin_prepare(Purpose::Activation, id, assets)?;
            }
        }
        Ok(())
    }

    fn commit_reading_close(&mut self, mode: MenuReadingMode) -> Result<UiAction> {
        self.cancel_menu_preparation();
        self.screen = Screen::Story;
        self.pauses.remove("menu");
        self.status.clear();
        Ok(match mode {
            MenuReadingMode::Auto => {
                self.auto = false;
                UiAction::ToggleAuto
            }
            MenuReadingMode::SkipRead => {
                self.skip = false;
                UiAction::ToggleSkip
            }
            MenuReadingMode::PeekStory => unreachable!("peeled off before deferral"),
        })
    }

    fn run_deferred_exit(&mut self, exit: effects::DeferredExit, budget: &mut u32) -> Result<()> {
        let effects::DeferredExit {
            kind,
            interaction,
            sequence,
        } = exit;
        match kind {
            DeferredExitKind::CloseScreen {
                cancelled_slot_restore,
            } => self.commit_close(cancelled_slot_restore, budget)?,
            DeferredExitKind::ReadingClose { mode } => {
                let toggle = self.commit_reading_close(mode)?;
                self.finish_story_modal(budget)?;
                self.action(toggle, interaction, sequence, budget)?;
            }
        }
        Ok(())
    }

    /// Advances menu page effects: fades follow the foreground clock, a
    /// finished close fade runs its deferred exit, prepared pages fire their
    /// enter effects once, and leaving the menu surfaces stops page music.
    pub(super) fn update_menu_effects(&mut self, budget: &mut u32) -> Result<()> {
        if self.menu_effects.session != self.generation.session {
            // The host resets both audio domains with the session; every
            // effect voice died with it and a deferred exit now references
            // state that no longer exists.
            self.menu_effects = MenuEffectsState::new(self.generation.session);
            self.menu_effects_clock = None;
        }
        let had_fade = self.menu_effects.fade.is_some();
        let exit = self.menu_effects.take_finished_close(self.ui_clock_us.0);
        if (exit.is_some() || self.menu_effects.fade.is_none())
            && self.menu_effects.elements.is_empty()
        {
            self.menu_effects_clock = None;
        }
        self.menu_effects.settle_enter(self.ui_clock_us.0);
        let drained = self
            .menu_effects
            .drain_finished_elements(self.ui_clock_us.0);
        if self.menu_effects.fade.is_none() && self.menu_effects.elements.is_empty() {
            self.menu_effects_clock = None;
        }
        if (had_fade && self.menu_effects.fade.is_none()) || drained > 0 {
            // The divert (or alpha ramp, or the last element track) lifted this
            // tick — a discrete visual change that a clock-only tick would
            // otherwise leave unprojected right as the clock token releases
            // and the frame loop stops.
            self.ui_visual_pulse = true;
        }
        if let Some(exit) = exit {
            self.run_deferred_exit(exit, budget)?;
        }
        let page = if matches!(self.screen, Screen::Title | Screen::Menu) {
            self.active_menu_id().map(str::to_owned)
        } else {
            None
        };
        let identity = page.map(|id| (id, self.menu_session.instance));
        if self.menu_effects.entered.is_some() && self.menu_effects.entered != identity {
            // The page changed, or the surfaces were left through a path that
            // owed no close fade; page effects never outlive their page.
            self.stop_menu_music();
            self.menu_effects.retire_page();
            // Retiring drops in-flight element tracks that would otherwise
            // hold the clock lease for animations no page will ever show.
            self.menu_effects_clock = None;
        }
        if let Some((id, instance)) = identity {
            if self.menu_effects.closing.is_none()
                && self.menu_effects.entered.is_none()
                && self.menu_page_ready(&id)
            {
                self.fire_menu_enter(&id, instance);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
