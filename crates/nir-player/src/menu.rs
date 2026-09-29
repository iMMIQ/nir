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
}
impl MenuSession {
    pub fn new(menu: Option<&ImageMenu>, preferences: &Preferences) -> Self {
        Self {
            menu: "title".into(),
            screen: Screen::Title,
            session: 1,
            profile: BTreeSet::new(),
            preferences: preferences.clone(),
            history_available: false,
            storage: (
                (0..3)
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
        if !matches!(self.screen, Screen::Title | Screen::Menu)
            || self.menu_session.depth() >= nir_format::MAX_MENU_PARENTS
            || !self.core.program().theme.image_menus.contains_key(target)
        {
            return Ok(());
        }
        // Frames hold only bounded semantic locals, never media leases or
        // history text. Returning creates new authority and prepares its page.
        let Some(current) = self.active_menu_id().map(str::to_owned) else {
            return Ok(());
        };
        let frame = MenuFrame {
            menu: current,
            locals: self.menu_session.locals.clone(),
        };
        self.menu_session.parents.push(frame);
        self.select_menu_page(target);
        self.sync_menu_state()
    }
    pub(super) fn pop_menu(&mut self) -> Result<bool> {
        if !matches!(self.screen, Screen::Title | Screen::Menu) {
            return Ok(false);
        }
        let Some(frame) = self.menu_session.parents.pop() else {
            return Ok(false);
        };
        self.select_menu_page(&frame.menu);
        self.menu_session.restore = Some(frame);
        self.sync_menu_state()?;
        Ok(true)
    }
    fn select_menu_page(&mut self, target: &str) {
        self.cancel_menu_preparation();
        if self.screen == Screen::Title {
            self.image_menu = target.into();
        } else {
            self.overlay_menu = Some(target.into());
        }
        self.menu_session.menu.clear();
        self.hovered_image = None;
        if self.core.program().theme.image_menus[target].uses_storage() {
            self.commands.push(AppCommand::ListSaves);
        }
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
                    speaker: h.speaker.clone(),
                    text: h.text.clone(),
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
                        ((*offset).max(0) as usize)
                            .min(core.state().history.len().saturating_sub(*limit as usize)),
                        *limit as usize,
                    ),
                ))
            })
            .collect()
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
        self.prepared_menu = None;
        let owns_error =
            self.diagnostic
                .as_ref()
                .and_then(|d| d.details.as_ref())
                .is_some_and(|d| {
                    d.request.is_some_and(|request| {
                        self.prepare.as_ref().is_some_and(|p| {
                            p.request == request && matches!(p.purpose, Purpose::Menu)
                        }) || self.content.get(&request).is_some_and(|p| {
                            matches!(
                                p.purpose,
                                ContentPurpose::Media {
                                    purpose: Purpose::Menu,
                                    ..
                                }
                            )
                        })
                    }) || (d.request.is_none()
                        && self
                            .failed_admission
                            .as_ref()
                            .is_some_and(|(p, _, _)| matches!(p, Purpose::Menu)))
                });
        if owns_error {
            self.error = None;
            self.diagnostic = None;
            self.status.clear();
        }
        if self
            .prepare
            .as_ref()
            .is_some_and(|p| matches!(p.purpose, Purpose::Menu))
            || self
                .failed_admission
                .as_ref()
                .is_some_and(|(p, _, _)| matches!(p, Purpose::Menu))
        {
            self.cancel_preparation();
            self.pauses.remove("prepare");
        }
        if self
            .deferred_prepare
            .as_ref()
            .is_some_and(|(_, p, _, _)| matches!(p, Purpose::Menu))
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
                    ContentPurpose::Media {
                        purpose: Purpose::Menu,
                        ..
                    }
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
        if self.screen != Screen::Menu {
            self.cancel_menu_preparation();
            return Ok(());
        }
        let Some(id) = self.active_menu_id() else {
            return Ok(());
        };
        let stamp = self.menu_asset_stamp(id);
        if self.prepared_menu.as_ref() == Some(&stamp)
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
                            speaker: h.speaker.clone(),
                            text: h.text.clone(),
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
                        && matches!(action, ImageMenuAction::SaveSlot {slot} if slot.resolve(&self.menu_session.locals) == Some(c.slot))
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
                    if let Some((name, value)) = menu.history_page(
                        window,
                        *delta,
                        &self.menu_session.locals,
                        self.core.state().history.len(),
                    ) {
                        self.menu_session.locals.insert(name, MenuValue::Int(value));
                    }
                }
                Ok(None)
            }
            ImageMenuAction::SaveSlot { slot } | ImageMenuAction::LoadSlot { slot } => {
                let Some(slot) = slot.resolve(&self.menu_session.locals) else {
                    return Ok(None);
                };
                if self.save_jobs.values().any(|(busy, _)| *busy == slot) {
                    return Ok(None);
                }
                if matches!(action, ImageMenuAction::LoadSlot { .. }) {
                    return Ok(self
                        .slots
                        .iter()
                        .any(|row| row.slot == slot && row.exists)
                        .then_some(UiAction::Load { slot }));
                }
                if self.screen == Screen::Title
                    || self.return_screen == Screen::Title
                    || self.slot_load.is_some()
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
        let task = self.menu_effects.alloc_task();
        self.menu_effects.sounds.insert(
            task,
            effects::UiVoice { task, session },
        );
        self.commands.push(AppCommand::AudioStart {
            domain: TimeDomain::ForegroundUi,
            task,
            asset: asset.into(),
            bus: AudioBus::Sfx,
            looped: false,
            position_us: Micros(0),
            gain: 1.,
            envelope: 1.,
            session,
        });
    }

    fn start_menu_music(&mut self, music: &nir_format::MenuMusic) {
        let session = self.generation.session;
        let task = self.menu_effects.alloc_task();
        self.menu_effects.music = Some(effects::UiVoice { task, session });
        self.commands.push(AppCommand::AudioStart {
            domain: TimeDomain::ForegroundUi,
            task,
            asset: music.asset.clone(),
            bus: music.bus,
            looped: true,
            position_us: Micros(0),
            gain: music.gain,
            envelope: 1.,
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
        match self.screen {
            // Overlay pages play only once their own media is resident; the
            // stamp carries page identity and the full generation.
            Screen::Menu => self.prepared_menu.as_ref() == Some(&self.menu_asset_stamp(id)),
            // Title closure pages are all prepared at boot (and re-prepared
            // by a device recovery), so readiness is simply "not loading":
            // the Preparing gap never sounds.
            Screen::Title => {
                !self.is_loading()
                    && self.failed_admission.is_none()
                    && self.error.is_none()
            }
            _ => false,
        }
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
                    });
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
        let close = self
            .active_menu_effects()
            .and_then(|effects| effects.close);
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

    pub(super) fn commit_close(&mut self, cancelled_slot_restore: bool) -> Result<()> {
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
            } => self.commit_close(cancelled_slot_restore)?,
            DeferredExitKind::ReadingClose { mode } => {
                let toggle = self.commit_reading_close(mode)?;
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
        let exit = self.menu_effects.take_finished_close(self.ui_clock_us.0);
        if exit.is_some() || self.menu_effects.fade.is_none() {
            self.menu_effects_clock = None;
        }
        self.menu_effects.settle_enter(self.ui_clock_us.0);
        if self.menu_effects.fade.is_none() {
            self.menu_effects_clock = None;
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
mod tests {
    use super::*;
    fn player() -> Player {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        program
            .requires
            .extend(["ui.menu-elements.v1".into(), "ui.menu-state.v1".into()]);
        let menu:ImageMenu=serde_json::from_value(serde_json::json!({
            "background":"bg.station","buttons":[],
            "locals":{"tab":{"type":"enum","initial":"first","values":["first","second","third"]},"slot":{"type":"int","initial":0,"min":0,"max":2}},
            "elements":[
                {"id":"label","rect":[20,20,300,60],"text_local":"tab","content":{"type":"text","text":"first","size":30,"color":[1,1,1,1]}},
                {"id":"second","rect":[20,100,200,60],"content":{"type":"hit_region","label":"Second","action":{"type":"set_local","local":"tab","value":"second"}}},
                {"id":"third","rect":[240,100,200,60],"content":{"type":"hit_region","label":"Third","action":{"type":"set_local","local":"tab","value":"third"}}},
                {"id":"slot","rect":[20,200,200,60],"visible_when":[{"type":"local","name":"tab","equals":"third"}],"content":{"type":"hit_region","label":"Slot","action":{"type":"set_local","local":"slot","value":2}}},
                {"id":"other","rect":[20,300,200,60],"content":{"type":"hit_region","label":"Other","action":{"type":"menu","menu":"other"}}},
                {"id":"entry","rect":[20,400,200,60],"enabled_when":[{"type":"local","name":"tab","equals":"third"}],"content":{"type":"hit_region","label":"Entry","action":{"type":"entry","function":"main"}}}
            ]
        })).unwrap();
        program
            .theme
            .image_menus
            .insert("title".into(), menu.clone());
        program.theme.image_menus.insert("other".into(), menu);
        let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
        p.cancel_preparation();
        p.pauses.remove("prepare");
        p
    }
    fn control(p: &Player, id: &str) -> UiAction {
        UiAction::MenuControl {
            instance: p.menu_session.instance,
            revision: p.menu_session.revision,
            control: id.into(),
        }
    }
    fn dispatch(p: &mut Player, action: UiAction) {
        p.action(action, 0, 1, &mut 1000).unwrap();
    }
    fn click(p: &mut Player, id: &str) {
        let a = control(p, id);
        dispatch(p, a);
    }
    #[test]
    fn local_actions_are_atomic_pure_and_reject_old_revisions_and_hidden_controls() {
        let mut p = player();
        let before = serde_json::to_value(p.core.state()).unwrap();
        let stale = control(&p, "third");
        click(&mut p, "second");
        let revision = p.menu_session.revision;
        dispatch(&mut p, stale);
        assert_eq!(p.menu_session.revision, revision);
        assert_eq!(
            p.menu_session.locals["tab"],
            MenuValue::Text("second".into())
        );
        click(&mut p, "slot");
        assert_eq!(p.menu_session.locals["slot"], MenuValue::Int(0));
        click(&mut p, "entry");
        assert_eq!(p.screen, Screen::Title);
        dispatch(
            &mut p,
            UiAction::ImageMenuEntry {
                function: "main".into(),
            },
        );
        assert_eq!(p.screen, Screen::Title);
        click(&mut p, "third");
        click(&mut p, "slot");
        assert_eq!(p.menu_session.locals["slot"], MenuValue::Int(2));
        assert_eq!(serde_json::to_value(p.core.state()).unwrap(), before);
        let model = p.model();
        let packet =
            nir_presentation::project(&model, 1280., 720., &nir_presentation::Messages::default());
        assert!(packet.texts.iter().any(|r| r.text == "third"));
        assert!(packet.semantics.iter().any(|n| n.label == "Slot"));
    }
    #[test]
    fn overlay_preserves_locals_but_reentry_and_navigation_invalidate_input() {
        let mut p = player();
        click(&mut p, "third");
        let stale = control(&p, "slot");
        dispatch(&mut p, UiAction::Settings);
        dispatch(&mut p, UiAction::Close);
        dispatch(&mut p, stale);
        assert_eq!(p.menu_session.locals["slot"], MenuValue::Int(0));
        assert_eq!(
            p.menu_session.locals["tab"],
            MenuValue::Text("third".into())
        );
        let stale = control(&p, "slot");
        click(&mut p, "other");
        assert_eq!(
            p.menu_session.locals["tab"],
            MenuValue::Text("first".into())
        );
        dispatch(&mut p, stale);
        assert_eq!(p.menu_session.locals["slot"], MenuValue::Int(0));
        let old = control(&p, "third");
        p.profile.insert("seen".into());
        dispatch(&mut p, old);
        assert_eq!(
            p.menu_session.locals["tab"],
            MenuValue::Text("first".into())
        );
    }
    #[test]
    fn projection_keeps_control_identity_across_conditional_siblings() {
        let mut p = player();
        let project = |p: &Player| {
            nir_presentation::project(
                &p.model(),
                1280.,
                720.,
                &nir_presentation::Messages::default(),
            )
        };
        let before = project(&p);
        let node = before
            .semantics
            .iter()
            .find(|n| n.label == "Other")
            .unwrap();
        let mut focus = nir_presentation::KeyboardFocus::default();
        focus.select(&before, (1, 0), Screen::Title, Some(node.id));
        click(&mut p, "third");
        let after = project(&p);
        let target = focus.node(&after, (1, 0), Screen::Title).unwrap();
        assert_eq!(target.label, "Other");
        assert_eq!(target.id, node.id);
        assert_ne!(target.action, node.action);
        assert!(target.action.same_focus_target(&node.action));
    }
    #[test]
    fn local_contract_rejects_unbounded_or_mistyped_data() {
        let p = player();
        let mut original: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        original.theme = (*p.core.program().theme).clone();
        original.requires = p.core.program().requires.clone();
        let mut broken = original.clone();
        broken.requires.retain(|c| c != "ui.menu-state.v1");
        assert!(Player::new(broken, "r".into(), "t".into()).is_err());
        for kind in 0..4 {
            let mut broken = original.clone();
            let menu = broken.theme.image_menus.get_mut("title").unwrap();
            match kind {
                0 => {
                    menu.locals.insert(
                        "tab".into(),
                        MenuLocal::Enum {
                            initial: "absent".into(),
                            values: vec!["first".into()],
                        },
                    );
                }
                1 => menu.elements[0].text_local = Some("absent".into()),
                2 => {
                    menu.elements[1].enabled_when = vec![MenuCondition::Local {
                        name: "slot".into(),
                        equals: MenuValue::Bool(true),
                    }]
                }
                _ => {
                    menu.locals.insert(
                        "slot".into(),
                        MenuLocal::Int {
                            initial: 0,
                            min: 1,
                            max: 2,
                        },
                    );
                }
            }
            assert!(Player::new(broken, "r".into(), "t".into()).is_err());
        }
    }
    fn service_program() -> Program {
        let mut p: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        p.requires.extend([
            "ui.menu-elements.v1".into(),
            "ui.menu-state.v1".into(),
            "ui.menu-services.v1".into(),
        ]);
        p.theme = (*player().core.program().theme).clone();
        p.theme.menu_overlay = Some("system".into());
        let image = p.assets["bg.station"].clone();
        p.assets.insert("menu.only".into(), image.clone());
        p.assets.insert("menu.child".into(), image);
        let menu:ImageMenu=serde_json::from_value(serde_json::json!({"background":"menu.only","buttons":[],"elements":[
            {"id":"speed","rect":[20,20,300,60],"text_preference":"text_speed","content":{"type":"text","text":"1.00","size":30,"color":[1,1,1,1]}},
            {"id":"increase","rect":[20,100,200,60],"content":{"type":"hit_region","label":"Faster","action":{"type":"adjust_preference","field":"text_speed","delta":0.25}}},
            {"id":"motion","rect":[240,100,200,60],"content":{"type":"hit_region","label":"Motion","action":{"type":"toggle_reduced_motion"}}},
            {"id":"child","rect":[20,200,200,60],"content":{"type":"hit_region","label":"Child","action":{"type":"menu","menu":"system.child"}}},
            {"id":"close","rect":[20,300,200,60],"content":{"type":"hit_region","label":"Close","action":{"type":"close"}}}
        ]})).unwrap();
        let mut child = menu.clone();
        child.background = "menu.child".into();
        p.theme.image_menus.insert("system".into(), menu);
        p.theme.image_menus.insert("system.child".into(), child);
        p
    }
    fn navigation_program() -> Program {
        let mut p = service_program();
        p.requires.push("ui.menu-navigation.v1".into());
        for id in ["system", "system.child"] {
            let menu = p.theme.image_menus.get_mut(id).unwrap();
            menu.locals.insert(
                "selected".into(),
                MenuLocal::Int {
                    initial: 0,
                    min: 0,
                    max: 9,
                },
            );
            menu.elements.iter_mut().find(|e| e.id == "child").unwrap().content = serde_json::from_value(serde_json::json!({
                "type":"hit_region","label":"Child","action":{"type":"push_menu","menu":"system.child"}
            })).unwrap();
            menu.elements.push(serde_json::from_value(serde_json::json!({
                "id":"select","rect":[20,400,200,60],"content":{"type":"hit_region","label":"Select","action":{"type":"set_local","local":"selected","value":2}}
            })).unwrap());
            menu.elements.push(serde_json::from_value(serde_json::json!({
                "id":"back","rect":[240,400,200,60],"content":{"type":"hit_region","label":"Parent","action":{"type":"back"}}
            })).unwrap());
        }
        p
    }
    #[test]
    fn history_availability_is_read_only_and_rechecked_before_navigation() {
        let mut program = navigation_program();
        program
            .requires
            .push("ui.menu-history-availability.v1".into());
        let menu = program.theme.image_menus.get_mut("system").unwrap();
        menu.elements
            .iter_mut()
            .find(|e| e.id == "child")
            .unwrap()
            .visible_when = vec![MenuCondition::HistoryAvailable { available: true }];
        menu.elements
            .iter_mut()
            .find(|e| e.id == "select")
            .unwrap()
            .enabled_when = vec![MenuCondition::HistoryAvailable { available: true }];
        let title = menu.clone();
        program.theme.image_menus.insert("title".into(), title);
        let mut missing = program.clone();
        missing
            .requires
            .retain(|c| c != "ui.menu-history-availability.v1");
        assert_eq!(
            Player::new(missing, "r".into(), "t".into())
                .err()
                .unwrap()
                .code,
            "E_CAPABILITY"
        );
        let mut p = Player::new(program, "r".into(), "t".into()).unwrap();
        let commands = p.pump(vec![], 1000);
        settle(&mut p, commands);
        let project = |p: &Player| {
            nir_presentation::project(
                &p.model(),
                1280.,
                720.,
                &nir_presentation::Messages::default(),
            )
        };
        let before = serde_json::to_value(p.core.snapshot()).unwrap();
        let packet = project(&p);
        assert!(!packet.semantics.iter().any(|n| n.label == "Child"));
        let select = menu_control(&packet, "Select");
        assert!(!select.enabled);
        let child = control(&p, "child");
        pump_action(&mut p, child);
        let select = control(&p, "select");
        pump_action(&mut p, select);
        assert_eq!(p.menu_session.depth(), 0);
        assert_eq!(p.menu_session.locals["selected"], MenuValue::Int(0));
        assert_eq!(serde_json::to_value(p.core.snapshot()).unwrap(), before);
        assert!(p.menu_session.flow.is_empty());

        let commands = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, commands);
        assert!(menu_control(&project(&p), "Child").enabled);
        assert!(menu_control(&project(&p), "Select").enabled);
        assert!(p.menu_session.flow.is_empty()); // the parent keeps no history text
        let stale = control(&p, "child");
        let initial = p.core.snapshot();
        let mut empty = initial.clone();
        empty.history.clear();
        p.core = Core::restore(p.validated.clone(), empty, "r").unwrap();
        let (instance, revision) = (p.menu_session.instance, p.menu_session.revision);
        assert!(p
            .resolve_menu_control(instance, revision, "child", 0, 1)
            .unwrap()
            .is_none());
        assert!(p
            .resolve_menu_control(instance, revision, "select", 0, 1)
            .unwrap()
            .is_none());
        p.sync_menu_state().unwrap();
        assert!(p.menu_session.revision > revision);
        assert!(!project(&p).semantics.iter().any(|n| n.label == "Child"));
        p.core = Core::restore(p.validated.clone(), initial, "r").unwrap();
        pump_action(&mut p, stale);
        assert_eq!(p.menu_session.depth(), 0);
        let before = serde_json::to_value(p.core.snapshot()).unwrap();
        let child = control(&p, "child");
        let commands = pump_action(&mut p, child);
        settle(&mut p, commands);
        assert_eq!(p.menu_session.depth(), 1);
        assert_eq!(serde_json::to_value(p.core.snapshot()).unwrap(), before);

        fn menu_control<'a>(
            packet: &'a nir_presentation::DrawPacket,
            label: &str,
        ) -> &'a nir_presentation::SemanticNode {
            packet.semantics.iter().find(|n| n.label == label).unwrap()
        }
    }
    #[test]
    fn navigation_preserves_bounded_parent_locals_releases_media_and_rejects_old_input() {
        let mut p = Player::new(navigation_program(), "r".into(), "t".into()).unwrap();
        let commands = p.pump(vec![], 1000);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, commands);
        let before = serde_json::to_value(p.core.snapshot()).unwrap();
        let commands = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, commands);
        let select = control(&p, "select");
        pump_action(&mut p, select);
        let parent_instance = p.menu_session.instance;
        let stale_parent = control(&p, "select");
        let revision = p.menu_session.revision;
        let back = control(&p, "back");
        pump_action(&mut p, back);
        assert_eq!(p.menu_session.revision, revision);
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert!(
            !packet
                .semantics
                .iter()
                .find(|n| n.label == "Parent")
                .unwrap()
                .enabled
        );
        // The uncredentialed legacy route cannot invoke a push control.
        pump_action(
            &mut p,
            UiAction::ImageMenu {
                menu: "system.child".into(),
            },
        );
        assert_eq!(p.active_menu_id(), Some("system"));
        let push = control(&p, "child");
        let commands = pump_action(&mut p, push);
        settle(&mut p, commands);
        assert_eq!(p.menu_session.depth(), 1);
        assert_eq!(p.menu_session.locals["selected"], MenuValue::Int(0));
        assert!(!p.retained_assets().contains("menu.only"));
        assert!(p.retained_assets().contains("menu.child"));
        let stale_child = control(&p, "close");
        let select = control(&p, "select");
        pump_action(&mut p, select);
        let back = control(&p, "back");
        let commands = pump_action(&mut p, back);
        settle(&mut p, commands);
        assert_eq!(p.screen, Screen::Menu);
        assert!(p.paused());
        assert_eq!(p.active_menu_id(), Some("system"));
        assert_eq!(p.menu_session.locals["selected"], MenuValue::Int(2));
        assert!(p.menu_session.instance > parent_instance);
        assert_eq!(p.menu_session.depth(), 0);
        assert!(p.retained_assets().contains("menu.only"));
        assert!(!p.retained_assets().contains("menu.child"));
        let current = p.menu_session.instance;
        pump_action(&mut p, stale_parent);
        pump_action(&mut p, stale_child);
        assert_eq!(p.screen, Screen::Menu);
        assert_eq!(p.menu_session.instance, current);
        // Recursive declarations are finite at runtime and projection agrees.
        for depth in 1..=nir_format::MAX_MENU_PARENTS {
            let push = control(&p, "child");
            let commands = pump_action(&mut p, push);
            settle(&mut p, commands);
            assert_eq!(p.model().menu_depth, depth);
        }
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert!(
            !packet
                .semantics
                .iter()
                .find(|n| n.label == "Child")
                .unwrap()
                .enabled
        );
        let revision = p.menu_session.revision;
        let push = control(&p, "child");
        pump_action(&mut p, push);
        assert_eq!(p.menu_session.depth(), nir_format::MAX_MENU_PARENTS);
        assert_eq!(p.menu_session.revision, revision);
        for depth in (0..nir_format::MAX_MENU_PARENTS).rev() {
            let commands = pump_action(&mut p, UiAction::Close);
            settle(&mut p, commands);
            assert_eq!(p.menu_session.depth(), depth);
            assert_eq!(p.screen, Screen::Menu);
        }
        assert_eq!(p.menu_session.locals["selected"], MenuValue::Int(2));
        assert_eq!(serde_json::to_value(p.core.snapshot()).unwrap(), before);
        let commands = pump_action(&mut p, UiAction::Close);
        settle(&mut p, commands);
        assert_eq!(p.screen, Screen::Story);
        assert_eq!(p.menu_session.depth(), 0);
        let commands = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, commands);
        assert_eq!(p.menu_session.locals["selected"], MenuValue::Int(0));
    }
    #[test]
    fn navigation_can_return_during_child_preparation_and_discard_late_completion() {
        let mut p = Player::new(navigation_program(), "r".into(), "t".into()).unwrap();
        let commands = p.pump(vec![], 1000);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, commands);
        let push = control(&p, "child");
        let commands = pump_action(&mut p, push);
        let request = commands
            .iter()
            .find_map(|c| match c {
                AppCommand::GetAssets { request, .. } => Some(*request),
                _ => None,
            })
            .unwrap();
        assert!(p.is_loading());
        assert_eq!(p.menu_session.depth(), 1);
        let commands = pump_action(&mut p, UiAction::Close);
        assert!(commands
            .iter()
            .any(|c| matches!(c,AppCommand::CancelAssets {request:r} if *r==request)));
        settle(&mut p, commands);
        let instance = p.menu_session.instance;
        p.pump(vec![AppEvent::PresentationReady { request }], 1000);
        assert_eq!(p.active_menu_id(), Some("system"));
        assert_eq!(p.menu_session.depth(), 0);
        assert_eq!(p.menu_session.instance, instance);
        assert!(p.paused());
        assert!(!p.is_loading());
        assert!(p.error.is_none());
    }
    #[test]
    fn navigation_returns_from_failed_child_to_parent_without_clearing_story_pause() {
        let mut p = Player::new(navigation_program(), "r".into(), "t".into()).unwrap();
        let commands = p.pump(vec![], 1000);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, commands);
        let push = control(&p, "child");
        let commands = pump_action(&mut p, push);
        let request = commands
            .iter()
            .find_map(|c| match c {
                AppCommand::GetAssets { request, .. } => Some(*request),
                _ => None,
            })
            .unwrap();
        p.pump(
            vec![AppEvent::AssetFailed {
                request,
                message: "fixture child failure".into(),
            }],
            1000,
        );
        assert!(p.error.is_some());
        assert!(!p.model().authored_menu);
        let commands = pump_action(&mut p, UiAction::Close);
        settle(&mut p, commands);
        assert_eq!(p.active_menu_id(), Some("system"));
        assert_eq!(p.menu_session.depth(), 0);
        assert!(p.error.is_none());
        assert!(p.paused());
        assert!(p.model().authored_menu);
        p.pump(vec![AppEvent::PresentationReady { request }], 1000);
        assert_eq!(p.active_menu_id(), Some("system"));
    }
    #[test]
    fn navigation_title_chain_survives_temporary_overlay_and_new_session_discards_it() {
        let mut program = navigation_program();
        program.theme.image_menus.get_mut("title").unwrap().elements.push(serde_json::from_value(serde_json::json!({
            "id":"push","rect":[600,500,100,60],"content":{"type":"hit_region","label":"Child","action":{"type":"push_menu","menu":"system.child"}}
        })).unwrap());
        let mut p = Player::new(program, "r".into(), "t".into()).unwrap();
        let commands = p.pump(vec![], 1000);
        settle(&mut p, commands);
        let third = control(&p, "third");
        pump_action(&mut p, third);
        let push = control(&p, "push");
        let commands = pump_action(&mut p, push);
        settle(&mut p, commands);
        assert_eq!(p.screen, Screen::Title);
        assert_eq!(p.menu_session.depth(), 1);
        let select = control(&p, "select");
        pump_action(&mut p, select);
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert_eq!(
            nir_presentation::pointer_action(&packet, &p.model(), 10., 10., 2),
            Some(UiAction::Close)
        );
        let commands = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, commands);
        assert_eq!(p.menu_session.depth(), 0);
        let push = control(&p, "child");
        let commands = pump_action(&mut p, push);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::Close);
        settle(&mut p, commands);
        assert_eq!(p.screen, Screen::Menu);
        let commands = pump_action(&mut p, UiAction::Close);
        settle(&mut p, commands);
        assert_eq!(p.screen, Screen::Title);
        assert_eq!(p.active_menu_id(), Some("system.child"));
        assert_eq!(p.menu_session.depth(), 1);
        assert_eq!(p.menu_session.locals["selected"], MenuValue::Int(2));
        let commands = pump_action(&mut p, UiAction::Close);
        settle(&mut p, commands);
        assert_eq!(p.active_menu_id(), Some("title"));
        assert_eq!(
            p.menu_session.locals["tab"],
            MenuValue::Text("third".into())
        );
        let push = control(&p, "push");
        let commands = pump_action(&mut p, push);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, commands);
        assert_eq!(p.screen, Screen::Story);
        assert_eq!(p.menu_session.depth(), 0);
        assert!(p.menu_session.suspended_title.is_none());
    }
    #[test]
    fn navigation_validates_capability_targets_and_overlay_entry_closure() {
        for case in 0..5 {
            let mut p = navigation_program();
            match case {
                0=>p.requires.retain(|c| c!="ui.menu-navigation.v1"),
                1=>{p.theme.image_menus.remove("system.child");},
                3=>{
                    let target="target".repeat(30);
                    p.theme.image_menus.insert(target.clone(),p.theme.image_menus["system.child"].clone());
                    if let MenuContent::HitRegion {action,..}=&mut p.theme.image_menus.get_mut("system").unwrap().elements.iter_mut().find(|e| e.id=="child").unwrap().content {
                        *action=ImageMenuAction::PushMenu {menu:target};
                    }
                },
                4=>{
                    let menu=p.theme.image_menus.remove("system").unwrap();
                    let key="parent".repeat(30);
                    p.theme.menu_overlay=Some(key.clone());p.theme.image_menus.insert(key,menu);
                },
                _=>p.theme.image_menus.get_mut("system.child").unwrap().elements.push(serde_json::from_value(serde_json::json!({
                    "id":"entry","rect":[500,400,100,60],"content":{"type":"hit_region","label":"Entry","action":{"type":"entry","function":"main"}}
                })).unwrap())
            }
            assert!(
                Player::new(p, "r".into(), "t".into()).is_err(),
                "case {case}"
            );
        }
        let mut old = service_program();
        let menu = old.theme.image_menus.remove("system").unwrap();
        let key = "legacy".repeat(30);
        old.theme.menu_overlay = Some(key.clone());
        old.theme.image_menus.insert(key, menu);
        assert!(Player::new(old, "r".into(), "t".into()).is_ok());
    }
    #[test]
    fn value_controls_commit_once_reject_stale_and_validate_bounds() {
        let mut program = service_program();
        program.requires.push("ui.menu-values.v1".into());
        let menu = program.theme.image_menus.get_mut("system").unwrap();
        menu.elements.push(serde_json::from_value(serde_json::json!({
            "id":"range","rect":[20,400,300,60],"content":{"type":"range","label":"Speed","binding":{"type":"preference","field":"text_speed"},"min":0.25,"max":4.0,"step":0.25}
        })).unwrap());
        menu.elements.push(serde_json::from_value(serde_json::json!({
            "id":"toggle","rect":[20,470,300,60],"content":{"type":"toggle","label":"Motion","binding":{"type":"reduced_motion"}}
        })).unwrap());
        let mut p = Player::new(program.clone(), "r".into(), "t".into()).unwrap();
        let commands = p.pump(vec![], 1000);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, commands);
        let action = UiAction::MenuValue {
            instance: p.menu_session.instance,
            revision: p.menu_session.revision,
            control: "range".into(),
            value: MenuValueInput::Number(2.12),
        };
        pump_action(&mut p, action.clone());
        assert_eq!(p.preferences.text_speed, 2.);
        let revision = p.menu_session.revision;
        pump_action(&mut p, action);
        assert_eq!(p.menu_session.revision, revision);
        let action = UiAction::MenuValue {
            instance: p.menu_session.instance,
            revision,
            control: "range".into(),
            value: MenuValueInput::Number(9.),
        };
        pump_action(&mut p, action);
        assert_eq!(p.preferences.text_speed, 2.);
        assert_eq!(p.menu_session.revision, revision);
        let action = UiAction::MenuValue {
            instance: p.menu_session.instance,
            revision,
            control: "toggle".into(),
            value: MenuValueInput::Bool(true),
        };
        pump_action(&mut p, action);
        assert!(p.preferences.reduced_motion);
        program.requires.retain(|v| v != "ui.menu-values.v1");
        assert!(Player::new(program, "r".into(), "t".into()).is_err());
    }
    fn service_player() -> Player {
        Player::new(service_program(), "release".into(), "Test".into()).unwrap()
    }
    fn reading_program() -> Program {
        let mut program = service_program();
        program.requires.push("ui.menu-reading.v1".into());
        let menu = program.theme.image_menus.get_mut("system").unwrap();
        menu.locals.insert(
            "cursor".into(),
            MenuLocal::Int {
                initial: 0,
                min: 0,
                max: 3,
            },
        );
        for (i, mode) in ["auto", "skip_read", "peek_story"].iter().enumerate() {
            menu.elements.push(serde_json::from_value(serde_json::json!({
                "id":mode,"rect":[500,100+i*100,300,60],
                "content":{"type":"hit_region","label":mode,"action":{"type":"reading","mode":mode}}
            })).unwrap());
        }
        program
    }
    fn reading_player() -> Player {
        let mut p = Player::new(reading_program(), "r".into(), "t".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        assert!(p.core.dialogue().is_some());
        p
    }
    #[test]
    fn reading_visibility_guards_are_rechecked_for_non_reading_actions_and_values() {
        let mut program = reading_program();
        let menu = program.theme.image_menus.get_mut("system").unwrap();
        menu.elements.push(
            serde_json::from_value(serde_json::json!({
                "id":"guarded.close","rect":[0,0,100,40],
                "visible_when":[{"type":"reading_available","mode":"auto","available":true}],
                "content":{"type":"hit_region","label":"Guarded close","action":{"type":"close"}}
            }))
            .unwrap(),
        );
        menu.elements.push(serde_json::from_value(serde_json::json!({
            "id":"cursor.value","rect":[0,50,100,40],
            "content":{"type":"range","label":"Cursor","binding":{"type":"local","name":"cursor"},"min":0,"max":3,"step":1}
        })).unwrap());
        program.requires.push("ui.menu-values.v1".into());
        let mut p = Player::new(program, "r".into(), "t".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        let stale = control(&p, "guarded.close");
        let pause = p.acquire_pause("other-owner");
        pump_action(&mut p, stale);
        assert_eq!(p.screen, Screen::Menu);
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert!(!packet.semantics.iter().any(|s| s.label == "Guarded close"));
        drop(pause);
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let a = control(&p, "peek_story");
        pump_action(&mut p, a);
        // Even a current identity cannot write values while the menu is hidden.
        assert!(p
            .resolve_menu_value(
                p.menu_session.instance,
                p.menu_session.revision,
                "cursor.value",
                MenuValueInput::Number(2.)
            )
            .unwrap()
            .is_none());
        assert_eq!(p.menu_session.locals["cursor"], MenuValue::Int(0));
        pump_action(&mut p, UiAction::RestoreInterface);
        let action = control(&p, "guarded.close");
        pump_action(&mut p, action);
        assert_eq!(p.screen, Screen::Story);
    }
    #[test]
    fn story_projection_is_explicit_pure_and_rechecks_changes_before_dispatch() {
        let mut program = reading_program();
        program.requires.push("ui.menu-story.v1".into());
        program
            .variables
            .insert("route_locked".into(), Value::Bool(false));
        program
            .variables
            .insert("private_counter".into(), Value::I32(37));
        let menu = program.theme.image_menus.get_mut("system").unwrap();
        menu.story_exports
            .insert("locked".into(), "route_locked".into());
        menu.elements
            .iter_mut()
            .find(|e| e.id == "auto")
            .unwrap()
            .visible_when
            .push(MenuCondition::Story {
                name: "locked".into(),
                equals: MenuValue::Bool(false),
            });
        let mut p = Player::new(program, "r".into(), "t".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        let stale = control(&p, "auto");
        let initial = p.core.snapshot();
        let model = p.model();
        assert_eq!(
            model.menu_story,
            BTreeMap::from([("locked".into(), MenuValue::Bool(false))])
        );
        for _ in 0..3 {
            let _ = nir_presentation::project(
                &model,
                1280.,
                720.,
                &nir_presentation::Messages::default(),
            );
        }
        assert_eq!(
            serde_json::to_value(p.core.snapshot()).unwrap(),
            serde_json::to_value(&initial).unwrap()
        );
        // Change authoritative story state without refreshing the menu cache, as
        // a restore/host turn boundary could do. Commit must read current values.
        let mut changed = initial.clone();
        changed
            .variables
            .insert("route_locked".into(), Value::Bool(true));
        p.core = Core::restore(p.validated.clone(), changed, "r").unwrap();
        let (instance, revision) = (p.menu_session.instance, p.menu_session.revision);
        assert!(p
            .resolve_menu_control(instance, revision, "auto", 0, 1)
            .unwrap()
            .is_none());
        assert_eq!(p.screen, Screen::Menu);
        p.sync_menu_state().unwrap();
        assert!(p.menu_session.revision > revision);
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert!(!packet.semantics.iter().any(|n| n.label == "auto"));
        p.core = Core::restore(p.validated.clone(), initial, "r").unwrap();
        pump_action(&mut p, stale);
        assert_eq!(p.screen, Screen::Menu);
        assert!(!p.auto);
        assert_eq!(p.core.state().variables["private_counter"], Value::I32(37));
        let current = control(&p, "auto");
        pump_action(&mut p, current);
        assert_eq!(p.screen, Screen::Story);
        assert!(p.auto);
    }
    #[test]
    fn menu_reading_resumes_once_without_advancing_or_replaying_old_input() {
        for mode in [MenuReadingMode::Auto, MenuReadingMode::SkipRead] {
            let mut p = reading_player();
            let id = match mode {
                MenuReadingMode::Auto => "auto",
                MenuReadingMode::SkipRead => "skip_read",
                MenuReadingMode::PeekStory => "peek_story",
            };
            if mode == MenuReadingMode::SkipRead {
                let (_, d) = p.core.dialogue().unwrap();
                p.profile
                    .insert(format!("read:{}:{}", d.text_id, d.meaning_revision));
            }
            // Selecting Auto means enable, including when it was already on.
            p.auto = mode == MenuReadingMode::Auto;
            let c = pump_action(&mut p, UiAction::Menu);
            settle(&mut p, c);
            let interaction = p.current_interaction();
            let a = control(&p, id);
            let stale_hide = control(&p, "peek_story");
            pump_action(&mut p, a.clone());
            assert_eq!(p.screen, Screen::Story);
            assert_eq!(p.auto, mode == MenuReadingMode::Auto);
            assert_eq!(p.skip, mode == MenuReadingMode::SkipRead);
            assert_eq!(p.interface_hidden, mode == MenuReadingMode::PeekStory);
            assert_eq!(p.current_interaction(), interaction);
            assert!(!p.paused());
            pump_action(&mut p, a);
            pump_action(&mut p, stale_hide);
            assert_eq!(p.auto, mode == MenuReadingMode::Auto);
            assert_eq!(p.interface_hidden, mode == MenuReadingMode::PeekStory);
            assert_eq!(p.current_interaction(), interaction);
        }
    }
    #[test]
    fn reading_permissions_disable_projection_and_recheck_at_commit() {
        let mut p = reading_player();
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        let a = control(&p, "skip_read");
        pump_action(&mut p, a);
        assert_eq!(p.screen, Screen::Menu);
        assert!(!p.skip);
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert!(packet
            .semantics
            .iter()
            .any(|s| s.label == "skip_read" && !s.enabled));
        let stale = control(&p, "auto");
        let pause = p.acquire_pause("other-owner");
        pump_action(&mut p, stale.clone());
        assert_eq!(p.screen, Screen::Menu);
        assert!(p.model().menu_reading_modes.is_empty());
        drop(pause);
        pump_action(&mut p, stale);
        assert_eq!(p.screen, Screen::Menu);
        let a = control(&p, "auto");
        pump_action(&mut p, a);
        assert_eq!(p.screen, Screen::Story);
        assert!(p.auto);
    }
    #[test]
    fn menu_peek_retains_page_state_pause_and_reading_mode_until_restore() {
        let mut p = reading_player();
        p.auto = true;
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        p.menu_session
            .locals
            .insert("cursor".into(), MenuValue::Int(2));
        let instance = p.menu_session.instance;
        let before = serde_json::to_value(p.core.state()).unwrap();
        let stale = control(&p, "auto");
        let a = control(&p, "peek_story");
        pump_action(&mut p, a);
        assert_eq!(p.screen, Screen::Menu);
        assert_eq!(p.presentation_screen(), Screen::Story);
        assert!(p.interface_hidden && p.paused() && p.auto);
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert_eq!(packet.semantics.len(), 1);
        assert!(matches!(
            packet.semantics[0].action,
            UiAction::RestoreInterface
        ));
        let c = p.pump(vec![AppEvent::Tick { delta_us: 100_000 }], 1000);
        settle(&mut p, c);
        assert_eq!(serde_json::to_value(p.core.state()).unwrap(), before);
        pump_action(&mut p, stale);
        assert!(p.interface_hidden);
        pump_action(&mut p, UiAction::RestoreInterface);
        assert_eq!(p.presentation_screen(), Screen::Menu);
        assert!(!p.interface_hidden);
        assert!(p.paused() && p.auto);
        assert_eq!(p.menu_session.instance, instance);
        assert_eq!(p.menu_session.locals["cursor"], MenuValue::Int(2));
        assert_eq!(serde_json::to_value(p.core.state()).unwrap(), before);
    }
    #[test]
    fn menu_peek_can_hide_choices_without_selecting_or_replacing_them() {
        let mut program = reading_program();
        program.functions.get_mut("main").unwrap().entry = "choose".into();
        let mut p = Player::new(program, "r".into(), "t".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        assert!(p.core.state().choice.is_some());
        let before = serde_json::to_value(p.core.state()).unwrap();
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        assert_eq!(
            p.model().menu_reading_modes,
            BTreeSet::from([MenuReadingMode::PeekStory])
        );
        let a = control(&p, "peek_story");
        pump_action(&mut p, a);
        assert!(p.interface_hidden && p.paused());
        assert_eq!(p.model().screen, Screen::Story);
        pump_action(&mut p, UiAction::Close);
        assert!(!p.interface_hidden);
        assert_eq!(p.model().screen, Screen::Menu);
        assert_eq!(serde_json::to_value(p.core.state()).unwrap(), before);
    }
    #[test]
    fn reading_actions_require_capability_and_story_context() {
        let mut program = reading_program();
        program.requires.retain(|c| c != "ui.menu-reading.v1");
        assert!(Player::new(program, "r".into(), "t".into())
            .err()
            .unwrap()
            .to_string()
            .contains("ui.menu-reading.v1"));
        let mut p = Player::new(reading_program(), "r".into(), "t".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        assert!(p.model().menu_reading_modes.is_empty());
        let a = control(&p, "auto");
        pump_action(&mut p, a);
        assert_eq!(p.screen, Screen::Menu);
        assert!(!p.auto);
    }
    fn settle(p: &mut Player, mut commands: Vec<AppCommand>) -> Vec<AppCommand> {
        let mut output = vec![];
        for _ in 0..64 {
            if commands.is_empty() {
                return output;
            }
            let mut next = vec![];
            for command in commands {
                match command {
                    AppCommand::GetAssets {
                        request, assets, ..
                    } => {
                        for asset in assets {
                            next.extend(
                                p.pump(vec![AppEvent::AssetReady { request, asset }], 1000),
                            );
                        }
                    }
                    AppCommand::PreparePresentation { request } => {
                        next.extend(p.pump(vec![AppEvent::PresentationReady { request }], 1000))
                    }
                    command => output.push(command),
                }
            }
            commands = next;
        }
        panic!("preparation did not settle")
    }
    fn pump_action(p: &mut Player, action: UiAction) -> Vec<AppCommand> {
        p.pump(
            vec![AppEvent::Action {
                action,
                interaction: p.current_interaction(),
                sequence: p.core.state().last_input + 1,
                session: p.generation.session,
            }],
            1000,
        )
    }
    #[test]
    fn overlay_prepares_only_active_media_and_preferences_use_existing_service() {
        let mut p = service_player();
        assert!(!p.retained_assets().contains("menu.only"));
        assert!(!p.retained_assets().contains("menu.child"));
        let commands = p.pump(vec![], 1000);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, commands);
        assert_eq!(p.screen, Screen::Story);
        assert!(!p.retained_assets().contains("menu.only"));
        let before = serde_json::to_value(p.core.state()).unwrap();
        let commands = pump_action(&mut p, UiAction::Menu);
        assert!(p.is_loading());
        settle(&mut p, commands);
        assert_eq!(p.active_menu_id(), Some("system"));
        assert!(p.paused());
        assert!(p.retained_assets().contains("menu.only"));
        assert!(!p.retained_assets().contains("menu.child"));
        let stale = control(&p, "motion");
        let increase = control(&p, "increase");
        let commands = pump_action(&mut p, increase);
        assert!(commands
            .iter()
            .any(|c| matches!(c, AppCommand::PersistPreferences { .. })));
        assert_eq!(p.preferences.text_speed, 1.25);
        pump_action(&mut p, stale);
        assert!(!p.preferences.reduced_motion);
        assert_eq!(serde_json::to_value(p.core.state()).unwrap(), before);
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert!(packet.texts.iter().any(|t| t.text == "1.25"));
        let child = control(&p, "child");
        let commands = pump_action(&mut p, child);
        settle(&mut p, commands);
        assert_eq!(p.active_menu_id(), Some("system.child"));
        assert!(p.retained_assets().contains("menu.child"));
        assert!(!p.retained_assets().contains("menu.only"));
        let commands = pump_action(&mut p, UiAction::Close);
        settle(&mut p, commands);
        assert_eq!(p.screen, Screen::Story);
        assert!(!p.retained_assets().contains("menu.child"));
        let commands = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, commands);
        assert_eq!(p.active_menu_id(), Some("system"));
    }
    #[test]
    fn leaving_during_overlay_preparation_rejects_late_ready_and_preserves_title_locals() {
        let mut p = service_player();
        let commands = p.pump(vec![], 1000);
        settle(&mut p, commands);
        let third = control(&p, "third");
        pump_action(&mut p, third);
        let commands = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::Close);
        settle(&mut p, commands);
        assert_eq!(
            p.menu_session.locals["tab"],
            MenuValue::Text("third".into())
        );
        let commands = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::Menu);
        let request = commands
            .iter()
            .find_map(|c| {
                if let AppCommand::GetAssets { request, .. } = c {
                    Some(*request)
                } else {
                    None
                }
            })
            .unwrap();
        let closing = pump_action(&mut p, UiAction::Close);
        assert!(closing
            .iter()
            .any(|c| matches!(c,AppCommand::CancelAssets {request:r} if *r==request)));
        p.pump(vec![AppEvent::PresentationReady { request }], 1000);
        assert_eq!(p.screen, Screen::Story);
        assert!(!p.is_loading());
        assert!(p.error.is_none());
    }
    #[test]
    fn overlay_contract_rejects_unknown_roots_unsafe_entries_and_invalid_bindings() {
        let mut p = service_program();
        p.theme.image_menus.remove("title");
        let default_title = Player::new(p, "r".into(), "t".into()).unwrap();
        assert!(!default_title.model().authored_menu);
        for kind in 0..5 {
            let mut p = service_program();
            match kind {
                0 => p.requires.retain(|c| c != "ui.menu-services.v1"),
                1 => p.theme.menu_overlay = Some("absent".into()),
                2 => {
                    p.theme.image_menus.get_mut("system").unwrap().elements[0].text_local =
                        Some("absent".into())
                }
                3 => {
                    p.theme.image_menus.get_mut("system").unwrap().elements[1].content =
                        MenuContent::HitRegion {
                            label: "bad".into(),
                            action: ImageMenuAction::AdjustPreference {
                                field: MenuPreference::TextSpeed,
                                delta: f32::INFINITY,
                            },
                            requires: None,
                        }
                }
                _ => {
                    p.theme
                        .image_menus
                        .get_mut("system.child")
                        .unwrap()
                        .elements[1]
                        .content = MenuContent::HitRegion {
                        label: "bad".into(),
                        action: ImageMenuAction::Entry {
                            function: "main".into(),
                        },
                        requires: None,
                    }
                }
            }
            assert!(Player::new(p, "r".into(), "t".into()).is_err());
        }
    }
    #[test]
    fn failed_overlay_keeps_builtin_recovery_and_close_discards_only_its_error() {
        let mut p = service_player();
        let commands = p.pump(vec![], 1000);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, commands);
        let commands = pump_action(&mut p, UiAction::Menu);
        let request = commands
            .iter()
            .find_map(|c| {
                if let AppCommand::GetAssets { request, .. } = c {
                    Some(*request)
                } else {
                    None
                }
            })
            .unwrap();
        p.pump(
            vec![AppEvent::AssetFailed {
                request,
                message: "fixture resource failure".into(),
            }],
            1000,
        );
        assert!(p.error.is_some());
        assert!(!p.model().authored_menu);
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert!(packet.semantics.iter().any(|n| n.action == UiAction::Close));
        assert!(!packet
            .quads
            .iter()
            .any(|q| q.asset.as_deref() == Some("menu.only")));
        pump_action(&mut p, UiAction::Close);
        assert!(p.error.is_none());
        assert!(!p.paused());
        assert_eq!(p.screen, Screen::Story);
    }
    #[test]
    fn reusing_one_definition_for_title_and_overlay_does_not_share_local_values() {
        let mut program = service_program();
        let mut shared = program.theme.image_menus["system"].clone();
        shared.locals.insert(
            "count".into(),
            MenuLocal::Int {
                initial: 0,
                min: 0,
                max: 1,
            },
        );
        shared.elements[1].content = MenuContent::HitRegion {
            label: "set".into(),
            action: ImageMenuAction::SetLocal {
                local: "count".into(),
                value: MenuValue::Int(1),
            },
            requires: None,
        };
        program.theme.image_menus.insert("title".into(), shared);
        program.theme.menu_overlay = Some("title".into());
        let mut p = Player::new(program, "r".into(), "t".into()).unwrap();
        let commands = p.pump(vec![], 1000);
        settle(&mut p, commands);
        let set = control(&p, "increase");
        pump_action(&mut p, set);
        assert_eq!(p.menu_session.locals["count"], MenuValue::Int(1));
        let commands = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, commands);
        assert_eq!(p.menu_session.locals["count"], MenuValue::Int(0));
        pump_action(&mut p, UiAction::Close);
        assert_eq!(p.menu_session.locals["count"], MenuValue::Int(1));
    }

    fn storage_program() -> Program {
        let mut p = service_program();
        p.requires.push("ui.menu-storage.v1".into());
        p.theme.image_menus.insert("system".into(),serde_json::from_value(serde_json::json!({
            "background":"menu.only","buttons":[],
            "locals":{"selected":{"type":"int","initial":0,"min":0,"max":2}},
            "elements":[
                {"id":"label","rect":[20,20,300,60],"text_slot":{"type":"local","name":"selected"},"content":{"type":"text","text":"Empty","size":30,"color":[1,1,1,1]}},
                {"id":"save","rect":[20,100,200,60],"content":{"type":"hit_region","label":"Save selected","action":{"type":"save_slot","slot":{"type":"local","name":"selected"}}}},
                {"id":"load","rect":[240,100,200,60],"content":{"type":"hit_region","label":"Load selected","action":{"type":"load_slot","slot":{"type":"local","name":"selected"}}}}
            ]
        })).unwrap());
        p
    }
    fn storage_player() -> Player {
        let mut p = Player::new(storage_program(), "release".into(), "Test".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        p
    }
    fn save_control(p: &mut Player) -> Vec<AppCommand> {
        let action = control(p, "save");
        pump_action(p, action)
    }
    fn occupied(p: &mut Player, revision: u32) {
        p.pump(
            vec![AppEvent::Slots(
                vec![SlotView {
                    slot: 0,
                    label: "Saved scene".into(),
                    exists: true,
                }],
                BTreeMap::from([(0, revision)]),
            )],
            1000,
        );
    }
    #[test]
    fn authored_slots_save_empty_then_confirm_overwrite_with_one_shot_token() {
        let mut p = storage_player();
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert!(
            !packet
                .semantics
                .iter()
                .find(|n| n.label == "Load selected")
                .unwrap()
                .enabled
        );
        let stale = control(&p, "save");
        let commands = save_control(&mut p);
        let job = commands
            .iter()
            .find_map(|c| {
                if let AppCommand::Save {
                    job,
                    expected_revision,
                    ..
                } = c
                {
                    assert_eq!(*expected_revision, 0);
                    Some(*job)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(p.save_confirmation.is_none());
        assert!(!pump_action(&mut p, stale)
            .iter()
            .any(|c| matches!(c, AppCommand::Save { .. })));
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert!(
            !packet
                .semantics
                .iter()
                .find(|n| n.label == "Save selected")
                .unwrap()
                .enabled
        );
        p.pump(
            vec![AppEvent::Saved {
                job,
                slot: 0,
                revision: 1,
            }],
            1000,
        );
        occupied(&mut p, 1);
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert!(packet.texts.iter().any(|t| t.text == "Saved scene"));
        assert!(!save_control(&mut p)
            .iter()
            .any(|c| matches!(c, AppCommand::Save { .. })));
        let token = p.save_confirmation.as_ref().unwrap().token;
        let packet = nir_presentation::project(
            &p.model(),
            400.,
            720.,
            &nir_presentation::Messages::default(),
        );
        assert_eq!(packet.semantics.len(), 2);
        assert!(packet.semantics.iter().all(|n| matches!(
            n.action,
            UiAction::ConfirmSave { .. } | UiAction::CancelSave { .. }
        )));
        let blocked = control(&p, "load");
        assert!(!pump_action(&mut p, blocked)
            .iter()
            .any(|c| matches!(c, AppCommand::Load { .. })));
        let commands = pump_action(&mut p, UiAction::ConfirmSave { token });
        assert!(commands.iter().any(|c| matches!(
            c,
            AppCommand::Save {
                expected_revision: 1,
                ..
            }
        )));
        assert!(!pump_action(&mut p, UiAction::ConfirmSave { token })
            .iter()
            .any(|c| matches!(c, AppCommand::Save { .. })));
    }
    #[test]
    fn save_confirmation_expires_on_revision_back_and_load() {
        let mut p = storage_player();
        occupied(&mut p, 1);
        save_control(&mut p);
        let token = p.save_confirmation.as_ref().unwrap().token;
        occupied(&mut p, 2);
        assert!(p.save_confirmation.is_none());
        assert!(!pump_action(&mut p, UiAction::ConfirmSave { token })
            .iter()
            .any(|c| matches!(c, AppCommand::Save { .. })));
        occupied(&mut p, 1);
        assert_eq!(p.slot_revisions[&0], 2);
        save_control(&mut p);
        let token = p.save_confirmation.as_ref().unwrap().token;
        pump_action(&mut p, UiAction::Close);
        assert_eq!(p.screen, Screen::Menu);
        assert!(p.save_confirmation.is_none());
        assert!(!pump_action(&mut p, UiAction::ConfirmSave { token })
            .iter()
            .any(|c| matches!(c, AppCommand::Save { .. })));
        save_control(&mut p);
        let token = p.save_confirmation.as_ref().unwrap().token;
        let commands = pump_action(&mut p, UiAction::Load { slot: 0 });
        assert!(commands
            .iter()
            .any(|c| matches!(c, AppCommand::Load { slot: 0, .. })));
        assert!(p.save_confirmation.is_none());
        assert!(!pump_action(&mut p, UiAction::ConfirmSave { token })
            .iter()
            .any(|c| matches!(c, AppCommand::Save { .. })));
    }
    #[test]
    fn storage_contract_requires_capability_and_bounded_slot_selector() {
        for kind in 0..4 {
            let mut p = storage_program();
            match kind {
                0 => p.requires.retain(|c| c != "ui.menu-storage.v1"),
                1 => {
                    p.theme.image_menus.get_mut("system").unwrap().elements[0].text_slot =
                        Some(MenuSlot::Fixed { slot: 3 })
                }
                2 => {
                    p.theme
                        .image_menus
                        .get_mut("system")
                        .unwrap()
                        .locals
                        .insert(
                            "selected".into(),
                            MenuLocal::Int {
                                initial: 0,
                                min: 0,
                                max: 3,
                            },
                        );
                }
                _ => {
                    p.theme.image_menus.get_mut("system").unwrap().elements[0].text_preference =
                        Some(MenuPreference::TextSpeed)
                }
            }
            assert!(Player::new(p, "release".into(), "Test".into()).is_err());
        }
    }
    #[test]
    fn confirmation_rechecks_control_permission_and_revision_exhaustion_is_reported() {
        let mut program = storage_program();
        program
            .theme
            .image_menus
            .get_mut("system")
            .unwrap()
            .elements[1]
            .enabled_when
            .push(MenuCondition::Profile {
                key: "locked".into(),
                present: false,
            });
        let mut p = Player::new(program, "release".into(), "Test".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        occupied(&mut p, 1);
        save_control(&mut p);
        let token = p.save_confirmation.as_ref().unwrap().token;
        p.pump(
            vec![AppEvent::Profile(BTreeSet::from(["locked".into()]))],
            1000,
        );
        assert!(p.save_confirmation.is_none());
        assert!(!pump_action(&mut p, UiAction::ConfirmSave { token })
            .iter()
            .any(|c| matches!(c, AppCommand::Save { .. })));
        let mut p = storage_player();
        occupied(&mut p, u32::MAX);
        save_control(&mut p);
        let token = p.save_confirmation.as_ref().unwrap().token;
        let commands = pump_action(&mut p, UiAction::ConfirmSave { token });
        assert!(!commands
            .iter()
            .any(|c| matches!(c, AppCommand::Save { .. })));
        assert_eq!(p.diagnostic.as_ref().unwrap().code, "E_SAVE_LIMIT");
    }
    fn history_program() -> Program {
        let mut p = service_program();
        p.requires.push("ui.menu-history.v1".into());
        p.theme.image_menus.insert("system".into(),serde_json::from_value(serde_json::json!({
            "background":"menu.only","buttons":[],"locals":{"offset":{"type":"int","initial":0,"min":0,"max":999}},
            "elements":[
                {"id":"records","rect":[20,20,600,500],"content":{"type":"history_window","offset_local":"offset","limit":16,"row_height":30,"size":16,"color":[1,1,1,1]}},
                {"id":"older","rect":[20,600,200,60],"content":{"type":"hit_region","label":"Older","action":{"type":"history_page","window":"records","delta":16}}},
                {"id":"newer","rect":[240,600,200,60],"content":{"type":"hit_region","label":"Newer","action":{"type":"history_page","window":"records","delta":-16}}}
            ]
        })).unwrap());
        p
    }
    fn history_flow_program() -> Program {
        let mut p = service_program();
        p.requires.push("ui.menu-history-flow.v1".into());
        p.theme
            .image_menus
            .get_mut("system")
            .unwrap()
            .locals
            .insert("shown".into(), MenuLocal::Bool { initial: true });
        p.theme
            .image_menus
            .get_mut("system")
            .unwrap()
            .locals
            .insert("enabled".into(), MenuLocal::Bool { initial: true });
        p.theme.image_menus.get_mut("system").unwrap().elements = vec![serde_json::from_value(serde_json::json!({
            "id":"records", "rect":[20,20,600,500],
            "visible_when":[{"type":"local","name":"shown","equals":true}],
            "enabled_when":[{"type":"local","name":"enabled","equals":true}],
            "content":{"type":"history_flow","size":16,"line_height":24,"gap":12,"wheel_step":48,"page_step":200,"max_visible":32,"color":[1,1,1,1]}
        })).unwrap()];
        p
    }
    fn history_scrollbar_program() -> Program {
        let mut p = history_flow_program();
        p.requires.push("ui.menu-history-scrollbar.v1".into());
        for name in [
            "bar.track",
            "bar.thumb",
            "bar.hover",
            "bar.pressed",
            "bar.disabled",
        ] {
            p.assets.insert(name.into(), p.assets["menu.only"].clone());
        }
        let states = serde_json::json!({"asset":"bar.thumb","hover_asset":"bar.hover","pressed_asset":"bar.pressed","disabled_asset":"bar.disabled"});
        p.theme.image_menus.get_mut("system").unwrap().elements.push(serde_json::from_value(serde_json::json!({
            "id":"scroll", "rect":[650,20,32,500],
            "content":{"type":"history_scrollbar","window":"records","label":"History scroll","thumb_height":24,"arrow_height":16,"line_step":24,
                "track":{"asset":"bar.track"},"thumb":states,"decrease":states,"increase":states}
        })).unwrap());
        p
    }
    fn open_scrollbar_history() -> Player {
        let mut p = Player::new(history_scrollbar_program(), "r".into(), "Test".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        let mut snapshot = p.core.snapshot();
        snapshot.history = vec![snapshot.history[0].clone(); 1000];
        p.restore(snapshot).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        p
    }
    #[test]
    fn authored_navigation_policy_keeps_edge_scrollbar_controls_reachable() {
        let p = open_scrollbar_history();
        let before = serde_json::to_value(p.core.snapshot()).unwrap();
        let mut model = p.model();
        let legacy =
            nir_presentation::project(&model, 1280., 720., &nir_presentation::Messages::default());
        assert!(matches!(legacy.hit(1196., 20.), Some(UiAction::Close)));
        let menu = model.theme.image_menus.get_mut("system").unwrap();
        menu.builtin_navigation = false;
        menu.elements
            .iter_mut()
            .find(|e| e.id == "scroll")
            .unwrap()
            .rect = [1180., 12., 32., 500.];
        let mut reading = nir_presentation::ReadingState::default();
        let mut text = nir_presentation::TextEngine::default();
        text.add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
        let messages = nir_presentation::Messages::default();
        let identity = (p.generation.session, p.current_interaction());
        let mut packet = reading.project(&model, identity, 1280., 720., &messages, &mut text);
        for _ in 0..100 {
            if !reading.history_pending() {
                break;
            }
            packet = reading.project(&model, identity, 1280., 720., &messages, &mut text);
        }
        assert!(!packet.semantics.iter().any(|n| n.action == UiAction::Close));
        let bar = packet.history_bar.as_ref().unwrap();
        let (x, y) = (bar.decrease[0] + 8., bar.decrease[1] + 8.);
        let expected = bar.offset - bar.line_step;
        assert!(matches!(
            packet.hit(x, y),
            Some(UiAction::MenuHistoryScroll { .. })
        ));
        assert!(reading.history_bar_gesture(0, x, y, 0, identity, &packet));
        packet = reading.project(&model, identity, 1280., 720., &messages, &mut text);
        assert_eq!(packet.history_bar.unwrap().offset, expected);
        assert_eq!(serde_json::to_value(p.core.snapshot()).unwrap(), before);
    }
    #[test]
    fn image_history_scrollbar_drag_states_and_input_authority() {
        use nir_presentation::{HistoryBarPart, ReadingState, SemanticValue, TextEngine};
        let p = open_scrollbar_history();
        let before = serde_json::to_value(p.core.snapshot()).unwrap();
        assert!([
            "bar.track",
            "bar.thumb",
            "bar.hover",
            "bar.pressed",
            "bar.disabled"
        ]
        .iter()
        .all(|id| p.retained_assets().contains(*id)));
        let mut reading = ReadingState::default();
        let mut text = TextEngine::default();
        text.add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
        let messages = nir_presentation::Messages::default();
        let identity = (p.generation.session, p.current_interaction());
        let mut packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert!(!packet.history_bar.as_ref().unwrap().enabled);
        for _ in 0..100 {
            packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
            if !reading.history_pending() {
                break;
            }
        }
        let newest = packet.history_bar.clone().unwrap();
        assert!(newest.enabled);
        assert_eq!(newest.offset, newest.max);
        assert_eq!(newest.part_at(666., 28.), Some(HistoryBarPart::Decrease));
        let states = |packet: &nir_presentation::DrawPacket| {
            packet
                .quads
                .iter()
                .filter_map(|q| q.asset.as_deref())
                .filter(|id| id.starts_with("bar."))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            states(&packet),
            ["bar.track", "bar.thumb", "bar.thumb", "bar.disabled"]
        );
        let stale = newest
            .action(HistoryScrollInput::Position { ratio: 0. })
            .unwrap();
        reading.hover_history_bar(666., newest.thumb[1] + 6.);
        packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert_eq!(states(&packet)[1], "bar.hover");
        assert!(reading.history_bar_gesture(0, 666., newest.thumb[1] + 6., 0, identity, &packet));
        packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert_eq!(packet.history_bar.as_ref().unwrap().offset, newest.offset);
        assert_eq!(states(&packet)[1], "bar.pressed");
        // Preserve the off-center grab: moving to the track midpoint with the
        // same -6px offset positions the thumb exactly halfway through travel.
        let center = newest.track[1] + newest.track[3] / 2.;
        assert!(reading.history_bar_gesture(1, 666., center - 6., 0, identity, &packet));
        packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert!((packet.history_bar.as_ref().unwrap().offset - newest.max / 2.).abs() < 0.01);
        assert_eq!(states(&packet)[1], "bar.pressed");
        assert!(!reading.scroll_menu_history(&stale, &packet));
        let mut wrong = packet
            .history_bar
            .as_ref()
            .unwrap()
            .action(HistoryScrollInput::Position { ratio: 0. })
            .unwrap();
        if let UiAction::MenuHistoryScroll { control, .. } = &mut wrong {
            *control = Some("unknown".into());
        }
        assert!(!reading.scroll_menu_history(&wrong, &packet));
        let id = packet
            .semantics
            .iter()
            .find(|n| matches!(n.value, Some(SemanticValue::Scrollbar { .. })))
            .unwrap()
            .id;
        let node = packet.semantics.iter().find(|n| n.id == id).unwrap();
        let up = nir_presentation::value_action(node, 4).unwrap();
        let down = nir_presentation::value_action(node, 5).unwrap();
        if let (
            UiAction::MenuHistoryScroll {
                input: HistoryScrollInput::Position { ratio: a },
                ..
            },
            UiAction::MenuHistoryScroll {
                input: HistoryScrollInput::Position { ratio: b },
                ..
            },
        ) = (&up, &down)
        {
            assert!(a < &0.5 && b > &0.5);
        } else {
            panic!("vertical keyboard action");
        }
        assert!(node.action.same_focus_target(&up));
        assert!(reading.history_bar_gesture(2, 666., -100., 0, identity, &packet));
        packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert_eq!(packet.history_bar.as_ref().unwrap().offset, 0.);
        assert_eq!(states(&packet)[2], "bar.disabled");
        let bar = packet.history_bar.clone().unwrap();
        assert!(!reading.history_bar_gesture(0, 666., 28., 0, identity, &packet));
        assert!(reading.history_bar_gesture(0, 666., bar.increase[1] + 8., 0, identity, &packet));
        packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert_eq!(packet.history_bar.as_ref().unwrap().offset, 24.);
        assert_eq!(states(&packet)[3], "bar.pressed");
        assert!(reading.history_bar_gesture(2, 666., bar.increase[1] + 8., 0, identity, &packet));
        packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert_eq!(states(&packet)[3], "bar.hover");
        assert!(reading.history_bar_gesture(0, 666., 300., 0, identity, &packet));
        packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert_eq!(packet.history_bar.as_ref().unwrap().offset, 224.);
        assert!(reading.history_bar_gesture(3, 0., 0., 0, identity, &packet));
        assert!(!reading.history_bar_gesture(1, 666., 400., 0, identity, &packet));
        assert_eq!(serde_json::to_value(p.core.snapshot()).unwrap(), before);
    }
    #[test]
    fn history_scrollbar_obeys_clip_paint_order_and_cancels_stale_capture() {
        use nir_presentation::{MenuPaint, ReadingState, TextEngine};
        let mut p = open_scrollbar_history();
        let mut reading = ReadingState::default();
        let mut text = TextEngine::default();
        text.add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
        let messages = nir_presentation::Messages::default();
        let identity = (p.generation.session, p.current_interaction());
        let mut packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        for _ in 0..100 {
            packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
            if !reading.history_pending() {
                break;
            }
        }
        assert!(matches!(packet.menu_paint.last(), Some(MenuPaint::Quad(_))));
        let mut model = p.model();
        let elements = &mut model.theme.image_menus.get_mut("system").unwrap().elements;
        elements.swap(0, 1);
        packet = reading.project(&model, identity, 1280., 720., &messages, &mut text);
        assert!(matches!(packet.menu_paint.last(), Some(MenuPaint::Text(_))));
        elements_clear_overlay(&mut model);
        // A later disabled hit region is still a hit barrier.
        model.theme.image_menus.get_mut("system").unwrap().elements.push(serde_json::from_value(serde_json::json!({
            "id":"overlay","rect":[640,0,100,540],"enabled_when":[{"type":"local","name":"enabled","equals":false}],
            "content":{"type":"hit_region","label":"Overlay","action":{"type":"close"}}
        })).unwrap());
        packet = reading.project(&model, identity, 1280., 720., &messages, &mut text);
        assert!(!reading.history_bar_gesture(0, 666., 28., 0, identity, &packet));
        elements_clear_overlay(&mut model);
        let bar = model
            .theme
            .image_menus
            .get_mut("system")
            .unwrap()
            .elements
            .iter_mut()
            .find(|e| e.id == "scroll")
            .unwrap();
        bar.clip = Some([0., 200., 32., 100.]);
        packet = reading.project(&model, identity, 1280., 720., &messages, &mut text);
        assert_eq!(
            packet.history_bar.as_ref().unwrap().clip,
            [650., 220., 32., 100.]
        );
        assert!(!reading.history_bar_gesture(0, 666., 28., 0, identity, &packet));
        assert!(reading.history_bar_gesture(0, 666., 250., 0, identity, &packet));
        // Any reflow or geometry change cancels capture before a later move.
        let old = packet
            .history_bar
            .as_ref()
            .unwrap()
            .action(HistoryScrollInput::Position { ratio: 0. })
            .unwrap();
        packet = reading.project(&model, identity, 640., 360., &messages, &mut text);
        assert!(!reading.history_bar_gesture(1, 333., 150., 0, identity, &packet));
        assert!(!reading.scroll_menu_history(&old, &packet));
        let bar = model
            .theme
            .image_menus
            .get_mut("system")
            .unwrap()
            .elements
            .iter_mut()
            .find(|e| e.id == "scroll")
            .unwrap();
        bar.clip = None;
        packet = reading.project(&model, identity, 1280., 720., &messages, &mut text);
        for _ in 0..100 {
            packet = reading.project(&model, identity, 1280., 720., &messages, &mut text);
            if !reading.history_pending() {
                break;
            }
        }
        let bar = packet.history_bar.clone().unwrap();
        assert!(reading.history_bar_gesture(0, 666., bar.thumb[1] + 12., 0, identity, &packet));
        assert!(reading.history_bar_gesture(
            1,
            666.,
            200.,
            0,
            (identity.0 + 1, identity.1),
            &packet
        ));
        assert_eq!(packet.history_bar.as_ref().unwrap().offset, bar.offset);
        let old = bar
            .action(HistoryScrollInput::Position { ratio: 0. })
            .unwrap();
        p.menu_session
            .locals
            .insert("enabled".into(), MenuValue::Bool(false));
        let disabled = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert!(!disabled.history_bar.as_ref().unwrap().enabled);
        assert!(!reading.scroll_menu_history(&old, &disabled));
        pump_action(&mut p, UiAction::Close);
        let closed = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert!(closed.history_bar.is_none());
        assert!(!reading.scroll_menu_history(&old, &closed));
        fn elements_clear_overlay(model: &mut nir_presentation::UiModel) {
            model
                .theme
                .image_menus
                .get_mut("system")
                .unwrap()
                .elements
                .retain(|e| e.id != "overlay");
        }
    }
    #[test]
    fn history_scrollbar_requires_capability_valid_target_geometry_and_all_state_assets() {
        for case in 0..9 {
            let mut p = history_scrollbar_program();
            let menu = p.theme.image_menus.get_mut("system").unwrap();
            match case {
                0 => p.requires.retain(|c| c != "ui.menu-history-scrollbar.v1"),
                1 => {
                    let mut other = menu.elements[1].clone();
                    other.id = "other".into();
                    menu.elements.push(other);
                }
                2 => {
                    if let MenuContent::HistoryScrollbar { window, .. } =
                        &mut menu.elements[1].content
                    {
                        *window = "missing".into();
                    }
                }
                3 => {
                    if let MenuContent::HistoryScrollbar { thumb_height, .. } =
                        &mut menu.elements[1].content
                    {
                        *thumb_height = 500.;
                    }
                }
                4 => {
                    if let MenuContent::HistoryScrollbar { arrow_height, .. } =
                        &mut menu.elements[1].content
                    {
                        *arrow_height = f32::NAN;
                    }
                }
                5 => {
                    if let MenuContent::HistoryScrollbar { line_step, .. } =
                        &mut menu.elements[1].content
                    {
                        *line_step = 0.;
                    }
                }
                6 => {
                    if let MenuContent::HistoryScrollbar { thumb, .. } =
                        &mut menu.elements[1].content
                    {
                        thumb.hover_asset = Some("unknown".into());
                    }
                }
                7 => {
                    if let MenuContent::HistoryScrollbar { label, .. } =
                        &mut menu.elements[1].content
                    {
                        label.clear();
                    }
                }
                _ => {
                    let audio = p
                        .assets
                        .values()
                        .find(|a| a.kind == AssetKind::Audio)
                        .unwrap()
                        .clone();
                    p.assets.insert("bar.pressed".into(), audio);
                }
            }
            assert!(
                Player::new(p, "r".into(), "Test".into()).is_err(),
                "case {case}"
            );
        }
    }
    #[test]
    fn continuous_history_shares_one_page_snapshot_and_guards_scroll_versions() {
        let mut p = Player::new(history_flow_program(), "r".into(), "Test".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        let mut snapshot = p.core.snapshot();
        snapshot.history = vec![snapshot.history[0].clone(); 1000];
        p.restore(snapshot).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        assert!(p.model().menu_history_flow.is_none());
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        let before = serde_json::to_value(p.core.snapshot()).unwrap();
        let rows = p.model().menu_history_flow.unwrap();
        assert_eq!(rows.len(), 1000);
        assert!(Arc::ptr_eq(&rows, &p.model().menu_history_flow.unwrap()));
        let mut reading = nir_presentation::ReadingState::default();
        let mut text = nir_presentation::TextEngine::default();
        text.add_font_asset(
            "font.reader",
            include_bytes!("../../../examples/rain-letters/assets/source/reader.otf").to_vec(),
        )
        .unwrap();
        let messages = nir_presentation::Messages::default();
        let identity = (p.generation.session, p.current_interaction());
        let mut packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        {
            let mut clipped = p.model();
            clipped
                .theme
                .image_menus
                .get_mut("system")
                .unwrap()
                .elements[0]
                .clip = Some([0., 50., 300., 200.]);
            let mut pending = nir_presentation::ReadingState::default();
            let packet = pending.project(&clipped, identity, 1280., 720., &messages, &mut text);
            assert!(pending.history_pending());
            assert_eq!(
                packet.texts.last().unwrap().clip,
                Some([20., 70., 300., 200.])
            );
            clipped
                .theme
                .image_menus
                .get_mut("system")
                .unwrap()
                .elements[0]
                .clip = Some([0., 0., 0., 0.]);
            let packet = pending.project(&clipped, identity, 1280., 720., &messages, &mut text);
            assert!(!pending.history_pending());
            assert!(packet.scrolls.is_empty());
        }
        assert!(reading.history_pending());
        assert!(packet.scrolls.is_empty());
        for _ in 0..100 {
            if !reading.history_pending() {
                break;
            }
            packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        }
        assert!(!reading.history_pending());
        assert!(reading.history_error().is_none());
        assert_eq!(packet.scrolls.len(), 1);
        assert!(packet.texts.len() <= 32);
        let newest = packet.scrolls[0].offset;
        assert_eq!(newest, packet.scrolls[0].max);
        let stale = packet.scrolls[0].action(-1, false);
        assert!(reading.scroll_menu_history(&stale, &packet));
        assert!(!reading.scroll_menu_history(&stale, &packet));
        packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert_eq!(packet.scrolls[0].offset, newest - 48.);
        let current = packet.scrolls[0].action(-1, true);
        let mut invalid = current.clone();
        if let UiAction::MenuHistoryScroll { input, .. } = &mut invalid {
            *input = HistoryScrollInput::Step { delta: i32::MIN };
        }
        assert!(!reading.scroll_menu_history(&invalid, &packet));
        assert!(reading.scroll_menu_history(&current, &packet));
        // Menu revision changes invalidate authority even before a redraw consumes input.
        packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        let stale_pref = packet.scrolls[0].action(-1, false);
        let c = pump_action(&mut p, UiAction::FontSize { delta: 0.25 });
        settle(&mut p, c);
        for _ in 0..100 {
            packet = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
            if !reading.history_pending() {
                break;
            }
        }
        assert!(!reading.scroll_menu_history(&stale_pref, &packet));
        assert_eq!(serde_json::to_value(p.core.snapshot()).unwrap(), before);
        let stale_visible = packet.scrolls[0].action(-1, false);
        p.menu_session
            .locals
            .insert("shown".into(), MenuValue::Bool(false));
        let hidden = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert!(hidden.scrolls.is_empty());
        assert!(!reading.scroll_menu_history(&stale_visible, &hidden));
        p.menu_session
            .locals
            .insert("shown".into(), MenuValue::Bool(true));
        p.menu_session
            .locals
            .insert("enabled".into(), MenuValue::Bool(false));
        let disabled = reading.project(&p.model(), identity, 1280., 720., &messages, &mut text);
        assert!(disabled.scrolls.is_empty());
        assert!(!reading.scroll_menu_history(&stale_visible, &disabled));
        let old_instance = p.menu_session.instance;
        let weak = Arc::downgrade(&rows);
        drop(rows);
        drop(reading);
        pump_action(&mut p, UiAction::Close);
        assert!(p.model().menu_history_flow.is_none());
        assert!(weak.upgrade().is_none());
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        assert_ne!(p.menu_session.instance, old_instance);
        assert_eq!(p.model().menu_history_flow.unwrap().len(), 1000);
    }
    #[test]
    fn continuous_history_requires_capability_and_bounded_density() {
        for case in 0..6 {
            let mut p = history_flow_program();
            let menu = p.theme.image_menus.get_mut("system").unwrap();
            match case {
                0 => p.requires.retain(|c| c != "ui.menu-history-flow.v1"),
                1 => {
                    let mut second = menu.elements[0].clone();
                    second.id = "second".into();
                    menu.elements.push(second);
                }
                2 => {
                    if let MenuContent::HistoryFlow { max_visible, .. } =
                        &mut menu.elements[0].content
                    {
                        *max_visible = u32::MAX;
                    }
                }
                3 => {
                    if let MenuContent::HistoryFlow { line_height, .. } =
                        &mut menu.elements[0].content
                    {
                        *line_height = f32::NAN;
                    }
                }
                4 => menu.elements[0].rect[3] = 8192.,
                _ => {
                    if let MenuContent::HistoryFlow { wheel_step, .. } =
                        &mut menu.elements[0].content
                    {
                        *wheel_step = 0.;
                    }
                }
            }
            assert!(
                Player::new(p, "r".into(), "Test".into()).is_err(),
                "case {case}"
            );
        }
    }
    #[test]
    fn thousand_history_rows_only_project_a_bounded_stable_window() {
        let mut p = Player::new(history_program(), "release".into(), "Test".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        let mut snapshot = p.core.snapshot();
        snapshot.history = vec![snapshot.history[0].clone(); 1000];
        p.restore(snapshot).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        assert!(p.model().history.is_empty());
        assert!(p.model().menu_history.is_empty());
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        let before = serde_json::to_value(p.core.snapshot()).unwrap();
        let assets = p.retained_assets();
        let model = p.model();
        let rows = &model.menu_history["records"];
        assert_eq!(rows.len(), 16);
        assert_eq!(rows[0].key, 984);
        assert_eq!(rows[15].key, 999);
        assert_eq!(model.history_total, 1000);
        assert!(model.history.is_empty());
        let packet =
            nir_presentation::project(&model, 1280., 720., &nir_presentation::Messages::default());
        assert!(packet.texts.len() <= 20);
        assert!(
            !packet
                .semantics
                .iter()
                .find(|n| n.label == "Newer")
                .unwrap()
                .enabled
        );
        let old = control(&p, "older");
        pump_action(&mut p, old.clone());
        let model = p.model();
        assert_eq!(model.menu_history["records"][0].key, 968);
        pump_action(&mut p, old);
        assert_eq!(p.menu_session.locals["offset"], MenuValue::Int(16));
        let newer = control(&p, "newer");
        pump_action(&mut p, newer);
        assert_eq!(p.model().menu_history["records"][0].key, 984);
        assert_eq!(serde_json::to_value(p.core.snapshot()).unwrap(), before);
        assert_eq!(p.retained_assets(), assets);
        pump_action(&mut p, UiAction::History);
        assert_eq!(p.model().history.len(), 3);
        assert!(p.model().menu_history.is_empty());
        pump_action(&mut p, UiAction::HistoryPage { delta: 999 });
        assert_eq!(p.model().history.len(), 1);
    }
    #[test]
    fn history_window_validation_rejects_unbounded_templates_and_bad_paging() {
        for kind in 0..5 {
            let mut p = history_program();
            let menu = p.theme.image_menus.get_mut("system").unwrap();
            match kind {
                0 => p.requires.retain(|c| c != "ui.menu-history.v1"),
                1 => {
                    if let MenuContent::HistoryWindow { limit, .. } = &mut menu.elements[0].content
                    {
                        *limit = u32::MAX;
                    }
                }
                2 => {
                    if let MenuContent::HistoryWindow { offset_local, .. } =
                        &mut menu.elements[0].content
                    {
                        *offset_local = "missing".into();
                    }
                }
                3 => {
                    let mut e = menu.elements[0].clone();
                    for n in 0..4 {
                        e.id = format!("extra{n}");
                        menu.elements.push(e.clone());
                    }
                }
                _ => {
                    if let MenuContent::HitRegion { action, .. } = &mut menu.elements[1].content {
                        *action = ImageMenuAction::HistoryPage {
                            window: "missing".into(),
                            delta: 1,
                        };
                    }
                }
            }
            assert!(Player::new(p, "release".into(), "Test".into()).is_err());
        }
    }

    // ---- ui.menu-effects.v1: page effect transactions ------------------
    fn ui_audio(commands: &[AppCommand]) -> Vec<(u32, &str, AudioBus, bool, f32)> {
        commands
            .iter()
            .filter_map(|c| match c {
                AppCommand::AudioStart {
                    domain: TimeDomain::ForegroundUi,
                    task,
                    asset,
                    bus,
                    looped,
                    gain,
                    ..
                } => Some((*task, asset.as_str(), *bus, *looped, *gain)),
                _ => None,
            })
            .collect()
    }
    fn ui_stops(commands: &[AppCommand]) -> Vec<u32> {
        commands
            .iter()
            .filter_map(|c| match c {
                AppCommand::AudioStop {
                    domain: TimeDomain::ForegroundUi,
                    task,
                    ..
                } => Some(*task),
                _ => None,
            })
            .collect()
    }
    fn effects_program() -> Program {
        let mut p = service_program();
        p.requires.push("ui.menu-effects.v1".into());
        for (id, asset) in [("system", "audio.bgm"), ("system.child", "audio.voice")] {
            let menu = p.theme.image_menus.get_mut(id).unwrap();
            menu.effects = Some(
                serde_json::from_value(serde_json::json!({
                    "enter": {"sound": "audio.bell", "fade_us": "400000"},
                    "close": {"sound": "audio.bell", "fade_us": "300000"},
                    "click": "audio.bell",
                    "music": {"asset": asset, "bus": "voice", "gain": 0.5}
                }))
                .unwrap(),
            );
        }
        p.theme.image_menus.get_mut("other").unwrap().effects = Some(
            serde_json::from_value(serde_json::json!({
                "enter": {"sound": "audio.bell"},
                "music": {"asset": "audio.voice"}
            }))
            .unwrap(),
        );
        p.theme.image_menus.get_mut("title").unwrap().effects = Some(
            serde_json::from_value(serde_json::json!({
                "enter": {"sound": "audio.bell", "fade_us": "400000"},
                "music": {"asset": "audio.bgm"}
            }))
            .unwrap(),
        );
        p
    }
    #[test]
    fn effects_require_their_capability() {
        let mut p = effects_program();
        p.requires.retain(|c| c != "ui.menu-effects.v1");
        assert_eq!(
            Player::new(p, "r".into(), "t".into()).err().unwrap().code,
            "E_CAPABILITY"
        );
    }
    #[test]
    fn enter_effects_wait_for_preparation_fire_once_and_music_follows_pages() {
        let mut p = Player::new(effects_program(), "r".into(), "t".into()).unwrap();
        // The Preparing gap is silent: boot preparation has not completed.
        let commands = p.pump(vec![], 1000);
        assert!(ui_audio(&commands).is_empty());
        let fired = settle(&mut p, commands);
        let starts = ui_audio(&fired);
        assert_eq!(starts.len(), 2, "one enter sound plus one music voice");
        let (_, _, bus, looped, gain) = starts
            .iter()
            .find(|(_, asset, ..)| *asset == "audio.bgm")
            .unwrap();
        assert_eq!((*bus, *looped, *gain), (AudioBus::Bgm, true, 1.));
        let (_, _, bus, looped, _) = starts
            .iter()
            .find(|(_, asset, ..)| *asset == "audio.bell")
            .unwrap();
        assert_eq!((*bus, *looped), (AudioBus::Sfx, false));
        let instance = p.menu_session.instance;
        assert_eq!(
            p.menu_effects.entered,
            Some(("title".into(), instance)),
            "ownership recorded"
        );
        // Ownership is stable: no re-fire, no stop, until the page changes.
        let again = p.pump(vec![], 1000);
        assert!(ui_audio(&again).is_empty());
        assert!(ui_stops(&again).is_empty());
        // The enter fade follows the foreground clock and releases its lease.
        assert_eq!(p.model().menu_opacity, 0.);
        assert!(p.menu_effects_clock.is_some());
        p.pump(
            vec![AppEvent::TickDomains {
                story_us: 0,
                foreground_us: 200_000,
            }],
            1000,
        );
        assert!((p.model().menu_opacity - 0.5).abs() < 1e-3);
        p.pump(
            vec![AppEvent::TickDomains {
                story_us: 0,
                foreground_us: 200_000,
            }],
            1000,
        );
        assert_eq!(p.model().menu_opacity, 1.);
        assert!(p.menu_effects.fade.is_none());
        assert!(p.menu_effects_clock.is_none());
        // A new title page stops the old music and plays its own enter.
        let title_music = p.menu_effects.music.unwrap().task;
        let navigate = control(&p, "other");
        let commands = pump_action(&mut p, navigate);
        assert!(ui_stops(&commands).contains(&title_music));
        let starts = ui_audio(&commands);
        assert_eq!(starts.len(), 2);
        assert!(starts.iter().any(|(_, asset, ..)| *asset == "audio.voice"));
        assert_eq!(
            p.menu_effects.entered,
            Some(("other".into(), instance + 1)),
            "a new page instance owns new voices"
        );
    }
    #[test]
    fn failed_boot_preparation_keeps_the_title_silent() {
        let mut p = Player::new(effects_program(), "r".into(), "t".into()).unwrap();
        let commands = p.pump(vec![], 1000);
        let request = commands
            .iter()
            .find_map(|c| {
                if let AppCommand::GetAssets { request, .. } = c {
                    Some(*request)
                } else {
                    None
                }
            })
            .unwrap();
        p.pump(
            vec![AppEvent::AssetFailed {
                request,
                message: "fixture boot failure".into(),
            }],
            1000,
        );
        let commands = p.pump(vec![], 1000);
        assert!(p.menu_effects.entered.is_none());
        assert!(ui_audio(&commands).is_empty());
    }
    /// Returns the player with the overlay open and prepared, plus the task
    /// ids of its enter sound and looping page music.
    fn effects_player_at_overlay() -> (Player, u32, u32) {
        let mut p = Player::new(effects_program(), "r".into(), "t".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::Menu);
        // The overlay Preparing gap is silent.
        assert!(ui_audio(&c).is_empty());
        let fired = settle(&mut p, c);
        let bell = ui_audio(&fired)
            .iter()
            .find(|(_, asset, ..)| *asset == "audio.bell")
            .unwrap()
            .0;
        let music = p.menu_effects.music.unwrap().task;
        (p, bell, music)
    }
    #[test]
    fn close_plays_its_sound_locks_input_and_defers_the_exit() {
        let (mut p, _, music) = effects_player_at_overlay();
        let commands = pump_action(&mut p, UiAction::Close);
        // Acceptance feedback first; the page itself is still up.
        let starts = ui_audio(&commands);
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].1, "audio.bell");
        assert_eq!(p.screen, Screen::Menu);
        assert!(p.menu_effects.closing.is_some());
        assert!(p.paused());
        // Old input is locked while the close fade runs: the repeat Close is
        // swallowed (beyond telemetry) and changes nothing.
        let locked = pump_action(&mut p, UiAction::Close);
        assert!(locked.iter().all(|c| matches!(c, AppCommand::Observation { .. })));
        assert_eq!(p.screen, Screen::Menu);
        assert!(p.menu_effects.closing.is_some());
        assert_eq!(p.screen, Screen::Menu);
        p.pump(
            vec![AppEvent::TickDomains {
                story_us: 0,
                foreground_us: 150_000,
            }],
            1000,
        );
        assert_eq!(p.screen, Screen::Menu);
        assert!((p.model().menu_opacity - 0.5).abs() < 1e-3);
        // Projection agrees: the authored page itself is half faded, not just
        // the model scalar.
        let packet = nir_presentation::project(
            &p.model(),
            1280.,
            720.,
            &nir_presentation::Messages::default(),
        );
        let quad = packet
            .quads
            .iter()
            .find(|q| q.asset.as_deref() == Some("menu.only"))
            .unwrap();
        assert!((quad.color[3] - 0.5).abs() < 1e-3);
        // The deferred exit commits exactly when the fade completes, and the
        // looping page music stops with the page.
        let commands = p.pump(
            vec![AppEvent::TickDomains {
                story_us: 0,
                foreground_us: 150_000,
            }],
            1000,
        );
        assert_eq!(p.screen, Screen::Story);
        assert!(!p.paused());
        assert!(p.menu_effects.closing.is_none());
        assert!(p.menu_effects_clock.is_none());
        assert!(ui_stops(&commands).contains(&music));
        assert_eq!(p.model().menu_opacity, 1., "story is never faded");
    }
    #[test]
    fn click_sounds_only_on_accepted_control_commits() {
        let (mut p, _, _) = effects_player_at_overlay();
        let revision = p.menu_session.revision;
        let accepted = control(&p, "increase");
        let commands = pump_action(&mut p, accepted);
        assert!(ui_audio(&commands)
            .iter()
            .any(|(_, asset, ..)| *asset == "audio.bell"));
        let sounds = p.menu_effects.sounds.clone();
        // The commit and the preference it changed each advance identity, so
        // the captured revision is now stale in either case.
        let current = p.menu_session.revision;
        assert!(current > revision);
        let stale = UiAction::MenuControl {
            instance: p.menu_session.instance,
            revision,
            control: "increase".into(),
        };
        let commands = pump_action(&mut p, stale);
        assert!(ui_audio(&commands).is_empty());
        assert_eq!(p.menu_session.revision, current);
        assert_eq!(p.menu_effects.sounds, sounds);
    }
    #[test]
    fn reduced_motion_suppresses_fades_but_not_sounds() {
        let mut p = Player::new(effects_program(), "r".into(), "t".into()).unwrap();
        p.preferences.reduced_motion = true;
        let c = p.pump(vec![], 1000);
        let fired = settle(&mut p, c);
        assert_eq!(ui_audio(&fired).len(), 2, "enter sound and music still play");
        assert!(p.menu_effects.fade.is_none());
        assert!(p.menu_effects_clock.is_none());
        assert_eq!(p.model().menu_opacity, 1.);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::Menu);
        let fired = settle(&mut p, c);
        assert_eq!(ui_audio(&fired).len(), 2);
        assert_eq!(p.model().menu_opacity, 1.);
        // No fade means no deferral: the close sound plays and the exit is
        // immediate, with no clock lease ever taken.
        let commands = pump_action(&mut p, UiAction::Close);
        assert_eq!(ui_audio(&commands).len(), 1);
        assert_eq!(p.screen, Screen::Story);
        assert!(p.menu_effects.closing.is_none());
    }
    #[test]
    fn ui_sound_events_are_accepted_by_task_and_session_and_reset_with_sessions() {
        let (mut p, bell, music) = effects_player_at_overlay();
        assert!(
            !p.menu_effects.sounds.contains_key(&music),
            "looping music is never awaited"
        );
        // A stale-session end event cannot retire a live voice.
        p.pump(
            vec![AppEvent::AudioEnded {
                domain: TimeDomain::ForegroundUi,
                task: bell,
                session: p.generation.session + 1,
            }],
            1000,
        );
        assert!(p.menu_effects.sounds.contains_key(&bell));
        p.pump(
            vec![AppEvent::AudioEnded {
                domain: TimeDomain::ForegroundUi,
                task: bell,
                session: p.generation.session,
            }],
            1000,
        );
        assert!(!p.menu_effects.sounds.contains_key(&bell));
        assert!(p.menu_effects.music.is_some());
        // An unknown failing task is not a fault of this session's effects.
        p.pump(
            vec![AppEvent::AudioFailed {
                domain: TimeDomain::ForegroundUi,
                task: music + 100,
                session: p.generation.session,
                message: "fixture ui audio failure".into(),
            }],
            1000,
        );
        assert!(p.error.is_none());
        // A new session resets the whole voice table; the host reset both
        // audio domains with it, and the rebooted title owns fresh voices.
        let c = pump_action(&mut p, UiAction::Title);
        assert_eq!(p.menu_effects.session, p.generation.session);
        assert!(p.menu_effects.entered.is_none(), "boot is still preparing");
        let fired = settle(&mut p, c);
        assert!(ui_audio(&fired).iter().any(
            |(_, asset, bus, looped, _)| *asset == "audio.bgm"
                && *bus == AudioBus::Bgm
                && *looped
        ));
        assert_eq!(
            p.menu_effects.entered,
            Some(("title".into(), p.menu_session.instance))
        );
        assert_eq!(
            p.menu_effects.music.unwrap().session,
            p.generation.session
        );
    }
    #[test]
    fn reading_close_defers_the_toggle_behind_the_shared_close_fade() {
        let mut program = reading_program();
        program.requires.push("ui.menu-effects.v1".into());
        program
            .theme
            .image_menus
            .get_mut("system")
            .unwrap()
            .effects = Some(
            serde_json::from_value(serde_json::json!({
                "close": {"sound": "audio.bell", "fade_us": "300000"},
                "click": "audio.bell"
            }))
            .unwrap(),
        );
        let mut p = Player::new(program, "r".into(), "t".into()).unwrap();
        let c = p.pump(vec![], 1000);
        settle(&mut p, c);
        let c = pump_action(&mut p, UiAction::NewGame);
        settle(&mut p, c);
        assert!(p.core.dialogue().is_some());
        let c = pump_action(&mut p, UiAction::Menu);
        settle(&mut p, c);
        let interaction = p.current_interaction();
        let resume = control(&p, "auto");
        let commands = pump_action(&mut p, resume);
        // Acceptance click, then the close sound; the toggle itself waits.
        assert_eq!(
            ui_audio(&commands)
                .iter()
                .map(|(_, asset, ..)| *asset)
                .collect::<Vec<_>>(),
            vec!["audio.bell", "audio.bell"]
        );
        assert_eq!(p.screen, Screen::Menu);
        assert!(p.menu_effects.closing.is_some());
        assert!(!p.auto);
        p.pump(
            vec![AppEvent::TickDomains {
                story_us: 0,
                foreground_us: 300_000,
            }],
            1000,
        );
        assert_eq!(p.screen, Screen::Story);
        assert!(p.auto, "the deferred reading action resumed exactly once");
        assert_eq!(p.current_interaction(), interaction);
        assert!(!p.paused());
    }
}
