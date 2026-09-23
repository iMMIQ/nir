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

    fn player() -> Player {
        let program: Program =
            serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        Player::new(program, "release".into(), "Test".into()).unwrap()
    }

    fn request() -> ContentRequest {
        ContentRequest {
            key: Some(ContentKey::Static {
                module: "story".into(),
            }),
            module: "story".into(),
            locale: None,
            hash: "ab".repeat(32),
        }
    }

    fn preparation(purpose: ContentPurpose, max_bytes: Option<u64>) -> ContentPreparation {
        ContentPreparation {
            purpose,
            objects: vec![request()],
            session: 1,
            failed: false,
            max_bytes,
        }
    }

    #[test]
    fn promoted_prefetch_skipped_retries_as_a_required_request() {
        let mut player = player();
        player.commands.clear();
        player.request = 41;
        player
            .content
            .insert(41, preparation(ContentPurpose::Prefetch, Some(1024)));

        player
            .begin_content(ContentPurpose::Execution, vec![request()])
            .unwrap();
        assert!(player
            .commands
            .iter()
            .any(|command| matches!(command, AppCommand::PromoteContent { request: 41, .. })));
        assert!(matches!(
            player.content[&41].purpose,
            ContentPurpose::Execution
        ));

        player.commands.clear();
        player
            .event(
                AppEvent::ContentSkipped {
                    request: 41,
                    code: "E_PREFETCH_LIMIT".into(),
                    message: "manifest too large".into(),
                },
                &mut 100,
            )
            .unwrap();
        assert!(!player.content.contains_key(&41));
        let (request_id, job) = player.content.iter().next().unwrap();
        assert_eq!(*request_id, 42);
        assert!(matches!(job.purpose, ContentPurpose::Execution));
        assert_eq!(job.max_bytes, None);
        assert!(player.commands.iter().any(|command| matches!(
            command,
            AppCommand::GetContent {
                request: 42,
                priority: ContentPriority::Required,
                max_bytes: None,
                ..
            }
        )));
        assert!(player.error.is_none());
    }

    #[test]
    fn speculative_skip_is_silent_and_remembers_the_wait_fingerprint() {
        let mut player = player();
        player.commands.clear();
        player.prepare = None;
        player.pauses.remove("prepare");
        player.request = 7;
        let attempt = PrefetchAttempt {
            wait: "main/start/0:wait".into(),
            function: "main".into(),
            module: "next".into(),
            locale: "en".into(),
            session: player.generation.session,
        };
        player.prefetch_attempted = Some(attempt.clone());
        let mut job = preparation(ContentPurpose::Prefetch, Some(1024));
        job.session = player.generation.session;
        player.content.insert(7, job);

        player
            .event(
                AppEvent::ContentSkipped {
                    request: 7,
                    code: "E_PREFETCH_LIMIT".into(),
                    message: "manifest too large".into(),
                },
                &mut 100,
            )
            .unwrap();
        assert!(player.content.is_empty());
        assert_eq!(player.prefetch_attempted, Some(attempt));
        assert!(player.error.is_none());
        assert!(!player.paused());
        assert!(!player
            .commands
            .iter()
            .any(|command| matches!(command, AppCommand::GetContent { .. })));
    }

    #[test]
    fn oversized_speculative_completion_is_discarded_without_installing() {
        let mut player = player();
        player.commands.clear();
        player.request = 8;
        let mut job = preparation(ContentPurpose::Prefetch, Some(1024));
        job.session = player.generation.session;
        player.content.insert(8, job);

        player.complete_content(8, vec![vec![0; 1025]]).unwrap();
        assert!(!player.content.contains_key(&8));
        assert!(player.error.is_none());
        assert!(!player
            .commands
            .iter()
            .any(|command| matches!(command, AppCommand::Diagnostic { .. })));
    }

    #[test]
    fn title_cancels_staged_restore_and_late_content_cannot_resume_it() {
        let mut player = player();
        player.commands.clear();
        player.candidate = Some(player.core.clone());
        player.restore_work = Some(RestoreWork {
            session: None,
            proof: None,
            active_batches: VecDeque::new(),
            rollback: false,
        });
        let mut job = preparation(ContentPurpose::RestoreBodies(false), None);
        job.session = player.generation.session;
        player.content.insert(55, job);
        let mut budget = 100;

        player.action(UiAction::Title, 0, 0, &mut budget).unwrap();
        assert!(player.restore_work.is_none());
        assert!(player.candidate.is_none());
        assert!(!player.content.contains_key(&55));
        assert!(player
            .commands
            .iter()
            .any(|command| matches!(command, AppCommand::CancelContent { request: 55 })));
        // A late terminal callback for the removed request is ignored.
        player
            .event(
                AppEvent::ContentReady {
                    request: 55,
                    objects: vec![vec![]],
                },
                &mut budget,
            )
            .unwrap();
        assert!(player.restore_work.is_none());
        assert!(player.candidate.is_none());
    }

    #[test]
    fn device_recovery_keeps_restore_purpose_and_restarts_requested_locale() {
        let mut player = player();
        player.commands.clear();
        player.prepare = None;
        player.candidate = Some(player.core.clone());
        player.restore_work = Some(RestoreWork {
            session: None,
            proof: None,
            active_batches: VecDeque::new(),
            rollback: false,
        });
        player.preferences.text_locale = "en".into();
        player.pauses.insert("device".into());
        let mut budget = 100;

        player.event(AppEvent::DeviceReady, &mut budget).unwrap();
        assert!(matches!(
            player.prepare.as_ref().unwrap().purpose,
            Purpose::Restore
        ));
        assert_eq!(
            player
                .locale_candidate
                .as_ref()
                .map(|candidate| candidate.text_locale.as_str()),
            Some("en")
        );
    }

    #[test]
    fn retry_restarts_media_after_verified_restore_consumed_its_proof() {
        let mut player = player();
        player.commands.clear();
        player.prepare = None;
        player.candidate = Some(player.core.clone());
        player.restore_work = Some(RestoreWork {
            session: None,
            proof: None,
            active_batches: VecDeque::new(),
            rollback: true,
        });

        player.action(UiAction::Retry, 0, 0, &mut 100).unwrap();
        assert!(player.prepare.is_some());
        assert!(matches!(
            player.prepare.as_ref().unwrap().purpose,
            Purpose::Rollback
        ));
    }

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
    RestoreValidation,
    RestoreBodies(bool),
    Locale,
    Prefetch,
}
#[derive(Clone)]
pub(super) struct ContentPreparation {
    pub purpose: ContentPurpose,
    pub objects: Vec<ContentRequest>,
    pub session: u32,
    pub failed: bool,
    pub max_bytes: Option<u64>,
}

