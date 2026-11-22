use super::*;

pub(super) fn bad_runtime(at: &str, message: &str) -> Diagnostic {
    err("E_RUNTIME_ROOT", at, message)
}
pub(super) fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub(super) fn validate_menu_audio_loops(
    theme: &Theme,
    caps: CapabilitySet,
    duration: impl Fn(&str) -> Option<u64>,
) -> Result<()> {
    for (id, menu) in &theme.image_menus {
        if let Some(music) = menu.effects.as_ref().and_then(|e| e.music.as_ref()) {
            if let Some(region) = music.loop_region {
                if !caps.contains(Capability::AudioLoopRegion) {
                    return Err(err("E_CAPABILITY", id, "audio.loop-region.v1"));
                }
                // Runtime roots contain asset identities; the catalog later
                // supplies duration and completes the same validation.
                if !region.valid(duration(&music.asset).unwrap_or(u64::MAX)) {
                    return Err(err("E_AUDIO_LOOP", id, "invalid menu music loop region"));
                }
            }
        }
    }
    Ok(())
}

/// Shared v1/v2 validation for UI theme, image menus, dialogue styles and
/// player policies against declared capabilities.
pub(super) fn validate_theme_and_player(
    theme: &Theme,
    player: &PlayerDefaults,
    variables: &BTreeMap<String, Value>,
    requires: &[String],
    caps: CapabilitySet,
    audio_duration: impl Fn(&str) -> Option<u64>,
) -> Result<()> {
    validate_ui_config(theme, player)?;
    if theme.image_menus.len() > 64 && !caps.contains(Capability::UiMenuPages) {
        return Err(err("E_CAPABILITY", "theme.image_menus", "ui.menu-pages.v1"));
    }
    validate_menu_audio_loops(theme, caps, audio_duration)?;
    let require_menu_cap = |used: bool, cap: Capability, loc: &str| -> Result<()> {
        if used && !caps.contains(cap) {
            return Err(err("E_CAPABILITY", loc, cap.as_str()));
        }
        Ok(())
    };
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_effects),
        Capability::UiMenuEffects,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme
            .image_menus
            .values()
            .any(|m| m.effects.iter().any(MenuEffects::uses_transition)),
        Capability::UiMenuTransition,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme
            .image_menus
            .values()
            .any(|m| m.effects.iter().any(MenuEffects::uses_element_tween)),
        Capability::UiMenuElementTween,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme.menu_overlay.is_some() || theme.image_menus.values().any(ImageMenu::uses_services),
        Capability::UiMenuServices,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_replay),
        Capability::UiReplay,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(|m| !m.builtin_navigation),
        Capability::UiMenuChrome,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_text_buttons),
        Capability::UiMenuTextButton,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_stack),
        Capability::UiMenuStack,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_story),
        Capability::UiMenuStory,
        "theme.image_menus",
    )?;
    for menu in theme.image_menus.values() {
        menu.validate_story_exports(variables)?;
    }
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_reading),
        Capability::UiMenuReading,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_storage),
        Capability::UiMenuStorage,
        "requires",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_navigation),
        Capability::UiMenuNavigation,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme
            .image_menus
            .values()
            .any(ImageMenu::uses_history_scrollbar),
        Capability::UiMenuHistoryScrollbar,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_history_flow),
        Capability::UiMenuHistoryFlow,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme
            .image_menus
            .values()
            .any(ImageMenu::uses_history_availability),
        Capability::UiMenuHistoryAvailability,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme
            .image_menus
            .values()
            .any(ImageMenu::uses_history_voice),
        Capability::UiMenuHistoryVoice,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_history),
        Capability::UiMenuHistory,
        "requires",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_values),
        Capability::UiMenuValues,
        "requires",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(ImageMenu::uses_state),
        Capability::UiMenuState,
        "theme.image_menus",
    )?;
    require_menu_cap(
        theme.image_menus.values().any(|m| !m.elements.is_empty()),
        Capability::UiMenuElements,
        "theme.image_menus",
    )?;
    validate_dialogue_styles(theme, requires)?;
    require_menu_cap(
        theme.dialogue.text_rect.is_some(),
        Capability::TextRect,
        "theme.dialogue.text_rect",
    )?;
    require_menu_cap(
        theme.dialogue.shadow.is_some(),
        Capability::TextShadow,
        "theme.dialogue.shadow",
    )?;
    require_menu_cap(
        player.auto_delay_policy != AutoDelayPolicy::LengthScaled,
        Capability::PlayerAutoDelayPolicy,
        "player.auto_delay_policy",
    )?;
    require_menu_cap(
        player.hide_policy != HidePolicy::ContinueStory,
        Capability::PlayerHidePolicy,
        "player.hide_policy",
    )?;
    Ok(())
}
pub(super) fn validate_image_menus(
    theme: &Theme,
    asset: impl Fn(&str) -> Option<AssetKind>,
    function: impl Fn(&str) -> Option<FunctionSignature>,
) -> Result<()> {
    for id in theme.image_assets() {
        if asset(&id) != Some(AssetKind::Image) {
            return Err(err("E_THEME_ASSET", &id, "theme requires an image asset"));
        }
    }
    for menu in theme.image_menus.values() {
        for id in menu.effect_assets() {
            if asset(&id) != Some(AssetKind::Audio) {
                return Err(err(
                    "E_THEME_ASSET",
                    &id,
                    "menu effects require an audio asset",
                ));
            }
        }
    }
    for menu in theme.image_menus.values() {
        for (_, action, _) in menu.controls() {
            if let ImageMenuAction::Entry { function: id }
            | ImageMenuAction::Replay { function: id } = action
            {
                if function(id).is_none_or(|f| !f.params.is_empty() || f.returns.is_some()) {
                    return Err(err(
                        "E_THEME_ENTRY",
                        id,
                        "menu entry must name a function without arguments or return value",
                    ));
                }
            }
        }
    }
    Ok(())
}
pub(super) fn validate_runtime_root(root: &RuntimeProgram) -> Result<()> {
    if root
        .title_nodes
        .iter()
        .any(|n| n.timeline_binding.is_some())
    {
        return Err(err(
            "E_TIMELINE",
            "title_nodes",
            "title animation requires a separate host capability",
        ));
    }
    if !root.timeline_owners.is_empty()
        && !root
            .requires
            .iter()
            .any(|c| c == "stage.sprite-timeline.v1")
    {
        return Err(err(
            "E_CAPABILITY",
            "timeline_owners",
            "stage.sprite-timeline.v1",
        ));
    }
    let root_caps = CapabilitySet::from_names_known(root.requires.iter().map(String::as_str));
    if root.variables.values().any(|v| v.ty() == ValueType::F80)
        && !root_caps.contains(Capability::StoryFloat80)
    {
        return Err(err("E_CAPABILITY", "variables", "story.float80.v1"));
    }
    validate_theme_and_player(
        &root.theme,
        &root.player,
        &root.variables,
        &root.requires,
        root_caps,
        |_| None,
    )?;
    validate_image_menus(
        &root.theme,
        |id| root.assets.get(id).map(|a| a.kind),
        |id| root.function_signature(id).cloned(),
    )?;
    let fail = || bad_runtime("program", "invalid runtime root index or configuration");
    if root.format != RUNTIME_FORMAT_VERSION
        || root.game_id.is_empty()
        || root.function_index.len() > 4096
        || root.text_contracts.len() > 100_000
        || root.assets.len() > 10_000
        || root.stage.width == 0
        || root.stage.height == 0
        || root.stage.width > 8192
        || root.stage.height > 8192
        || !root.locales.contains(&root.default_locale)
        || root.locales.is_empty()
        || root
            .locales
            .iter()
            .any(|locale| locale != "zh-Hans" && locale != "en" && locale != "ja")
    {
        return Err(fail());
    }
    CapabilitySet::from_names(root.requires.iter().map(String::as_str))?;
    let entry = root.function_signature(&root.entry).ok_or_else(fail)?;
    if !entry.params.is_empty() {
        return Err(err("E_CALL", "entry", "entry requires arguments"));
    }
    if root.modules.len() > 4096 {
        return Err(err("E_LIMIT", "modules", "too many modules"));
    }
    for (id, module) in &root.modules {
        if id.is_empty()
            || module.functions.is_empty()
            || !valid_hash(&module.code)
            || !valid_hash(&module.static_content)
            || module
                .locales
                .keys()
                .any(|locale| !root.locales.contains(locale))
            || module.locales.values().any(|digest| !valid_hash(digest))
            || (!module.texts.is_empty()
                && module.locales.keys().cloned().collect::<BTreeSet<_>>() != root.locales)
        {
            return Err(err("E_MODULE", id, "invalid module root index"));
        }
        for (function, signature) in &module.functions {
            let Some(index) = root.function_index.get(function) else {
                return Err(err("E_MODULE", function, "function index missing"));
            };
            if index.module != *id
                || index.signature != *signature
                || signature.entry.is_empty()
                || signature.entry_op.is_empty()
            {
                return Err(err(
                    "E_MODULE",
                    function,
                    "function interface ownership mismatch",
                ));
            }
        }
        let text_ids: BTreeSet<_> = root
            .text_owners
            .iter()
            .filter(|(_, owner)| *owner == id)
            .map(|(text, _)| text)
            .collect();
        if text_ids != module.texts.iter().collect() {
            return Err(err("E_MODULE", id, "text ownership index mismatch"));
        }
    }
    if root.function_index.iter().any(|(id, index)| {
        root.modules
            .get(&index.module)
            .and_then(|module| module.functions.get(id))
            != Some(&index.signature)
    }) {
        return Err(err(
            "E_MODULE",
            "function_index",
            "orphan or mismatched function",
        ));
    }
    for (module, index) in &root.modules {
        let scope = root.module_execution_scope(module).ok_or_else(fail)?;
        if index
            .functions
            .keys()
            .any(|id| root.function_execution_module(id) != Some(scope))
        {
            return Err(err("E_MODULE", module, "mixed execution scopes"));
        }
        if scope != module {
            if !root
                .requires
                .iter()
                .any(|cap| cap == "story.code-packages.v1")
            {
                return Err(err("E_CAPABILITY", module, "story.code-packages.v1"));
            }
            if !root.modules.contains_key(scope)
                || root.module_execution_scope(scope) != Some(scope)
                || root.modules[scope]
                    .functions
                    .keys()
                    .any(|id| root.function_index[id].execution_module.is_some())
                || !index.texts.is_empty()
                || [
                    &root.scene_owners,
                    &root.timeline_owners,
                    &root.cue_owners,
                    &root.choice_owners,
                    &root.text_owners,
                    &root.task_owners,
                ]
                .into_iter()
                .any(|owners| owners.values().any(|owner| owner == module))
            {
                return Err(err(
                    "E_MODULE",
                    module,
                    "invalid code package declaration scope",
                ));
            }
        } else if index
            .functions
            .keys()
            .any(|id| root.function_index[id].execution_module.is_some())
        {
            return Err(err("E_MODULE", module, "redundant execution scope"));
        }
    }
    let owner_maps = [
        &root.scene_owners,
        &root.timeline_owners,
        &root.cue_owners,
        &root.choice_owners,
        &root.text_owners,
    ];
    if owner_maps
        .into_iter()
        .flat_map(|owners| owners.values())
        .any(|module| !root.modules.contains_key(module))
        || root
            .task_owners
            .values()
            .any(|module| !root.modules.contains_key(module))
        || root.text_contracts.iter().any(|(id, identity)| {
            root.text_owners.get(id) != Some(&identity.module)
                || !root.modules.contains_key(&identity.module)
                || identity.source_revision == 0
                || identity.contract_revision == 0
                || identity.meaning_revision == 0
                || !valid_hash(&identity.contract_digest)
                || identity.images.len() > 256
                || (!identity.images.is_empty()
                    && !root.requires.iter().any(|c| c == "text.inline-image.v1"))
                || identity.images.iter().any(|i| {
                    i.id.is_empty()
                        || root
                            .assets
                            .get(&i.asset)
                            .is_none_or(|a| a.kind != AssetKind::Image)
                })
                || identity
                    .images
                    .iter()
                    .map(|i| &i.id)
                    .collect::<BTreeSet<_>>()
                    .len()
                    != identity.images.len()
        })
        || root
            .text_owners
            .keys()
            .any(|id| !root.text_contracts.contains_key(id))
    {
        return Err(err(
            "E_MODULE",
            "ownership",
            "invalid runtime ownership index",
        ));
    }
    let hash_map: BTreeMap<_, _> = root
        .assets
        .iter()
        .map(|(id, asset)| (id.clone(), asset.object.clone()))
        .collect();
    if root.locale_config.default_ui != "zh-Hans" && root.locale_config.default_ui != "en"
        || root.locale_config.default_text != root.default_locale
        || root
            .locale_config
            .text
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            != root.locales
        || !root
            .locale_config
            .ui
            .contains_key(&root.locale_config.default_ui)
        || root.locale_config.ui.is_empty()
        || root
            .locale_config
            .ui
            .keys()
            .any(|locale| locale != "zh-Hans" && locale != "en")
    {
        return Err(err(
            "E_LOCALE_CONFIG",
            "program.locale_config",
            "invalid runtime locale set",
        ));
    }
    for (surface, plans) in [
        ("ui", &root.locale_config.ui),
        ("text", &root.locale_config.text),
    ] {
        for (locale, plan) in plans {
            if plan.fonts.is_empty()
                || plan.digest != LocaleFontPlan::digest_for(&plan.fonts, &hash_map)
                || plan.fonts.iter().collect::<BTreeSet<_>>().len() != plan.fonts.len()
                || plan
                    .fonts
                    .iter()
                    .any(|font| root.assets.get(font).map(|a| a.kind) != Some(AssetKind::Font))
            {
                return Err(err(
                    "E_FONT_PLAN",
                    &format!("{surface}.{locale}"),
                    "invalid font plan index",
                ));
            }
        }
    }
    for (catalog, digest) in &root.catalogs {
        if catalog.is_empty() || !valid_hash(digest) {
            return Err(err("E_CATALOG", catalog, "invalid catalog identity"));
        }
    }
    for (id, asset) in &root.assets {
        if id.is_empty()
            || !valid_hash(&asset.object)
            || !root.catalogs.contains_key(&asset.catalog)
        {
            return Err(err(
                "E_ASSET",
                id,
                "asset index references an unknown object or catalog",
            ));
        }
    }
    for (task, owner) in &root.task_owners {
        if task.is_empty() || owner.is_empty() {
            return Err(err("E_TASK", task, "invalid task owner"));
        }
    }
    if let Some(title) = &root.title_scene {
        if !root.scene_owners.contains_key(title) || root.title_nodes.len() > MAX_NODES {
            return Err(err("E_SCENE", title, "invalid root title scene"));
        }
        validate_scene_nodes(&root.title_nodes, title, root)?;
    } else if !root.title_nodes.is_empty() {
        return Err(err(
            "E_SCENE",
            "title_nodes",
            "title nodes without a title scene",
        ));
    }
    Ok(())
}

