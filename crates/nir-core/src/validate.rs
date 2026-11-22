use nir_format::*;
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::Index;
use std::sync::{Arc, Mutex};

mod cue;
mod function;
mod residency;
mod root;
#[cfg(test)]
mod runtime_tests;
mod scene;

use cue::*;
pub use function::expr_type;
use function::{outgoing, validate_assignments, validate_runtime_function};
#[cfg(test)]
use function::{reads, term_exprs};
pub(crate) use residency::Instruction;
pub use residency::{
    ContentAdmission, ContentLease, ContentTable, ResidencyEntry, ResidencyReport,
    RuntimeProgramView, ValidatedProgram,
};
use root::*;
use scene::{validate_bitmap_node, validate_scene_nodes, validate_static_package};
pub(crate) use scene::{validate_timeline, validate_timeline_bindings};

fn validate(p: &RuntimeProgramView) -> Result<()> {
    if (p.variables.values().any(|v| v.ty() == ValueType::F80)
        || p.functions.values().any(Function::uses_float80)
        || p.choices.values().any(Choice::uses_float80))
        && !p.capabilities.contains(Capability::StoryFloat80)
    {
        return Err(err("E_CAPABILITY", "program", "story.float80.v1"));
    }
    if let Some(root) = p.runtime_root() {
        return validate_runtime_root(root);
    }
    validate_theme_and_player(
        &p.theme,
        &p.player,
        &p.variables,
        &p.requires,
        p.capabilities,
        |asset| p.assets.get(asset).map(|a| a.duration_us.0),
    )?;
    validate_image_menus(
        &p.theme,
        |id| p.asset_kind(id),
        |id| p.function_signature(id),
    )?;
    if p.format != FORMAT_VERSION {
        return Err(err("E_VERSION", "program", "unsupported semantic version"));
    }
    if p.functions.len() > 4096 || p.texts.len() > 100_000 || p.assets.len() > 10_000 {
        return Err(err("E_LIMIT", "program", "table limit"));
    }
    CapabilitySet::from_names(p.requires.iter().map(String::as_str))?;
    if p.game_id.is_empty()
        || p.function_signature(&p.entry).is_none()
        || p.stage.width == 0
        || p.stage.height == 0
        || p.stage.width > 8192
        || p.stage.height > 8192
    {
        return Err(err("E_PROGRAM", "program", "invalid game, entry or stage"));
    }
    if !p.function_signature(&p.entry).unwrap().params.is_empty() {
        return Err(err("E_CALL", "entry", "entry requires arguments"));
    }
    if !p.locales.contains_key(&p.default_locale) {
        return Err(err("E_LOCALE", "program", "missing default locale"));
    }
    let config = &p.locale_config;
    let asset_objects: BTreeMap<_, _> = p
        .asset_index
        .iter()
        .map(|(id, asset)| (id.clone(), asset.object.clone()))
        .collect();
    if config.default_ui != "zh-Hans" && config.default_ui != "en"
        || config.default_text != p.default_locale
        || !config.ui.contains_key(&config.default_ui)
        || !config.text.contains_key(&config.default_text)
        || config.ui.is_empty()
        || config.text.keys().collect::<BTreeSet<_>>() != p.locales.keys().collect::<BTreeSet<_>>()
        || config
            .ui
            .keys()
            .any(|locale| locale != "zh-Hans" && locale != "en")
    {
        return Err(err(
            "E_LOCALE_CONFIG",
            "program.locale_config",
            "invalid defaults or supported locale sets",
        ));
    }
    for (surface, plans) in [("ui", &config.ui), ("text", &config.text)] {
        for (locale, plan) in plans {
            if plan.fonts.is_empty()
                || plan.digest != LocaleFontPlan::digest_for(&plan.fonts, &asset_objects)
                || plan.fonts.iter().collect::<BTreeSet<_>>().len() != plan.fonts.len()
            {
                return Err(err(
                    "E_FONT_PLAN",
                    locale,
                    "font plans must be nonempty, unique and have a valid digest",
                ));
            }
            for font in &plan.fonts {
                if p.assets.get(font).map(|asset| asset.kind) != Some(AssetKind::Font) {
                    return Err(err(
                        "E_FONT_PLAN",
                        &format!("{surface}.{locale}"),
                        &format!("unknown font asset {font}"),
                    ));
                }
            }
        }
    }
    let mut declared_functions = BTreeSet::new();
    let mut declared_texts = BTreeSet::new();
    if p.modules.len() > 4096 {
        return Err(err("E_LIMIT", "modules", "too many modules"));
    }
    for (id, module) in p.modules.iter() {
        if id.is_empty() || module.functions.is_empty() {
            return Err(err("E_MODULE", id, "empty module identity or interface"));
        }
        for (fid, signature) in &module.functions {
            if !declared_functions.insert(fid)
                || signature.entry.is_empty()
                || signature.entry_op.is_empty()
                || p.functions
                    .get(fid)
                    .is_some_and(|f| FunctionSignature::from(f) != *signature)
            {
                return Err(err(
                    "E_MODULE",
                    fid,
                    "duplicate or mismatched function interface",
                ));
            }
        }
        for tid in &module.texts {
            if !declared_texts.insert(tid) || !p.texts.contains_key(tid) {
                return Err(err("E_MODULE", tid, "duplicate or unknown text ownership"));
            }
        }
    }
    if !p.modules.is_empty()
        && (p
            .functions
            .keys()
            .any(|id| !declared_functions.contains(id))
            || p.texts.keys().any(|id| !declared_texts.contains(id))
            || declared_functions.len() > 4096)
    {
        return Err(err(
            "E_MODULE",
            "modules",
            "incomplete ownership or function limit",
        ));
    }
    for (locale, texts) in p.locales.iter() {
        if locale != "zh-Hans" && locale != "en" && locale != "ja" {
            return Err(err(
                "E_CAPABILITY",
                locale,
                "locale is not in the tested language profile",
            ));
        }
        for (id, c) in p.texts.iter() {
            if c.source_revision == 0
                || c.contract_revision == 0
                || c.meaning_revision == 0
                || c.contract_digest != text_contract_digest(c)
            {
                return Err(err("E_TEXT_REVISION", id, locale));
            }
            for (param, ty) in &c.params {
                if p.variables.get(param).map(Value::ty) != Some(*ty) {
                    return Err(err("E_TEXT_PARAM", id, param));
                }
            }
            let Some(d) = texts.get(id) else {
                if p.text_module(id).is_some() {
                    continue;
                }
                return Err(err("E_TRANSLATION", locale, id));
            };
            if d.source_revision != c.source_revision
                || d.contract_revision != c.contract_revision
                || d.contract_digest != c.contract_digest
            {
                return Err(err("E_TEXT_REVISION", id, locale));
            }
            validate_text_spans(id, c, &d.spans)?;
            if !c.images.is_empty()
                && (!p.requires.iter().any(|c| c == "text.inline-image.v1")
                    || c.images.iter().any(|i| {
                        p.assets
                            .get(&i.asset)
                            .is_none_or(|a| a.kind != AssetKind::Image)
                    }))
            {
                return Err(err("E_TEXT_IMAGE", id, "capability or image binding"));
            }
            if d.spans.iter().any(|s| matches!(s, Span::Pause { .. }))
                && !p.requires.iter().any(|c| c == "text.pause.v1")
            {
                return Err(err("E_CAPABILITY", id, "text.pause.v1"));
            }
            if d.spans.iter().any(|s| matches!(s, Span::Ruby { .. }))
                && !p.requires.iter().any(|c| c == "text.ruby.v1")
            {
                return Err(err("E_CAPABILITY", id, "text.ruby.v1"));
            }
        }
        if texts.keys().any(|id| !p.texts.contains_key(id)) {
            return Err(err("E_TEXT_CONTRACT", locale, "unexpected text"));
        }
    }
    for (id, a) in p.assets.iter() {
        if a.bytes > MAX_INPUT_BYTES as u64 * 16 || a.width > 8192 || a.height > 8192 {
            return Err(err("E_LIMIT", id, "asset too large"));
        }
    }
    for (id, timeline) in p.sprite_timelines.iter() {
        validate_timeline(id, timeline, &p.requires)?;
    }
    for (id, nodes) in p.scenes.iter() {
        validate_timeline_bindings(nodes, id, |key| p.sprite_timelines.get(key))?;
        let err = |code: &str, at: &str, message: &str| {
            err(code, at, message).classified(
                ErrorDomain::Content,
                "scenes",
                "validate",
                vec![Recovery::FixContent],
            )
        };
        if nodes.len() > MAX_NODES {
            return Err(err("E_LIMIT", id, "too many nodes"));
        }
        let mut ids = BTreeSet::new();
        for n in nodes {
            if n.sprite_transform.is_some()
                && nodes
                    .iter()
                    .any(|child| child.parent.as_deref() == Some(n.id.as_str()))
            {
                return Err(err("E_VISUAL", id, "sprite transforms require leaf nodes"));
            }
            validate_bitmap_node(n, id, &p.variables, &p.requires, p.title_scene.as_deref())?;
            if !ids.insert(&n.id) {
                return Err(err("E_DUPLICATE", id, &n.id));
            }
            if ![n.x, n.y, n.width, n.height, n.scale, n.opacity]
                .iter()
                .chain(n.color.iter())
                .all(|v| v.is_finite())
                || n.width < 0.
                || n.height < 0.
                || n.scale < 0.
                || !(0.0..=1.0).contains(&n.opacity)
                || n.clip.is_some_and(|clip| {
                    !clip.iter().all(|v| v.is_finite()) || clip[2] < 0. || clip[3] < 0.
                })
            {
                return Err(err("E_VISUAL", id, &n.id));
            }
            if let Some(a) = &n.asset {
                if p.assets.get(a).map(|a| a.kind) != Some(AssetKind::Image) {
                    return Err(err("E_ASSET_TYPE", id, a));
                }
            }
        }
        for n in nodes {
            let mut seen = BTreeSet::new();
            let mut parent = n.parent.as_ref();
            while let Some(k) = parent {
                if k == &n.id || !seen.insert(k) {
                    return Err(err("E_SCENE_CYCLE", id, k));
                }
                parent = nodes
                    .iter()
                    .find(|v| &v.id == k)
                    .ok_or_else(|| err("E_NODE", id, k))?
                    .parent
                    .as_ref();
            }
        }
    }
    let mut task_defs: BTreeMap<&str, Vec<&Effect>> = BTreeMap::new();
    for (id, cue) in p.cues.iter() {
        let err = |code: &str, at: &str, message: &str| {
            err(code, at, message).classified(
                ErrorDomain::Content,
                "cues",
                "validate",
                vec![Recovery::FixContent],
            )
        };
        if cue.effects.is_empty() || cue.effects.len() > MAX_TASKS {
            return Err(err("E_LIMIT", id, "invalid cue size"));
        }
        let modal_count = cue
            .effects
            .iter()
            .filter(|def| matches!(def.effect, Effect::StoryModal { .. }))
            .count();
        if modal_count > 1
            || (modal_count > 0
                && cue
                    .effects
                    .iter()
                    .any(|def| matches!(def.effect, Effect::Dialogue { .. })))
        {
            return Err(err(
                "E_OWNERSHIP",
                id,
                "story modal requires exclusive interaction",
            ));
        }
        let style_count = cue
            .effects
            .iter()
            .filter(|def| matches!(def.effect, Effect::DialogueStyle { .. }))
            .count();
        if style_count > 1
            || (style_count > 0
                && cue
                    .effects
                    .iter()
                    .any(|def| matches!(def.effect, Effect::Dialogue { .. })))
        {
            return Err(err(
                "E_OWNERSHIP",
                id,
                "style replacement must own the dialogue window",
            ));
        }
        let mut decorations = BTreeSet::new();
        for def in &cue.effects {
            if let Effect::DialogueDecoration { slot, .. } = &def.effect {
                if !decorations.insert(slot) || style_count > 0 {
                    return Err(err(
                        "E_OWNERSHIP",
                        id,
                        "conflicting dialogue decoration writers",
                    ));
                }
            }
        }
        let mut names = BTreeSet::new();
        let mut writers = BTreeSet::new();
        let mut stage_count = 0;
        let mut dialogue_count = 0;
        for def in &cue.effects {
            validate_effect_extensions(
                &def.effect,
                &p.requires,
                &p.theme,
                &|asset| p.assets.get(asset).map(|a| a.duration_us.0),
                id,
            )?;
            if !names.insert(&def.id) {
                return Err(err("E_DUPLICATE", id, &def.id));
            }
            task_defs.entry(&def.id).or_default().push(&def.effect);
            if matches!(def.effect, Effect::Tween { .. })
                && !p.requires.iter().any(|c| c == "tween.target.v1")
            {
                return Err(err("E_CAPABILITY", id, "tween.target.v1"));
            }
            if let Some((address, track, _)) = def.effect.scalar_track(0., 0.) {
                if !address.accepts(track.to) {
                    return Err(err("E_VISUAL", id, "invalid target value"));
                }
                if let TweenTarget::AudioInstance { task, .. } = &address {
                    if !p.requires.iter().any(|c| c == "audio.gain-tween.v1") {
                        return Err(err("E_CAPABILITY", id, "audio.gain-tween.v1"));
                    }
                    if !matches!(track.easing, Easing::Linear) {
                        return Err(err("E_AUDIO_STOP", id, "envelope tweens are linear"));
                    }
                    if task == &def.id
                        || !p.task_definitions.get(task).is_some_and(|defs| {
                            !defs.is_empty()
                                && defs
                                    .iter()
                                    .all(|effect| matches!(effect.as_ref(), Effect::Audio { .. }))
                        })
                    {
                        return Err(err("E_AUDIO_STOP", id, task));
                    }
                }
                if !writers.insert(address) {
                    return Err(err("E_OWNERSHIP", id, "multiple property writers"));
                }
            }
            match &def.effect {
                Effect::SpriteTimeline {
                    timeline,
                    duration_us,
                    ..
                } => {
                    if p.sprite_timelines
                        .get(timeline)
                        .is_none_or(|t| t.duration_us != *duration_us)
                    {
                        return Err(err(
                            "E_TIMELINE",
                            id,
                            "timeline identity or duration mismatch",
                        ));
                    }
                }
                Effect::StagePresent {
                    scene,
                    transition,
                    inherit_images,
                    inherit_image_geometry,
                    ..
                } => {
                    if !transition.valid() {
                        return Err(err("E_TRANSITION", id, "invalid wipe softness"));
                    }
                    if let Some(cap) = transition.capability() {
                        if !p.requires.iter().any(|c| c == cap) {
                            return Err(err("E_CAPABILITY", id, cap));
                        }
                    }
                    if let Some(asset) = transition.asset() {
                        if p.asset_kind(asset) != Some(AssetKind::Image) {
                            return Err(err("E_ASSET", id, "mask must reference an image"));
                        }
                    }
                    stage_count += 1;
                    if !p.scenes.contains_key(scene) {
                        return Err(err("E_SCENE", id, scene));
                    }
                    validate_image_inheritance(
                        &p.scenes[scene],
                        inherit_images,
                        inherit_image_geometry,
                        id,
                    )?;
                }
                Effect::Dialogue { text, speaker, .. } => {
                    dialogue_count += 1;
                    if !p.texts.contains_key(text)
                        || (!speaker.is_empty() && !p.texts.contains_key(speaker))
                    {
                        return Err(err("E_TEXT", id, text));
                    }
                }
                Effect::AudioStop {
                    target,
                    duration_us,
                } => {
                    if !p.requires.iter().any(|cap| cap == "audio.stop.v1") {
                        return Err(err("E_CAPABILITY", id, "audio.stop.v1"));
                    }
                    if duration_us.0 > 60_000_000
                        || target == &def.id
                        || !p.task_definitions.get(target).is_some_and(|defs| {
                            !defs.is_empty()
                                && defs
                                    .iter()
                                    .all(|effect| matches!(effect.as_ref(), Effect::Audio { .. }))
                        })
                    {
                        return Err(err(
                            "E_AUDIO_STOP",
                            id,
                            "stop requires an audio target and duration within 0..60s",
                        ));
                    }
                }
                Effect::Audio { gain, .. }
                    if *gain != 1.0 && !p.requires.iter().any(|c| c == "audio.gain.v1") =>
                {
                    return Err(err("E_CAPABILITY", id, "audio.gain.v1"));
                }
                Effect::Audio { gain, .. } if !valid_audio_gain(*gain) => {
                    return Err(err(
                        "E_AUDIO_GAIN",
                        id,
                        "event gain must be finite and within 0..4",
                    ));
                }
                Effect::DialogueDecoration {
                    image: Some(image), ..
                } if p.assets.get(&image.asset).is_none_or(|asset| {
                    asset.kind != AssetKind::Image || [asset.width, asset.height] != image.size
                }) =>
                {
                    return Err(err("E_DIALOGUE_DECORATION", id, "image metadata mismatch"));
                }
                Effect::Audio { asset, .. }
                    if p.assets.get(asset).map(|a| a.kind) != Some(AssetKind::Audio) =>
                {
                    return Err(err("E_ASSET_TYPE", id, asset));
                }
                _ => {}
            }
        }
        if stage_count > 1 || dialogue_count > 1 {
            return Err(err("E_CUE", id, "at most one stage and dialogue per cue"));
        }
        validate_composition(
            id,
            cue,
            &p.requires,
            &mut names,
            |asset| p.assets.get(asset).map(|a| a.kind),
            |target| {
                p.task_definitions.get(target).is_some_and(|defs| {
                    !defs.is_empty()
                        && defs
                            .iter()
                            .all(|effect| matches!(effect.as_ref(), Effect::Audio { .. }))
                })
            },
            err,
        )?;
    }
    for (id, c) in p.choices.iter() {
        validate_choice_images(id, c, &p.requires, |asset| {
            p.assets.get(asset).map(|a| a.kind)
        })?;
        let mut ids = BTreeSet::new();
        for o in &c.options {
            if !ids.insert(&o.id) {
                return Err(err("E_DUPLICATE", id, &o.id));
            }
            if !p.texts.contains_key(&o.text) {
                return Err(err("E_TEXT", id, &o.text));
            }
            let vars = p
                .variables
                .iter()
                .map(|(k, v)| (k.clone(), v.ty()))
                .collect();
            for e in [&o.visible, &o.enabled].into_iter().flatten() {
                if expr_type(e, &vars, id)? != ValueType::Bool {
                    return Err(err("E_TYPE", id, "choice predicate must be Bool"));
                }
            }
        }
        if c.timeout_us.is_some() && !c.default.as_ref().is_some_and(|d| ids.contains(d)) {
            return Err(err(
                "E_CHOICE_DEFAULT",
                id,
                "timeout requires a valid default",
            ));
        }
    }
    let mut op_ids = BTreeSet::new();
    for (fid, f) in p.functions.iter() {
        if !f.blocks.contains_key(&f.entry) || f.blocks.len() > 100_000 {
            return Err(err("E_BLOCK", fid, "invalid entry/block limit"));
        }
        let mut vars: BTreeMap<_, _> = p
            .variables
            .iter()
            .map(|(k, v)| (k.clone(), v.ty()))
            .collect();
        for (k, v) in f.params.iter().chain(f.locals.iter()) {
            if vars.insert(k.clone(), *v).is_some() {
                return Err(err("E_DUPLICATE", fid, k));
            }
        }
        for (bid, b) in &f.blocks {
            let at = format!("{fid}/{bid}");
            for next in outgoing(&b.terminator) {
                if !f.blocks.contains_key(next) {
                    return Err(err("E_BLOCK", &at, next));
                }
            }
            for op in &b.ops {
                if !op_ids.insert(&op.id) {
                    return Err(err("E_DUPLICATE", &at, &op.id));
                }
                if let Operation::DialogueVoice { task, voice, wait } = &op.operation {
                    if !p.requires.iter().any(|c| c == "text.voice-binding.v1") {
                        return Err(err("E_CAPABILITY", &op.id, "text.voice-binding.v1"));
                    }
                    if *wait == VoiceWaitPolicy::SampledRemaining {
                        if !p.requires.iter().any(|c| c == "text.voice-timer.v1") {
                            return Err(err("E_CAPABILITY", &op.id, "text.voice-timer.v1"));
                        }
                        if voice.as_ref().is_some_and(|name|task_defs.get(name.as_str()).is_some_and(|defs|defs.iter().any(|effect|
                            matches!(effect,Effect::Audio{asset,..} if p.assets.get(asset).is_none_or(|a|a.duration_us.0==0))
                        ))) { return Err(err("E_VOICE_DURATION",&op.id,"sampled voice timer requires known asset duration")); }
                    }
                    if !task_defs.get(task.as_str()).is_some_and(|defs| {
                        !defs.is_empty()
                            && defs.iter().all(|e| matches!(e, Effect::Dialogue { .. }))
                    }) || voice.as_ref().is_some_and(|name| {
                        !task_defs.get(name.as_str()).is_some_and(|defs| {
                            defs.iter().all(|e| {
                                matches!(
                                    e,
                                    Effect::Audio {
                                        bus: AudioBus::Voice,
                                        ..
                                    }
                                )
                            }) && defs
                                .iter()
                                .any(|e| matches!(e, Effect::Audio { looped: false, .. }))
                        })
                    }) {
                        return Err(err(
                            "E_VOICE_BINDING",
                            &op.id,
                            "expected dialogue and non-looping Voice task",
                        ));
                    }
                }
                if let Operation::DialogueVisibility {
                    transition: Some(style),
                    duration_us,
                    ..
                } = &op.operation
                {
                    if !p.requires.iter().any(|c| c == "text.window-transition.v1") {
                        return Err(err("E_CAPABILITY", &op.id, "text.window-transition.v1"));
                    }
                    if !style.valid() {
                        return Err(err("E_TRANSITION", &op.id, "invalid window reveal style"));
                    }
                    if duration_us.0 == 0 || duration_us.0 > 60_000_000 {
                        return Err(err(
                            "E_TIME",
                            &op.id,
                            "window reveal requires a duration within 0..60s",
                        ));
                    }
                    if style.asset().is_some_and(|asset| {
                        p.assets
                            .get(asset)
                            .is_none_or(|a| a.kind != AssetKind::Image)
                    }) {
                        return Err(err("E_ASSET", &op.id, "mask must reference an image"));
                    }
                }
                match &op.operation {
                    Operation::MenuAccess { .. }
                        if !p.requires.iter().any(|c| c == "player.menu-access.v1") =>
                    {
                        return Err(err("E_CAPABILITY", &op.id, "player.menu-access.v1"));
                    }
                    Operation::AudioPause { .. }
                        if !p.requires.iter().any(|c| c == "audio.pause.v1") =>
                    {
                        return Err(err("E_CAPABILITY", &op.id, "audio.pause.v1"));
                    }
                    Operation::ProfileValueRead { target, key }
                    | Operation::ProfileValueAssign { target, key, .. } => {
                        if !p.requires.iter().any(|c| c == "story.profile-value.v1") {
                            return Err(err("E_CAPABILITY", &op.id, "story.profile-value.v1"));
                        }
                        if !p.variables.contains_key(target) || key.is_empty() || key.len() > 1024 {
                            return Err(err(
                                "E_PROFILE",
                                &op.id,
                                "invalid profile value target/key",
                            ));
                        }
                        if let Operation::ProfileValueAssign { value, .. } = &op.operation {
                            if p.variables.get(target).map(Value::ty)
                                != Some(expr_type(value, &vars, &op.id)?)
                            {
                                return Err(err("E_TYPE", &op.id, target));
                            }
                        }
                    }
                    Operation::ProfileRead { target, key } => {
                        if !p.requires.iter().any(|c| c == "story.profile-read.v1") {
                            return Err(err("E_CAPABILITY", &op.id, "story.profile-read.v1"));
                        }
                        if p.variables.get(target).map(Value::ty) != Some(ValueType::I32)
                            || key.is_empty()
                            || key.len() > 1024
                        {
                            return Err(err(
                                "E_PROFILE",
                                &op.id,
                                "invalid profile flag target/key",
                            ));
                        }
                    }
                    Operation::Assign { target, value }
                        if vars.get(target).copied() != Some(expr_type(value, &vars, &op.id)?) =>
                    {
                        return Err(err("E_TYPE", &op.id, target));
                    }
                    Operation::Random { target, min, max }
                        if (vars.get(target) != Some(&ValueType::I32) || min > max) =>
                    {
                        return Err(err("E_RANDOM", &op.id, "invalid bounds/target"));
                    }
                    Operation::DraftPatch { value, .. } if !value.is_finite() => {
                        return Err(err("E_VISUAL", &op.id, "non-finite patch"));
                    }
                    _ => {}
                }
            }
            match &b.terminator {
                Terminator::Branch { condition, .. }
                    if expr_type(condition, &vars, &at)? != ValueType::Bool =>
                {
                    return Err(err("E_TYPE", &at, "branch requires Bool"));
                }
                Terminator::Switch { value, cases, .. } => {
                    let ty = expr_type(value, &vars, &at)?;
                    if matches!(ty, ValueType::Bool | ValueType::F80)
                        || (ty == ValueType::I32 && cases.keys().any(|s| s.parse::<i32>().is_err()))
                    {
                        return Err(err("E_TYPE", &at, "switch requires I32 or String keys"));
                    }
                }
                Terminator::Call {
                    function,
                    args,
                    result,
                    ..
                } => {
                    let callee = p
                        .function_signature(function)
                        .ok_or_else(|| err("E_FUNCTION", &at, function))?;
                    if args.len() != callee.params.len() {
                        return Err(err("E_CALL", &at, "argument count"));
                    }
                    for (k, ty) in &callee.params {
                        let e = args.get(k).ok_or_else(|| err("E_CALL", &at, k))?;
                        if expr_type(e, &vars, &at)? != *ty {
                            return Err(err("E_TYPE", &at, k));
                        }
                    }
                    if let Some(r) = result {
                        if vars.get(r).copied() != callee.returns {
                            return Err(err("E_TYPE", &at, "return target"));
                        }
                    }
                }
                Terminator::Return { value }
                    if value
                        .as_ref()
                        .map(|e| expr_type(e, &vars, &at))
                        .transpose()?
                        != f.returns =>
                {
                    return Err(err("E_TYPE", &at, "return type"));
                }
                Terminator::Activate { cue, .. } if !p.cues.contains_key(cue) => {
                    return Err(err("E_CUE", &at, cue));
                }
                Terminator::Await {
                    conditions,
                    on_advance,
                    ..
                } => {
                    if on_advance.is_some()
                        && !p.requires.iter().any(|c| c == "control.advance-wait.v1")
                    {
                        return Err(err("E_CAPABILITY", &at, "control.advance-wait.v1"));
                    }
                    if conditions.is_empty() {
                        return Err(err("E_WAIT", &at, "empty All"));
                    }
                    for c in conditions {
                        let defs = task_defs
                            .get(c.task.as_str())
                            .ok_or_else(|| err("E_TASK", &at, &c.task))?;
                        if c.milestone == Milestone::Finished
                            && defs.iter().all(|e| {
                                e.effect_tree_any(&|x| {
                                    matches!(x, Effect::Audio { looped: true, .. })
                                })
                            })
                        {
                            return Err(err("E_INFINITE_WAIT", &at, &c.task));
                        }
                        if let Milestone::Marker(g) = &c.milestone {
                            if !defs.iter().any(|e| match e {
                                Effect::Dialogue { text, .. } => p.texts[text].gates.contains(g),
                                _ => false,
                            }) {
                                return Err(err("E_MILESTONE", &at, g));
                            }
                        }
                    }
                }
                Terminator::Interact {
                    choice,
                    branches,
                    result,
                    on_cancel,
                    ..
                } => {
                    if (result.is_some() || on_cancel.is_some())
                        && !p.requires.iter().any(|c| c == "story.typed-result.v1")
                    {
                        return Err(err("E_CAPABILITY", &at, "story.typed-result.v1"));
                    }
                    let c = p
                        .choices
                        .get(choice)
                        .ok_or_else(|| err("E_CHOICE", &at, choice))?;
                    if c.options.len() != branches.len()
                        || c.options.iter().any(|o| !branches.contains_key(&o.id))
                    {
                        return Err(err("E_CHOICE", &at, "branch coverage"));
                    }
                    if let Some(target) = result {
                        let ty = p
                            .variables
                            .get(target)
                            .map(Value::ty)
                            .ok_or_else(|| err("E_VARIABLE", &at, target))?;
                        if c.options
                            .iter()
                            .any(|o| o.value.as_ref().is_none_or(|v| v.ty() != ty))
                        {
                            return Err(err("E_TYPE", &at, "typed-result option values"));
                        }
                    }
                }
                _ => {}
            }
        }
        validate_assignments(fid, f, p.variables.keys().chain(f.params.keys()))?;
    }
    Ok(())
}