pub(super) struct RestoreWork {
    pub session: Option<RestoreSession>,
    pub proof: Option<VerifiedSnapshot>,
    pub active_batches: VecDeque<Vec<ContentRequest>>,
    pub rollback: bool,
}
impl RestoreWork {
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.session
            .as_ref()
            .map(RestoreSession::snapshot)
            .or_else(|| self.proof.as_ref().map(VerifiedSnapshot::snapshot))
    }
}

fn same_content_objects(left: &[ContentRequest], right: &[ContentRequest]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| {
            a.key == b.key && a.module == b.module && a.locale == b.locale && a.hash == b.hash
        })
}
impl Player {
    pub(super) fn start_staged_restore(
        &mut self,
        snapshot: Snapshot,
        rollback: bool,
    ) -> Result<()> {
        self.cancel_content(false);
        self.cancel_preparation();
        self.candidate = None;
        self.restore_work = None;
        self.pauses.remove("content");
        self.refresh_content_lease()?;
        let session = RestoreSession::new(&self.validated, snapshot, &self.release)?;
        self.restore_work = Some(RestoreWork {
            session: Some(session),
            proof: None,
            active_batches: VecDeque::new(),
            rollback,
        });
        self.resume_restore_work()
    }
    pub(super) fn restore_locale_changed(&mut self) -> Result<()> {
        let Some(proof) = self
            .restore_work
            .as_ref()
            .and_then(|work| work.proof.as_ref())
        else {
            return Ok(());
        };
        if self.candidate.is_some() {
            return Ok(());
        }
        let batches = self.restore_active_batches(proof.snapshot())?;
        let requests: Vec<_> = self
            .content
            .iter()
            .filter(|(_, job)| matches!(job.purpose, ContentPurpose::RestoreBodies(_)))
            .map(|(request, _)| *request)
            .collect();
        for request in requests {
            self.content.remove(&request);
            self.commands.push(AppCommand::CancelContent { request });
        }
        self.pauses.remove("content");
        if let Some(work) = &mut self.restore_work {
            work.active_batches = batches;
        }
        self.resume_restore_work()
    }
    fn restore_requirements_to_requests(
        &self,
        requirements: Vec<ContentRequirement>,
    ) -> Vec<ContentRequest> {
        requirements
            .into_iter()
            .map(|requirement| {
                let (module, locale) = match &requirement.key {
                    ContentKey::Static { module } | ContentKey::Code { module } => {
                        (module.clone(), None)
                    }
                    ContentKey::Text { module, locale } => (module.clone(), Some(locale.clone())),
                    ContentKey::Catalog { catalog } => (catalog.clone(), None),
                };
                ContentRequest {
                    key: Some(requirement.key),
                    module,
                    locale,
                    hash: requirement.digest,
                }
            })
            .collect()
    }
    fn restore_active_batches(&self, snapshot: &Snapshot) -> Result<VecDeque<Vec<ContentRequest>>> {
        let root = self
            .validated
            .runtime_root()
            .ok_or_else(|| Diagnostic::new("E_SNAPSHOT", "restore", "missing runtime root"))?;
        let mut by_module = BTreeMap::<String, BTreeSet<ContentKey>>::new();
        for frame in &snapshot.frames {
            let index = root
                .function_index
                .get(&frame.function)
                .ok_or_else(|| Diagnostic::new("E_SNAPSHOT", "restore", "unknown function"))?;
            let keys = by_module.entry(index.module.clone()).or_default();
            keys.insert(ContentKey::Static {
                module: index.module.clone(),
            });
            keys.insert(ContentKey::Code {
                module: index.module.clone(),
            });
        }
        if let Some(frame) = snapshot.frames.last() {
            if let Some(index) = root.function_index.get(&frame.function) {
                if root
                    .modules
                    .get(&index.module)
                    .is_some_and(|module| !module.texts.is_empty())
                {
                    by_module
                        .entry(index.module.clone())
                        .or_default()
                        .insert(ContentKey::Text {
                            module: index.module.clone(),
                            locale: self.effective_text_locale.clone(),
                        });
                }
            }
        }
        if let Some(pending) = &snapshot.pending {
            let module = root
                .cue_owners
                .get(&pending.cue)
                .ok_or_else(|| Diagnostic::new("E_SNAPSHOT", "restore", "unknown cue"))?;
            by_module
                .entry(module.clone())
                .or_default()
                .insert(ContentKey::Static {
                    module: module.clone(),
                });
        }
        if let Some(choice) = &snapshot.choice {
            let module = root
                .choice_owners
                .get(&choice.id)
                .ok_or_else(|| Diagnostic::new("E_SNAPSHOT", "restore", "unknown choice"))?;
            by_module
                .entry(module.clone())
                .or_default()
                .insert(ContentKey::Static {
                    module: module.clone(),
                });
        }
        let mut batches = VecDeque::new();
        let mut current = Vec::new();
        for keys in by_module.values() {
            let required = self.requests_for_keys(keys.iter().cloned())?;
            if required.is_empty() {
                continue;
            }
            if current.len() + required.len() > 128 {
                batches.push_back(std::mem::take(&mut current));
            }
            current.extend(required);
        }
        if !current.is_empty() {
            batches.push_back(current);
        }
        Ok(batches)
    }
    pub(super) fn resume_restore_work(&mut self) -> Result<()> {
        let mut work = self
            .restore_work
            .take()
            .ok_or_else(|| Diagnostic::new("E_SNAPSHOT", "restore", "missing restore session"))?;
        if let Some(session) = &work.session {
            if let Some(requirements) = session.next_requirements() {
                let objects = self.restore_requirements_to_requests(requirements);
                self.restore_work = Some(work);
                return self.begin_content(ContentPurpose::RestoreValidation, objects);
            }
        }
        if let Some(session) = work.session.take() {
            let proof = session.finish()?;
            work.active_batches = self.restore_active_batches(proof.snapshot())?;
            work.proof = Some(proof);
        }
        if let Some(objects) = work.active_batches.pop_front() {
            let rollback = work.rollback;
            self.restore_work = Some(work);
            return self.begin_content(ContentPurpose::RestoreBodies(rollback), objects);
        }
        let proof = work
            .proof
            .take()
            .ok_or_else(|| Diagnostic::new("E_SNAPSHOT", "restore", "missing verified snapshot"))?;
        let mut candidate = Core::restore_verified(self.validated.clone(), proof, &self.release)?;
        candidate.set_locale(&self.effective_text_locale)?;
        let assets = self.state_assets(&candidate);
        self.candidate = Some(candidate);
        let purpose = if work.rollback {
            Purpose::Rollback
        } else {
            Purpose::Restore
        };
        self.restore_work = Some(work);
        self.begin_prepare(purpose, 0, assets)
    }
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
        self.snapshot_content_keys_for_locale(snapshot, &snapshot.locale)
    }
    fn snapshot_content_keys_for_locale(
        &self,
        snapshot: &Snapshot,
        locale: &str,
    ) -> BTreeSet<ContentKey> {
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
                    locale: locale.to_owned(),
                });
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
        keys.extend(
            modules
                .into_iter()
                .map(|module| ContentKey::Static { module }),
        );
        keys
    }
    pub(super) fn touch_snapshot_content(&self, snapshot: &Snapshot) -> Result<()> {
        if self.validated.runtime_root().is_none() {
            return Ok(());
        }
        let mut keys = self.snapshot_content_keys(snapshot);
        keys.retain(|key| self.validated.is_resident(key));
        if !keys.is_empty() {
            self.validated.touch_content(&keys)?;
        }
        Ok(())
    }
    pub(super) fn refresh_content_lease(&mut self) -> Result<()> {
        let Some(root) = self.validated.runtime_root() else {
            return Ok(());
        };
        let mut groups = BTreeMap::new();
        if self.story_context_active() {
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
        if let Some(snapshot) = self.restore_work.as_ref().and_then(RestoreWork::snapshot) {
            groups.insert(
                "restore-candidate".to_owned(),
                self.snapshot_content_keys_for_locale(snapshot, &self.effective_text_locale),
            );
        }
        for (request, preparation) in &self.content {
            if matches!(
                preparation.purpose,
                ContentPurpose::RestoreValidation | ContentPurpose::Prefetch
            ) {
                continue;
            }
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
        self.cancel_content_except(locale, None);
    }
    fn cancel_content_except(&mut self, locale: bool, keep: Option<u32>) {
        let ids: Vec<_> = self
            .content
            .iter()
            .filter(|(id, p)| {
                Some(**id) != keep
                    && (matches!(p.purpose, ContentPurpose::Locale) == locale
                        || matches!(p.purpose, ContentPurpose::Prefetch))
            })
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
        if objects.is_empty() {
            return Err(Diagnostic::new(
                "E_MODULE",
                "prepare",
                "empty content barrier",
            ));
        }
        if !matches!(purpose, ContentPurpose::Prefetch) {
            let matching_prefetch = self.content.iter().find_map(|(request, preparation)| {
                (matches!(preparation.purpose, ContentPurpose::Prefetch)
                    && same_content_objects(&preparation.objects, &objects))
                .then_some(*request)
            });
            if let Some(request) = matching_prefetch {
                self.cancel_content_except(locale, Some(request));
                if let Some(preparation) = self.content.get_mut(&request) {
                    preparation.purpose = purpose;
                }
                self.pauses
                    .insert(if locale { "locale" } else { "content" }.into());
                self.commands.push(AppCommand::PromoteContent {
                    request,
                    session: self.generation.session,
                });
                self.observe("content_promoted", Some(request));
                return Ok(());
            }
            self.cancel_content(locale);
        } else {
            let old_prefetches: Vec<_> = self
                .content
                .iter()
                .filter(|(_, p)| matches!(p.purpose, ContentPurpose::Prefetch))
                .map(|(id, _)| *id)
                .collect();
            for request in old_prefetches {
                self.content.remove(&request);
                self.commands.push(AppCommand::CancelContent { request });
            }
        }
        let is_prefetch = matches!(purpose, ContentPurpose::Prefetch);
        let max_bytes = if is_prefetch {
            let residency = self.validated.residency();
            let available = residency
                .budget_bytes
                .saturating_sub(residency.resident_bytes)
                .min(2 * 1024 * 1024);
            if available == 0 {
                return Ok(());
            }
            Some(available)
        } else {
            None
        };
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
                max_bytes,
            },
        );
        if !is_prefetch {
            self.pauses
                .insert(if locale { "locale" } else { "content" }.into());
        }
        if !locale && !is_prefetch {
            self.error = None;
            self.diagnostic = None;
        }
        self.commands.push(AppCommand::GetContent {
            request,
            session: self.generation.session,
            objects,
            priority: if is_prefetch {
                ContentPriority::Prefetch
            } else {
                ContentPriority::Required
            },
            max_bytes,
        });
        self.observe(
            if is_prefetch {
                "prefetch_requested"
            } else {
                "content_requested"
            },
            Some(request),
        );
        Ok(())
    }
    pub(super) fn fail_content(&mut self, request: u32, message: String) {
        if !self.accepts_content(request) {
            return;
        }
        let purpose = self.content[&request].purpose.clone();
        self.content.get_mut(&request).unwrap().failed = true;
        self.commands.push(AppCommand::CancelContent { request });
        if matches!(purpose, ContentPurpose::Prefetch) {
            self.content.remove(&request);
            self.observe("prefetch_failed", Some(request));
        } else if matches!(purpose, ContentPurpose::Locale) {
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
    fn complete_restore_validation(
        &mut self,
        request: u32,
        job: &ContentPreparation,
        bytes: Vec<Vec<u8>>,
    ) -> Result<()> {
        let root = self
            .validated
            .runtime_root()
            .ok_or_else(|| Diagnostic::new("E_SNAPSHOT", "restore", "missing runtime root"))?;
        let objects = job
            .objects
            .iter()
            .zip(&bytes)
            .map(|(request, bytes)| {
                let key = request.key.as_ref().ok_or_else(|| {
                    Diagnostic::new("E_SNAPSHOT", "restore", "missing content key")
                })?;
                let object = nir_content::parse_runtime_object(root, key, bytes)?;
                Ok((key.clone(), object, bytes.len() as u64))
            })
            .collect::<Result<Vec<_>>>()?;
        let scratch = self
            .validated
            .empty_content_view()?
            .install_batch(objects)?;
        let work = self
            .restore_work
            .as_mut()
            .ok_or_else(|| Diagnostic::new("E_SNAPSHOT", "restore", "missing restore session"))?;
        work.session
            .as_mut()
            .ok_or_else(|| {
                Diagnostic::new("E_SNAPSHOT", "restore", "verification already finished")
            })?
            .verify_next_unit(&scratch)?;
        self.content.remove(&request);
        self.pauses.remove("content");
        self.observe("restore_unit_verified", Some(request));
        if let Err(error) = self.resume_restore_work() {
            self.pauses.insert("content".into());
            self.report(error, true);
        }
        Ok(())
    }
    fn complete_restore_bodies(
        &mut self,
        request: u32,
        job: ContentPreparation,
        objects: Vec<(ContentKey, RuntimeObject, u64)>,
    ) -> Result<()> {
        let rollback = match job.purpose {
            ContentPurpose::RestoreBodies(rollback) => rollback,
            _ => unreachable!(),
        };
        self.admit_live_batch(request, objects)?;
        self.content.remove(&request);
        self.pauses.remove("content");
        self.observe("restore_body_ready", Some(request));
        debug_assert_eq!(
            self.restore_work.as_ref().map(|work| work.rollback),
            Some(rollback)
        );
        if let Err(error) = self.resume_restore_work() {
            self.pauses.insert("content".into());
            self.report(error, true);
        }
        Ok(())
    }
    fn complete_prefetch(
        &mut self,
        request: u32,
        _job: &ContentPreparation,
        objects: Vec<(ContentKey, RuntimeObject, u64)>,
    ) -> Result<()> {
        // A speculative request cannot retire resident blocks. Any parse,
        // accounting, or promotion race failure remains an optional fallback.
        let result = (|| {
            let admission = self.validated.prepare_prefetch_batch(objects)?;
            let projected = admission.view();
            if self.story_context_active() {
                self.core.check_program_replacement(projected)?;
            }
            if let Some(candidate) = &self.candidate {
                candidate.check_program_replacement(projected)?;
            }
            let validated = admission.commit()?;
            if self.story_context_active() {
                self.core.replace_program(validated.clone())?;
            } else {
                self.core = Core::new(
                    validated.clone(),
                    self.release.clone(),
                    self.effective_text_locale.clone(),
                )?;
            }
            if let Some(candidate) = &mut self.candidate {
                candidate.replace_program(validated.clone())?;
            }
            Ok::<_, Diagnostic>(validated)
        })();
        if let Ok(validated) = result {
            self.validated = validated;
            self.content.remove(&request);
            self.observe("prefetch_ready", Some(request));
        } else {
            self.content.remove(&request);
            self.commands.push(AppCommand::CancelContent { request });
            self.observe("prefetch_failed", Some(request));
        }
        Ok(())
    }
    fn admission_lease_keys(&self, view: &ValidatedProgram) -> BTreeSet<ContentKey> {
        let mut keys = BTreeSet::new();
        if self.story_context_active() {
            keys.extend(self.snapshot_content_keys(self.core.state()));
        }
        if let Some(candidate) = &self.candidate {
            keys.extend(self.snapshot_content_keys(candidate.state()));
        }
        if let Some(snapshot) = self.restore_work.as_ref().and_then(RestoreWork::snapshot) {
            keys.extend(
                self.snapshot_content_keys_for_locale(snapshot, &self.effective_text_locale),
            );
        }
        for preparation in self.content.values() {
            if matches!(
                preparation.purpose,
                ContentPurpose::RestoreValidation | ContentPurpose::Prefetch
            ) {
                continue;
            }
            keys.extend(
                preparation
                    .objects
                    .iter()
                    .filter_map(|object| object.key.clone()),
            );
        }
        if let Some(root) = view.runtime_root() {
            let mut assets = self.retained_assets();
            if let Some(candidate) = &self.locale_candidate {
                assets.extend(self.font_assets(&candidate.ui_locale, &candidate.text_locale));
            }
            keys.extend(root.asset_catalogs(assets.iter().map(String::as_str)));
        }
        keys.retain(|key| view.is_resident(key));
        keys
    }
    fn admit_live_batch(
        &mut self,
        request: u32,
        objects: Vec<(ContentKey, RuntimeObject, u64)>,
    ) -> Result<()> {
        let demanded: BTreeSet<_> = objects.iter().map(|(key, _, _)| key.clone()).collect();
        // Update reference ownership immediately before the residency ledger
        // chooses any blocks to retire.
        self.refresh_content_lease()?;
        let admission = self.validated.prepare_install_batch(objects)?;
        let projected = admission.view();
        if self.story_context_active() {
            self.core.check_program_replacement(projected)?;
        }
        if let Some(candidate) = &self.candidate {
            candidate.check_program_replacement(projected)?;
        }
        let lease_keys = self.admission_lease_keys(admission.view());
        let (validated, lease) =
            admission.commit_with_lease(lease_keys, format!("content-admission-{request}"))?;
        if self.story_context_active() {
            self.core.replace_program(validated.clone())?;
        } else {
            self.core = Core::new(
                validated.clone(),
                self.release.clone(),
                self.effective_text_locale.clone(),
            )?;
        }
        if let Some(candidate) = &mut self.candidate {
            candidate.replace_program(validated.clone())?;
        }
        self.content_leases.push(lease);
        self.validated = validated;
        if !demanded.is_empty() {
            self.validated.touch_content(&demanded)?;
        }
        self.refresh_content_lease()?;
        Ok(())
    }
    fn admit_live_program(&mut self, validated: ValidatedProgram) -> Result<()> {
        if self.story_context_active() {
            self.core.check_program_replacement(&validated)?;
        }
        if let Some(candidate) = &self.candidate {
            candidate.check_program_replacement(&validated)?;
        }
        if self.story_context_active() {
            self.core.replace_program(validated.clone())?;
        } else {
            self.core = Core::new(
                validated.clone(),
                self.release.clone(),
                self.effective_text_locale.clone(),
            )?;
        }
        if let Some(candidate) = &mut self.candidate {
            candidate.replace_program(validated.clone())?;
        }
        self.validated = validated;
        Ok(())
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
        if matches!(job.purpose, ContentPurpose::Prefetch) {
            let total_bytes: u64 = bytes.iter().map(|item| item.len() as u64).sum();
            let cap = job
                .max_bytes
                .unwrap_or(2 * 1024 * 1024)
                .min(2 * 1024 * 1024);
            if total_bytes > cap {
                self.content.remove(&request);
                self.commands.push(AppCommand::CancelContent { request });
                self.observe("prefetch_failed", Some(request));
                return Ok(());
            }
        }
        if matches!(job.purpose, ContentPurpose::RestoreValidation) {
            return self.complete_restore_validation(request, &job, bytes);
        }
        if let Some(root) = self.validated.runtime_root() {
            let objects = job
                .objects
                .iter()
                .zip(&bytes)
                .map(|(request, bytes)| {
                    let key = request.key.as_ref().ok_or_else(|| {
                        Diagnostic::new("E_MODULE", "content", "runtime request missing typed key")
                    })?;
                    let object = nir_content::parse_runtime_object(root, key, bytes)?;
                    Ok((key.clone(), object, bytes.len() as u64))
                })
                .collect::<Result<Vec<_>>>()?;
            if matches!(job.purpose, ContentPurpose::Prefetch) {
                self.complete_prefetch(request, &job, objects)?;
                return Ok(());
            }
            if matches!(job.purpose, ContentPurpose::RestoreBodies(_)) {
                return self.complete_restore_bodies(request, job, objects);
            }
            if let ContentPurpose::Restore(snapshot, _) = &job.purpose {
                let projected = self.validated.install_batch(objects.clone())?;
                Core::restore(projected, *snapshot.clone(), &self.release)?;
            }
            self.admit_live_batch(request, objects)?;
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
            let validated = ValidatedProgram::new(p)?;
            self.admit_live_program(validated)?;
        }
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
            ContentPurpose::RestoreValidation => unreachable!(),
            ContentPurpose::RestoreBodies(_) => unreachable!(),
            ContentPurpose::Prefetch => unreachable!(),
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