pub(super) fn validate_object_key(
    root: &RuntimeProgram,
    key: &ContentKey,
    object: &RuntimeObject,
) -> Result<()> {
    let fail = || {
        err(
            "E_CONTENT_KEY",
            &format!("{key:?}"),
            "package does not match typed root identity",
        )
    };
    match (key, object) {
        (ContentKey::Static { module }, RuntimeObject::Static(package)) => {
            let Some(index) = root.modules.get(module) else {
                return Err(fail());
            };
            if package.format != RUNTIME_FORMAT_VERSION
                || package.module != *module
                || package
                    .scenes
                    .keys()
                    .any(|id| root.scene_owners.get(id) != Some(module))
                || package
                    .cues
                    .keys()
                    .any(|id| root.cue_owners.get(id) != Some(module))
                || package
                    .choices
                    .keys()
                    .any(|id| root.choice_owners.get(id) != Some(module))
                || package
                    .text_contracts
                    .keys()
                    .any(|id| root.text_owners.get(id) != Some(module))
                || package
                    .sprite_timelines
                    .keys()
                    .any(|id| root.timeline_owners.get(id) != Some(module))
                || package.sprite_timelines.len()
                    != root
                        .timeline_owners
                        .values()
                        .filter(|owner| *owner == module)
                        .count()
                || package.scenes.len()
                    != root
                        .scene_owners
                        .values()
                        .filter(|owner| *owner == module)
                        .count()
                || package.cues.len()
                    != root
                        .cue_owners
                        .values()
                        .filter(|owner| *owner == module)
                        .count()
                || package.choices.len()
                    != root
                        .choice_owners
                        .values()
                        .filter(|owner| *owner == module)
                        .count()
                || package
                    .text_contracts
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    != index.texts
            {
                return Err(fail());
            }
            for cue in package.cues.values() {
                for effect in &cue.effects {
                    if root.task_owners.get(&effect.id) != Some(module) {
                        return Err(err(
                            "E_TASK",
                            &effect.id,
                            "task ownership differs from root",
                        ));
                    }
                }
            }
            Ok(())
        }
        (ContentKey::Code { module }, RuntimeObject::Code(package)) => {
            let Some(index) = root.modules.get(module) else {
                return Err(fail());
            };
            if package.format != RUNTIME_FORMAT_VERSION
                || package.module != *module
                || package.functions.len() != index.functions.len()
                || package.functions.iter().any(|(id, function)| {
                    index.functions.get(id) != Some(&FunctionSignature::from(function))
                        || root.function_module(id) != Some(module.as_str())
                })
            {
                return Err(fail());
            }
            Ok(())
        }
        (ContentKey::Text { module, locale }, RuntimeObject::Text(package)) => {
            let Some(index) = root.modules.get(module) else {
                return Err(fail());
            };
            if package.format != RUNTIME_FORMAT_VERSION
                || package.module != *module
                || package.locale != *locale
                || !root.locales.contains(locale)
                || package.texts.keys().cloned().collect::<BTreeSet<_>>() != index.texts
            {
                return Err(fail());
            }
            Ok(())
        }
        (ContentKey::Catalog { catalog }, RuntimeObject::Catalog(package)) => {
            let expected: BTreeSet<_> = root
                .assets
                .iter()
                .filter(|(_, a)| a.catalog == *catalog)
                .map(|(id, _)| id.clone())
                .collect();
            if package.format != RUNTIME_FORMAT_VERSION
                || package.catalog != *catalog
                || package.assets.keys().cloned().collect::<BTreeSet<_>>() != expected
                || package.assets.iter().any(|(id, asset)| {
                    root.assets
                        .get(id)
                        .is_none_or(|i| i.kind != asset.kind || i.object != asset.object)
                })
            {
                return Err(fail());
            }
            Ok(())
        }
        _ => Err(fail()),
    }
}