fn validate_choice_images(
    id: &str,
    choice: &Choice,
    requires: &[String],
    asset_kind: impl Fn(&str) -> Option<AssetKind>,
) -> Result<()> {
    let images: Vec<_> = choice
        .options
        .iter()
        .filter_map(|o| o.image.as_ref())
        .collect();
    if images.is_empty() {
        return Ok(());
    }
    if !requires.iter().any(|c| c == "choice.image.v1") {
        return Err(err("E_CAPABILITY", id, "choice.image.v1"));
    }
    if images.len() != choice.options.len() {
        return Err(err("E_CHOICE_IMAGE", id, "mixed image and text choices"));
    }
    for image in images {
        if !image.rect.iter().all(|v| v.is_finite() && v.abs() <= 8192.)
            || image.rect[2] <= 0.
            || image.rect[3] <= 0.
        {
            return Err(err("E_CHOICE_IMAGE", id, "invalid image choice rectangle"));
        }
        if image.disabled_asset.is_some()
            && !requires.iter().any(|c| c == "choice.disabled-image.v1")
        {
            return Err(err("E_CAPABILITY", id, "choice.disabled-image.v1"));
        }
        for asset in std::iter::once(&image.asset)
            .chain(image.hover_asset.iter())
            .chain(image.disabled_asset.iter())
        {
            if asset_kind(asset) != Some(AssetKind::Image) {
                return Err(err("E_ASSET_TYPE", id, asset));
            }
        }
    }
    Ok(())
}
