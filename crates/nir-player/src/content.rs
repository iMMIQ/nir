use super::*;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<ContentKey>,
    pub module: String,
    pub locale: Option<String>,
    pub hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_preferences_select_boot_fonts_before_the_first_request() {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let mut font = program.assets["font.reader"].clone();
        font.object = "b".repeat(64);
        program.assets.insert("font.english".into(), font);
        let objects = program
            .assets
            .iter()
            .map(|(id, a)| (id.clone(), a.object.clone()))
            .collect();
        for plans in [
            &mut program.locale_config.ui,
            &mut program.locale_config.text,
        ] {
            let plan = plans.get_mut("en").unwrap();
            plan.fonts = vec!["font.english".into()];
            plan.digest = LocaleFontPlan::digest_for(&plan.fonts, &objects);
        }
        let preferences = program.player.preferences("en".into(), "en".into());
        let mut player = Player::from_validated(
            ValidatedProgram::new(program).unwrap(),
            "release".into(),
            "Test".into(),
            Some(preferences),
        )
        .unwrap();
        let commands = player.pump(vec![], 100);
        let (assets, descriptors) = commands
            .iter()
            .find_map(|command| match command {
                AppCommand::GetAssets {
                    assets,
                    descriptors,
                    ..
                } => Some((assets, descriptors)),
                _ => None,
            })
            .unwrap();
        assert!(assets.contains(&"font.english".into()));
        assert!(!assets.contains(&"font.reader".into()));
        assert_eq!(
            descriptors.keys().cloned().collect::<BTreeSet<_>>(),
            assets.iter().cloned().collect()
        );
        assert_eq!(player.effective_text_locale, "en");
        assert!(!commands
            .iter()
            .any(|c| matches!(c, AppCommand::PrepareLocale { .. })));
    }

    #[test]
    fn restore_admission_failure_after_content_install_is_reported() {
        let mut program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let snapshot = Core::new(
            ValidatedProgram::new(program.clone()).unwrap(),
            "release".into(),
            program.default_locale.clone(),
        )
        .unwrap()
        .snapshot();
        let bytes = serde_json::to_vec(&ModuleCode {
            format: 1,
            module: "story".into(),
            functions: program.functions.clone(),
        })
        .unwrap();
        program.modules.insert(
            "story".into(),
            ModuleIndex {
                static_content: String::new(),
                functions: program
                    .functions
                    .iter()
                    .map(|(id, f)| (id.clone(), FunctionSignature::from(f)))
                    .collect(),
                texts: program.texts.keys().cloned().collect(),
                code: nir_content::digest(&bytes),
                locales: BTreeMap::new(),
            },
        );
        program.functions.clear();
        let mut player = Player::new(program, "release".into(), "test".into()).unwrap();
        player.restore(snapshot).unwrap();
        let request = *player.content.keys().next().unwrap();
        // Force the media admission that follows successful module validation
        // to fail, independently of network and content parsing.
        player.request = u32::MAX;
        player.complete_content(request, vec![bytes]).unwrap();
        assert!(player.content[&request].failed);
        assert!(player.error.is_some());
        assert!(player.paused());
        assert_eq!(player.diagnostic.as_ref().unwrap().code, "E_MODULE_PREPARE");
    }
}
#[derive(Clone)]
pub(super) enum ContentPurpose {
    Execution,
    Media {
        purpose: Purpose,
        activation: u32,
        assets: BTreeSet<String>,
    },
    Restore(Box<Snapshot>, bool),
    Locale,
}
#[derive(Clone)]
pub(super) struct ContentPreparation {
    pub purpose: ContentPurpose,
    pub objects: Vec<ContentRequest>,
    pub session: u32,
    pub failed: bool,
}
impl Player {
    pub fn accepts_content(&self, request: u32) -> bool {
        self.content
            .get(&request)
            .is_some_and(|p| !p.failed && p.session == self.generation.session)
    }
    pub(super) fn content_requirements(
        &self,
        module: &str,
        locale: Option<&str>,
        code: bool,
    ) -> Result<Vec<ContentRequest>> {
        if let Some(root) = self.validated.runtime_root() {
            let index = root
                .modules
                .get(module)
                .ok_or_else(|| Diagnostic::new("E_MODULE", module, "unknown module"))?;
            let mut keys = vec![ContentKey::Static {
                module: module.into(),
            }];
            if code {
                keys.push(ContentKey::Code {
                    module: module.into(),
                });
            }
            if let Some(locale) = locale {
                if !root.locales.contains(locale) {
                    return Err(Diagnostic::new("E_LOCALE", module, locale));
                }
                if !index.texts.is_empty() {
                    keys.push(ContentKey::Text {
                        module: module.into(),
                        locale: locale.into(),
                    });
                }
            }
            return self.requests_for_keys(keys);
        }
        let p = self.validated.program();
        let index = p
            .modules
            .get(module)
            .ok_or_else(|| Diagnostic::new("E_MODULE", module, "unknown module"))?;
        let mut objects = vec![];
        if code && index.functions.keys().any(|f| !p.functions.contains_key(f)) {
            objects.push(ContentRequest {
                key: None,
                module: module.into(),
                locale: None,
                hash: index.code.clone(),
            });
        }
        if let Some(locale) = locale {
            let texts = p
                .locales
                .get(locale)
                .ok_or_else(|| Diagnostic::new("E_LOCALE", module, locale))?;
            if index.texts.iter().any(|t| !texts.contains_key(t)) {
                let hash = index.locales.get(locale).ok_or_else(|| {
                    Diagnostic::new("E_MODULE_TEXT", module, "missing locale object")
                })?;
                objects.push(ContentRequest {
                    key: None,
                    module: module.into(),
                    locale: Some(locale.into()),
                    hash: hash.clone(),
                });
            }
        }
        if objects
            .iter()
            .any(|o| o.hash.len() != 64 || !o.hash.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(Diagnostic::new(
                "E_MODULE_OBJECT",
                module,
                "invalid content object identity",
            ));
        }
        Ok(objects)
    }
    fn requests_for_keys(
        &self,
        keys: impl IntoIterator<Item = ContentKey>,
    ) -> Result<Vec<ContentRequest>> {
        let Some(root) = self.validated.runtime_root() else {
            return Ok(vec![]);
        };
        let mut objects = vec![];
        for key in keys.into_iter().collect::<BTreeSet<_>>() {
            if self.validated.is_resident(&key) {
                continue;
            }
            let requirement = root.content_requirement(&key).ok_or_else(|| {
                Diagnostic::new("E_MODULE_OBJECT", "content", "missing object identity")
            })?;
            let (module, locale) = match &key {
                ContentKey::Static { module } | ContentKey::Code { module } => {
                    (module.clone(), None)
                }
                ContentKey::Text { module, locale } => (module.clone(), Some(locale.clone())),
                ContentKey::Catalog { .. } => (String::new(), None),
            };
            objects.push(ContentRequest {
                key: Some(key),
                module,
                locale,
                hash: requirement.digest,
            });
        }
        Ok(objects)
    }
    pub(super) fn asset_content_requirements(
        &self,
        ids: &BTreeSet<String>,
    ) -> Result<Vec<ContentRequest>> {
        let Some(root) = self.validated.runtime_root() else {
            return Ok(vec![]);
        };
        let keys = ids
            .iter()
            .map(|id| {
                root.assets
                    .get(id)
                    .map(|asset| ContentKey::Catalog {
                        catalog: asset.catalog.clone(),
                    })
                    .ok_or_else(|| Diagnostic::new("E_ASSET", id, "unknown asset"))
            })
            .collect::<Result<Vec<_>>>()?;
        self.requests_for_keys(keys)
    }
    fn snapshot_content_keys(&self, snapshot: &Snapshot) -> BTreeSet<ContentKey> {
        let Some(root) = self.validated.runtime_root() else {
            return BTreeSet::new();
        };
        let mut keys = BTreeSet::new();
        let mut modules = BTreeSet::new();
        for frame in &snapshot.frames {
            if let Some(index) = root.function_index.get(&frame.function) {
                modules.insert(index.module.clone());
                keys.insert(ContentKey::Code {
                    module: index.module.clone(),
                });
            }
        }
        if let Some(frame) = snapshot.frames.last() {
            if let Some(index) = root.function_index.get(&frame.function) {
                keys.insert(ContentKey::Text {
                    module: index.module.clone(),
                    locale: snapshot.locale.clone(),
                });
            }
        }
        for task in snapshot.tasks.values() {
            if let Some(module) = root.task_owners.get(&task.name) {
                modules.insert(module.clone());
            }
        }
        if let Some(pending) = &snapshot.pending {
            if let Some(module) = root.cue_owners.get(&pending.cue) {
                modules.insert(module.clone());
            }
        }
        if let Some(choice) = &snapshot.choice {
            if let Some(module) = root.choice_owners.get(&choice.id) {
                modules.insert(module.clone());
            }
        }
        for dialogue in snapshot
            .tasks
            .values()
            .filter_map(|t| t.dialogue.as_ref())
            .chain(snapshot.pending.iter().flat_map(|p| p.dialogues.values()))
        {
            if let Some(module) = root.text_owners.get(&dialogue.text_id) {
                modules.insert(module.clone());
                keys.insert(ContentKey::Text {
                    module: module.clone(),
                    locale: dialogue.locale.clone(),
                });
            }
        }
        keys.extend(
            modules
                .into_iter()
                .map(|module| ContentKey::Static { module }),
        );
        keys
    }
    pub(super) fn refresh_content_lease(&mut self) -> Result<()> {
        let Some(root) = self.validated.runtime_root() else {
            return Ok(());
        };
        let mut groups = BTreeMap::new();
        if self.screen != Screen::Title {
            groups.insert(
                "active-execution".to_owned(),
                self.snapshot_content_keys(self.core.state()),
            );
        }
        if let Some(candidate) = &self.candidate {
            groups.insert(
                "restore-candidate".to_owned(),
                self.snapshot_content_keys(candidate.state()),
            );
        }
        for (request, preparation) in &self.content {
            let mut keys: BTreeSet<_> = preparation
                .objects
                .iter()
                .filter_map(|r| r.key.clone())
                .collect();
            if let ContentPurpose::Restore(snapshot, _) = &preparation.purpose {
                keys.extend(self.snapshot_content_keys(snapshot));
            }
            groups.insert(format!("content-request-{request}"), keys);
        }
        let mut assets = self.retained_assets();
        if let Some(candidate) = &self.locale_candidate {
            assets.extend(self.font_assets(&candidate.ui_locale, &candidate.text_locale));
        }
        let catalog_keys = assets
            .iter()
            .filter_map(|asset| {
                root.assets.get(asset).map(|index| ContentKey::Catalog {
                    catalog: index.catalog.clone(),
                })
            })
            .collect();
        groups.insert("active-and-candidate-media".to_owned(), catalog_keys);
        let mut leases = vec![];
        for (owner, mut keys) in groups {
            keys.retain(|key| self.validated.is_resident(key));
            if !keys.is_empty() {
                leases.push(self.validated.lease(keys, owner)?);
            }
        }
        // Acquire before dropping old leases: M2.2 must never see a gap in
        // coverage while execution/preparation views are replaced.
        self.content_leases = leases;
        Ok(())
    }
    pub(super) fn cancel_content(&mut self, locale: bool) {
        let ids: Vec<_> = self
            .content
            .iter()
            .filter(|(_, p)| matches!(p.purpose, ContentPurpose::Locale) == locale)
            .map(|(id, _)| *id)
            .collect();
        for request in ids {
            self.content.remove(&request);
            self.commands.push(AppCommand::CancelContent { request });
        }
        self.pauses
            .remove(if locale { "locale" } else { "content" });
    }
    pub(super) fn begin_content(
        &mut self,
        purpose: ContentPurpose,
        objects: Vec<ContentRequest>,
    ) -> Result<()> {
        let locale = matches!(purpose, ContentPurpose::Locale);
        if objects.len() > 128 {
            return Err(Diagnostic::new(
                "E_LIMIT",
                "content",
                "content batch exceeds 128 objects",
            ));
        }
        self.cancel_content(locale);
        if objects.is_empty() {
            return Err(Diagnostic::new(
                "E_MODULE",
                "prepare",
                "empty content barrier",
            ));
        }
        self.request = self
            .request
            .checked_add(1)
            .ok_or_else(|| Diagnostic::new("E_LIMIT", "content", "request counter overflow"))?;
        let request = self.request;
        self.content.insert(
            request,
            ContentPreparation {
                purpose,
                objects: objects.clone(),
                session: self.generation.session,
                failed: false,
            },
        );
        self.pauses
            .insert(if locale { "locale" } else { "content" }.into());
        if !locale {
            self.error = None;
            self.diagnostic = None;
        }
        self.commands.push(AppCommand::GetContent {
            request,
            session: self.generation.session,
            objects,
        });
        self.observe("content_requested", Some(request));
        Ok(())
    }
    pub(super) fn fail_content(&mut self, request: u32, message: String) {
        if !self.accepts_content(request) {
            return;
        }
        let job = self.content.get_mut(&request).unwrap();
        job.failed = true;
        self.commands.push(AppCommand::CancelContent { request });
        if matches!(job.purpose, ContentPurpose::Locale) {
            self.locale_error = Some(message);
            self.pauses.remove("locale");
        } else {
            self.report(
                Diagnostic::new("E_MODULE_PREPARE", "content", message).classified(
                    ErrorDomain::Prepare,
                    "load-module",
                    "content",
                    vec![Recovery::Retry, Recovery::Exit],
                ),
                true,
            );
        }
    }
    pub(super) fn complete_content(&mut self, request: u32, bytes: Vec<Vec<u8>>) -> Result<()> {
        if !self.accepts_content(request) {
            return Ok(());
        }
        let job = self.content[&request].clone();
        if bytes.len() != job.objects.len()
            || bytes.len() > 128
            || bytes
                .iter()
                .try_fold(0usize, |sum, item| sum.checked_add(item.len()))
                .is_none_or(|total| total > MAX_INPUT_BYTES)
        {
            return Err(Diagnostic::new(
                "E_LIMIT",
                "content",
                "content batch count/bytes",
            ));
        }
        let validated = if let Some(root) = self.validated.runtime_root() {
            let objects = job
                .objects
                .iter()
                .zip(bytes)
                .map(|(request, bytes)| {
                    let key = request.key.as_ref().ok_or_else(|| {
                        Diagnostic::new("E_MODULE", "content", "runtime request missing typed key")
                    })?;
                    let object = nir_content::parse_runtime_object(root, key, &bytes)?;
                    Ok((key.clone(), object, bytes.len() as u64))
                })
                .collect::<Result<Vec<_>>>()?;
            self.validated.install_batch(objects)?
        } else {
            let mut p = self
                .validated
                .legacy_program()
                .ok_or_else(|| Diagnostic::new("E_MODULE", "content", "missing source program"))?;
            for (object, bytes) in job.objects.iter().zip(bytes) {
                match &object.locale {
                    Some(locale) => {
                        nir_content::install_module_texts(&mut p, &bytes, &object.module, locale)?
                    }
                    None => nir_content::install_module_code(&mut p, &bytes, &object.module)?,
                }
            }
            // Preserve the existing finite content envelope even across many loads.
            if serde_json::to_vec(&p)
                .map_err(|e| Diagnostic::new("E_MODULE", "content", e.to_string()))?
                .len()
                > MAX_INPUT_BYTES
            {
                return Err(Diagnostic::new(
                    "E_LIMIT",
                    "content",
                    "resident program exceeds 16 MiB",
                ));
            }
            ValidatedProgram::new(p)?
        };
        // Validate restore candidates before installing anything into the live view.
        if let ContentPurpose::Restore(snapshot, _) = &job.purpose {
            Core::restore(validated.clone(), *snapshot.clone(), &self.release)?;
        }
        self.core.replace_program(validated.clone())?;
        if let Some(candidate) = &mut self.candidate {
            candidate.replace_program(validated.clone())?;
        }
        self.validated = validated;
        self.content.remove(&request);
        self.pauses
            .remove(if matches!(job.purpose, ContentPurpose::Locale) {
                "locale"
            } else {
                "content"
            });
        self.observe("content_ready", Some(request));
        let continuation = match job.purpose.clone() {
            ContentPurpose::Execution => {
                self.error = None;
                self.diagnostic = None;
                Ok(())
            }
            ContentPurpose::Media {
                purpose,
                activation,
                assets,
            } => self.begin_prepare(purpose, activation, assets),
            ContentPurpose::Restore(snapshot, rollback) => {
                self.restore_with_purpose(*snapshot, rollback)
            }
            ContentPurpose::Locale => self.start_locale_switch(),
        };
        if let Err(error) = continuation {
            // Bytes are already verified, but admitting the following media or
            // locale preparation can still fail. Keep a retryable request so
            // this error cannot disappear after removal of the download job.
            let locale = matches!(job.purpose, ContentPurpose::Locale);
            if locale {
                self.invalidate_locale_candidate();
            }
            self.content.insert(request, job);
            self.pauses
                .insert(if locale { "locale" } else { "content" }.into());
            self.fail_content(request, error.to_string());
        }
        Ok(())
    }
    pub(super) fn restore_content_requirements(&self, s: &Snapshot) -> Result<Vec<ContentRequest>> {
        let p = self.validated.program();
        if s.format != SNAPSHOT_VERSION
            || s.game_id != p.game_id
            || s.revision != p.revision
            || s.release != self.release
            || s.frames.is_empty()
            || s.frames.len() > MAX_FRAMES
            || s.tasks.len() > MAX_TASKS * 2
        {
            return Err(Diagnostic::new(
                "E_SNAPSHOT",
                "restore",
                "incompatible content or state limits",
            ));
        }
        let mut objects = vec![];
        if let Some(root) = self.validated.runtime_root() {
            let mut modules = BTreeSet::new();
            for task in s.tasks.values() {
                let owner = root.task_owners.get(&task.name).ok_or_else(|| {
                    Diagnostic::new("E_SNAPSHOT", "restore", "unknown task definition")
                })?;
                modules.insert(owner.clone());
            }
            if let Some(pending) = &s.pending {
                let owner = root.cue_owners.get(&pending.cue).ok_or_else(|| {
                    Diagnostic::new("E_SNAPSHOT", "restore", "unknown pending cue")
                })?;
                modules.insert(owner.clone());
            }
            if let Some(choice) = &s.choice {
                let owner = root
                    .choice_owners
                    .get(&choice.id)
                    .ok_or_else(|| Diagnostic::new("E_SNAPSHOT", "restore", "unknown choice"))?;
                modules.insert(owner.clone());
            }
            objects.extend(
                self.requests_for_keys(
                    modules
                        .into_iter()
                        .map(|module| ContentKey::Static { module }),
                )?,
            );
            let mut assets: BTreeSet<String> = s
                .scene
                .iter()
                .chain(&s.draft)
                .chain(
                    s.tasks
                        .values()
                        .flat_map(|t| t.source.iter().chain(&t.target)),
                )
                .filter_map(|node| node.asset.clone())
                .collect();
            for task in s.tasks.values() {
                if let Effect::Audio { asset, .. } = &task.effect {
                    assets.insert(asset.clone());
                }
            }
            assets.extend(self.font_assets(&self.effective_ui_locale, &self.effective_text_locale));
            for locale in s
                .tasks
                .values()
                .filter_map(|t| t.dialogue.as_ref().map(|d| &d.locale))
                .chain(
                    s.pending
                        .iter()
                        .flat_map(|p| p.dialogues.values().map(|d| &d.locale)),
                )
                .chain(s.choice.iter().map(|c| &c.locale))
                .chain(s.history.iter().map(|h| &h.locale))
            {
                let plan = root.locale_config.text.get(locale).ok_or_else(|| {
                    Diagnostic::new("E_SNAPSHOT", "restore", "unknown frozen locale")
                })?;
                assets.extend(plan.fonts.iter().cloned());
            }
            objects.extend(self.asset_content_requirements(&assets)?);
        }
        let mut add = |items: Vec<ContentRequest>| {
            for item in items {
                if !objects.contains(&item) {
                    objects.push(item);
                }
            }
        };
        for f in &s.frames {
            if let Some(module) = p.function_module(&f.function) {
                add(self.content_requirements(module, None, true)?);
            }
        }
        for d in s.tasks.values().filter_map(|t| t.dialogue.as_ref()).chain(
            s.pending
                .iter()
                .flat_map(|pending| pending.dialogues.values()),
        ) {
            if let Some(module) = p.text_module(&d.text_id) {
                add(self.content_requirements(module, Some(&d.locale), false)?);
            }
        }
        Ok(objects)
    }
}