pub(super) fn validate_runtime_objects(
    view: &RuntimeProgramView,
    objects: &[RuntimeObject],
) -> Result<()> {
    for object in objects {
        match object {
            RuntimeObject::Static(package) => validate_static_package(view, package)?,
            RuntimeObject::Code(package) => {
                for (id, function) in &package.functions {
                    validate_runtime_function(view, id, function)?;
                }
            }
            RuntimeObject::Text(package) => {
                let contracts = &view.texts;
                for (id, document) in &package.texts {
                    let contract = contracts.get(id).ok_or_else(|| {
                        err(
                            "E_TEXT_CONTRACT",
                            id,
                            "static text contract is not resident",
                        )
                    })?;
                    if document.source_revision != contract.source_revision
                        || document.contract_revision != contract.contract_revision
                        || document.contract_digest != contract.contract_digest
                    {
                        return Err(err("E_TEXT_REVISION", id, &package.locale));
                    }
                    validate_text_spans(id, contract, &document.spans)?;
                    if !contract.images.is_empty()
                        && (!view.requires.iter().any(|c| c == "text.inline-image.v1")
                            || contract
                                .images
                                .iter()
                                .any(|i| view.asset_kind(&i.asset) != Some(AssetKind::Image)))
                    {
                        return Err(err("E_TEXT_IMAGE", id, "capability or image binding"));
                    }
                    if document
                        .spans
                        .iter()
                        .any(|s| matches!(s, Span::Pause { .. }))
                        && !view.requires.iter().any(|c| c == "text.pause.v1")
                    {
                        return Err(err("E_CAPABILITY", id, "text.pause.v1"));
                    }
                    if document
                        .spans
                        .iter()
                        .any(|s| matches!(s, Span::Ruby { .. }))
                        && !view.requires.iter().any(|c| c == "text.ruby.v1")
                    {
                        return Err(err("E_CAPABILITY", id, "text.ruby.v1"));
                    }
                }
            }
            RuntimeObject::Catalog(package) => {
                // A static body can be prefetched before its media catalog.
                // Complete interval validation as soon as duration is available,
                // including declarations resident before this catalog arrived.
                for (id, cue) in view.cues.iter() {
                    for definition in &cue.effects {
                        validate_effect_extensions(
                            &definition.effect,
                            &view.requires,
                            &view.theme,
                            &|asset| view.asset(asset).map(|a| a.duration_us.0),
                            id,
                        )?;
                        if let Effect::DialogueDecoration {
                            image: Some(image), ..
                        } = &definition.effect
                        {
                            if let Some(asset) = package.assets.get(&image.asset) {
                                if asset.kind != AssetKind::Image {
                                    return Err(err("E_ASSET_TYPE", id, &image.asset));
                                }
                                if [asset.width, asset.height] != image.size {
                                    return Err(err(
                                        "E_VISUAL",
                                        id,
                                        "dialogue decoration size must match image asset dimensions",
                                    ));
                                }
                            }
                        }
                    }
                }
                validate_menu_audio_loops(&view.theme, view.capabilities, |asset| {
                    package.assets.get(asset).map(|a| a.duration_us.0)
                })?;
                for (id, asset) in &package.assets {
                    if asset.bytes > MAX_INPUT_BYTES as u64 * 16
                        || asset.width > 8192
                        || asset.height > 8192
                        || (asset.kind == AssetKind::Image
                            && (asset.width == 0 || asset.height == 0))
                        || (asset.kind == AssetKind::Image
                            && asset.decoded_bytes != asset.width as u64 * asset.height as u64 * 4)
                        || (asset.kind == AssetKind::Audio
                            && (asset.duration_us.0 == 0 || asset.decoded_bytes == 0))
                        || (asset.kind == AssetKind::Font && asset.decoded_bytes == 0)
                    {
                        return Err(err("E_ASSET", id, "invalid asset descriptor"));
                    }
                }
            }
        }
    }
    Ok(())
}
