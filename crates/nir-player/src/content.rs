use super::*;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContentRequest {
    pub module: String,
    pub locale: Option<String>,
    pub hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let p = self.validated.program();
        let index = p
            .modules
            .get(module)
            .ok_or_else(|| Diagnostic::new("E_MODULE", module, "unknown module"))?;
        let mut objects = vec![];
        if code && index.functions.keys().any(|f| !p.functions.contains_key(f)) {
            objects.push(ContentRequest {
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
            || bytes.iter().map(Vec::len).sum::<usize>() > MAX_INPUT_BYTES
        {
            return Err(Diagnostic::new(
                "E_LIMIT",
                "content",
                "content batch count/bytes",
            ));
        }
        let mut p = self.validated.program().clone();
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
