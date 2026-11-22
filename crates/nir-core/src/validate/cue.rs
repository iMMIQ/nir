use super::*;

pub(super) fn err(code: &str, at: &str, message: &str) -> Diagnostic {
    Diagnostic::new(code, at, message)
}
pub(super) const MAX_CONTENT_BATCH_ITEMS: usize = 128;
/// Composition nesting cap; the task graph stays reviewable and the runtime
/// chase loop stays bounded without trusting author intent.
pub(super) const MAX_COMPOSE_DEPTH: usize = 8;

/// Every property address a def subtree writes.
pub(super) fn subtree_writers(def: &EffectDef) -> BTreeSet<TweenTarget> {
    let mut set = BTreeSet::new();
    if let Some((address, _, _)) = def.effect.scalar_track(0., 0.) {
        set.insert(address);
    }
    for child in def.effect.compose_children().unwrap_or(&[]) {
        set.extend(subtree_writers(child));
    }
    set
}

/// Writers may never overlap between effects that can run at the same
/// instant: the def against `ambient`, then recursively. Sequence positions
/// run alone, so a chain may rewrite the address an earlier position wrote;
/// parallel siblings and everything outside the chain may not.
pub(super) fn check_writer_tree(
    at: &str,
    def: &EffectDef,
    ambient: &BTreeSet<TweenTarget>,
    err: &impl Fn(&str, &str, &str) -> Diagnostic,
) -> Result<()> {
    let own = subtree_writers(def);
    if !ambient.is_disjoint(&own) {
        return Err(err(
            "E_OWNERSHIP",
            at,
            "concurrent effects write the same property",
        ));
    }
    if let Effect::ParallelAll { children } = &def.effect {
        let sets: Vec<_> = children.iter().map(subtree_writers).collect();
        for (index, child) in children.iter().enumerate() {
            let mut inner = ambient.clone();
            for (other, set) in sets.iter().enumerate() {
                if other != index {
                    inner.extend(set.iter().cloned());
                }
            }
            check_writer_tree(at, child, &inner, err)?;
        }
    }
    if let Effect::Sequence { children } = &def.effect {
        for child in children {
            check_writer_tree(at, child, ambient, err)?;
        }
    }
    Ok(())
}

/// The cue-level facts a composition check borrows from its caller: the
/// diagnostic location and capability list of the cue being validated, plus
/// its asset typing, top-level audio task membership and diagnostic
/// constructor. Bundling them keeps the recursive child check reviewable.
pub(super) struct ComposeCheck<'a> {
    pub(super) at: &'a str,
    pub(super) requires: &'a [String],
    pub(super) asset_kind: &'a dyn Fn(&str) -> Option<AssetKind>,
    pub(super) audio_task: &'a dyn Fn(&str) -> bool,
    pub(super) err: &'a dyn Fn(&str, &str, &str) -> Diagnostic,
}

/// Capability and bounds checks shared by inline, static, and composed effects.
pub(super) fn validate_image_inheritance(
    nodes: &[Node],
    inherited: &[String],
    geometry: &[String],
    at: &str,
) -> Result<()> {
    if geometry.len() > inherited.len()
        || geometry.iter().collect::<BTreeSet<_>>().len() != geometry.len()
        || geometry.iter().any(|id| !inherited.contains(id))
        || inherited.len() > MAX_NODES
        || inherited.iter().collect::<BTreeSet<_>>().len() != inherited.len()
        || inherited.iter().any(|id| {
            nodes.iter().find(|n| &n.id == id).is_none_or(|n| {
                n.asset.is_none()
                    || n.parent.is_some()
                    || n.bitmap_text.is_some()
                    || n.timeline_binding.is_some()
                    || n.sprite_transform.is_some()
            })
        })
    {
        return Err(Diagnostic::new(
            "E_STAGE_INHERIT",
            at,
            "requires unique ordinary image roots",
        ));
    }
    Ok(())
}

pub(super) fn validate_dialogue_styles(theme: &Theme, requires: &[String]) -> Result<()> {
    for (cap, needed) in [
        ("dialogue.style.v1", !theme.dialogue_styles.is_empty()),
        (
            "text.rect.v1",
            theme
                .dialogue_styles
                .values()
                .any(|style| style.dialogue.text_rect.is_some()),
        ),
    ] {
        if needed && !requires.iter().any(|value| value == cap) {
            return Err(err("E_CAPABILITY", "theme.dialogue_styles", cap));
        }
    }
    if theme
        .dialogue_styles
        .values()
        .any(|style| style.dialogue.shadow.is_some())
        && !requires.iter().any(|value| value == "text.shadow.v1")
    {
        return Err(err(
            "E_CAPABILITY",
            "theme.dialogue_styles",
            "text.shadow.v1",
        ));
    }
    Ok(())
}

pub(super) fn validate_effect_extensions(
    effect: &Effect,
    requires: &[String],
    theme: &Theme,
    duration: &impl Fn(&str) -> Option<u64>,
    at: &str,
) -> Result<()> {
    if effect.effect_tree_any(&|e| matches!(e, Effect::DialogueDecoration { .. }))
        && !requires.iter().any(|cap| cap == "dialogue.decoration.v1")
    {
        return Err(Diagnostic::new(
            "E_CAPABILITY",
            at,
            "dialogue.decoration.v1",
        ));
    }
    if effect.effect_tree_any(
        &|e| matches!(e, Effect::DialogueDecoration { image: Some(image), .. } if !image.valid()),
    ) {
        return Err(Diagnostic::new(
            "E_DIALOGUE_DECORATION",
            at,
            "invalid image placement",
        ));
    }
    if effect.effect_tree_any(&|e| matches!(e, Effect::DialogueDecoration { image: Some(image), .. }
        if matches!(image.placement, DialogueDecorationPlacement::TextOrigin { .. })
            && (theme.dialogue.text_rect.is_none() || theme.dialogue_styles.values().any(|style| style.dialogue.text_rect.is_none())))) {
        return Err(Diagnostic::new("E_DIALOGUE_DECORATION", at, "text-origin placement requires explicit text rectangles"));
    }
    if effect.effect_tree_any(&|e| matches!(e, Effect::StoryModal { .. }))
        && !requires.iter().any(|cap| cap == "ui.story-modal.v1")
    {
        return Err(Diagnostic::new("E_CAPABILITY", at, "ui.story-modal.v1"));
    }
    if effect.effect_tree_any(&|e| {
        matches!(e, Effect::StoryModal {
        target: nir_format::StoryModalTarget::ImageMenu { menu }
    } if !theme.image_menus.contains_key(menu))
    }) {
        return Err(Diagnostic::new("E_MENU", at, "unknown story modal menu"));
    }
    if effect.effect_tree_any(&|e| matches!(e, Effect::DialogueStyle { .. }))
        && !requires.iter().any(|cap| cap == "dialogue.style.v1")
    {
        return Err(Diagnostic::new("E_CAPABILITY", at, "dialogue.style.v1"));
    }
    if effect.effect_tree_any(&|e| matches!(e, Effect::DialogueStyle { style } if !theme.dialogue_styles.contains_key(style))) {
        return Err(Diagnostic::new("E_DIALOGUE_STYLE", at, "unknown dialogue style"));
    }
    if effect.effect_tree_any(&|e| matches!(e, Effect::SourceMotion { .. }))
        && !requires.iter().any(|cap| cap == "stage.source-motion.v1")
    {
        return Err(Diagnostic::new(
            "E_CAPABILITY",
            at,
            "stage.source-motion.v1",
        ));
    }
    if effect.effect_tree_any(&|e| {
        matches!(e, Effect::SourceMotion { node, property, to, duration_us, curve }
        if node.is_empty() || node.len() > 256
        || match property {
            Property::X | Property::Y => to.abs_diff(0) > 8192 || *curve == SourceMotionCurve::OpacityLinear,
            Property::Opacity => !(0..=255).contains(to) || *curve != SourceMotionCurve::OpacityLinear,
            _ => true,
        }
        || duration_us.0 > 60_000_000 || !duration_us.0.is_multiple_of(1000))
    }) {
        return Err(Diagnostic::new(
            "E_MOTION",
            at,
            "invalid bounded integer stock motion",
        ));
    }
    if effect.effect_tree_any(&|e| {
        matches!(
            e,
            Effect::SourceMotion {
                property: Property::Opacity,
                ..
            }
        )
    }) && !requires.iter().any(|cap| cap == "stage.source-opacity.v1")
    {
        return Err(Diagnostic::new(
            "E_CAPABILITY",
            at,
            "stage.source-opacity.v1",
        ));
    }
    if effect.effect_tree_any(
        &|e| matches!(e, Effect::StagePresent { inherit_images, .. } if !inherit_images.is_empty()),
    ) && !requires.iter().any(|cap| cap == "stage.inherit-image.v1")
    {
        return Err(Diagnostic::new(
            "E_CAPABILITY",
            at,
            "stage.inherit-image.v1",
        ));
    }
    if effect.effect_tree_any(
        &|e| matches!(e, Effect::StagePresent { inherit_image_geometry, .. } if !inherit_image_geometry.is_empty()),
    ) && !requires.iter().any(|cap| cap == "stage.inherit-image-geometry.v1")
    {
        return Err(Diagnostic::new("E_CAPABILITY", at, "stage.inherit-image-geometry.v1"));
    }
    if effect.effect_tree_any(&|e| {
        matches!(
            e,
            Effect::StagePresent {
                dialogue_visible: Some(_),
                ..
            }
        )
    }) && !requires.iter().any(|cap| cap == "stage.window-flip.v1")
    {
        return Err(Diagnostic::new("E_CAPABILITY", at, "stage.window-flip.v1"));
    }
    if effect.effect_tree_any(&|e| {
        matches!(e, Effect::StagePresent {
        dialogue_visible: Some(_), duration_us, .. } if duration_us.0 > 60_000_000)
    }) {
        return Err(Diagnostic::new(
            "E_TRANSITION",
            at,
            "reserved window flip exceeds 60 seconds",
        ));
    }
    if effect.effect_tree_any(&|e| {
        matches!(e, Effect::StagePresent {
        dialogue_visible: Some(_), duration_us, .. } if duration_us.0 > 0)
    }) && !requires
        .iter()
        .any(|cap| cap == "text.window-transition.v1")
    {
        return Err(Diagnostic::new(
            "E_CAPABILITY",
            at,
            "text.window-transition.v1",
        ));
    }
    if effect.effect_tree_any(&|e| matches!(e, Effect::SpriteTimeline { .. }))
        && !requires.iter().any(|c| c == "stage.sprite-timeline.v1")
    {
        return Err(err("E_CAPABILITY", at, "stage.sprite-timeline.v1"));
    }
    if effect.effect_tree_any(&|e| {
        matches!(e, Effect::SpriteTimeline { timeline, root, duration_us, .. }
        if timeline.is_empty() || timeline.len() > 256 || root.is_empty() || root.len() > 256
            || duration_us.0 == 0 || duration_us.0 > 3_600_000_000)
    }) {
        return Err(err("E_TIMELINE", at, "invalid finite sprite timeline"));
    }
    if effect.effect_tree_any(&|e| {
        matches!(
            e,
            Effect::SpriteTimeline {
                delete_on_finish: true,
                ..
            }
        )
    }) && !requires.iter().any(|c| c == "stage.sprite-lifecycle.v1")
    {
        return Err(err("E_CAPABILITY", at, "stage.sprite-lifecycle.v1"));
    }
    if effect.effect_tree_any(&|e| matches!(e, Effect::SpriteWave { .. }))
        && !requires.iter().any(|cap| cap == "stage.sprite-wave.v1")
    {
        return Err(Diagnostic::new("E_CAPABILITY", at, "stage.sprite-wave.v1"));
    }
    if effect.effect_tree_any(&|e| {
        matches!(e, Effect::SpriteWave { nodes, spec }
        if !spec.valid() || spec.randomize || nodes.is_empty() || nodes.len() > 32
        || nodes.iter().any(|n| n.is_empty() || n.len() > 256)
        || nodes.iter().collect::<BTreeSet<_>>().len() != nodes.len())
    }) {
        return Err(Diagnostic::new("E_SHAKE", at, "invalid finite sprite wave"));
    }
    if effect.effect_tree_any(&|e| matches!(e, Effect::SpriteShake { .. }))
        && !requires.iter().any(|cap| cap == "stage.sprite-shake.v1")
    {
        return Err(Diagnostic::new("E_CAPABILITY", at, "stage.sprite-shake.v1"));
    }
    if effect.effect_tree_any(&|e| {
        matches!(
            e,
            Effect::SpriteShake {
                mode: nir_format::SpriteShakeMode::Quake,
                ..
            }
        )
    }) && !requires.iter().any(|cap| cap == "stage.sprite-quake.v1")
    {
        return Err(Diagnostic::new("E_CAPABILITY", at, "stage.sprite-quake.v1"));
    }
    if effect.effect_tree_any(&|e| {
        matches!(e,Effect::SpriteShake { nodes,spec,.. }
        if !spec.valid() || nodes.is_empty() || nodes.len()>32
            || nodes.iter().any(|n|n.is_empty()||n.len()>256)
            || nodes.iter().collect::<BTreeSet<_>>().len()!=nodes.len())
    }) {
        return Err(Diagnostic::new(
            "E_SHAKE",
            at,
            "invalid independent sprite shake",
        ));
    }
    if effect.effect_tree_any(&|e| matches!(e, Effect::DialogueShake { .. }))
        && !requires.iter().any(|cap| cap == "stage.dialogue-shake.v1")
    {
        return Err(Diagnostic::new(
            "E_CAPABILITY",
            at,
            "stage.dialogue-shake.v1",
        ));
    }
    if effect.effect_tree_any(&|e| matches!(e, Effect::DialogueShake { spec } if !spec.valid())) {
        return Err(Diagnostic::new(
            "E_SHAKE",
            at,
            "invalid finite dialogue shake",
        ));
    }
    if effect.effect_tree_any(&|e| {
        matches!(
            e,
            Effect::Audio {
                loop_region: Some(_),
                ..
            }
        )
    }) && !requires.iter().any(|cap| cap == "audio.loop-region.v1")
    {
        return Err(Diagnostic::new("E_CAPABILITY", at, "audio.loop-region.v1"));
    }
    if effect.effect_tree_any(&|e| match e {
        Effect::Audio {
            asset,
            looped,
            loop_region: Some(region),
            ..
        } => !looped || !region.valid(duration(asset).unwrap_or(u64::MAX)),
        _ => false,
    }) {
        return Err(Diagnostic::new(
            "E_AUDIO_LOOP",
            at,
            "loop requires 0 <= start < end <= decoded duration",
        ));
    }
    Ok(())
}

pub(super) fn check_compose_child<'a>(
    def: &'a EffectDef,
    parent_scope: Scope,
    depth: usize,
    names: &mut BTreeSet<&'a String>,
    check: &ComposeCheck<'_>,
) -> Result<()> {
    let ComposeCheck {
        at,
        requires,
        asset_kind,
        audio_task,
        err,
    } = *check;
    if !names.insert(&def.id) {
        return Err(err("E_DUPLICATE", at, &def.id));
    }
    if def.scope != parent_scope {
        return Err(err("E_SCOPE", at, &def.id));
    }
    if depth > MAX_COMPOSE_DEPTH {
        return Err(err("E_LIMIT", at, "composition nesting"));
    }
    if matches!(
        def.effect,
        Effect::StagePresent { .. }
            | Effect::Dialogue { .. }
            | Effect::StoryModal { .. }
            | Effect::DialogueStyle { .. }
            | Effect::DialogueDecoration { .. }
            | Effect::SpriteTimeline { .. }
    ) {
        return Err(err(
            "E_COMPOSE",
            at,
            "composition children cannot present stages or dialogue",
        ));
    }
    if matches!(def.effect, Effect::Tween { .. })
        && !requires.iter().any(|c| c == "tween.target.v1")
    {
        return Err(err("E_CAPABILITY", at, "tween.target.v1"));
    }
    match &def.effect {
        Effect::Audio { gain, .. }
            if *gain != 1.0 && !requires.iter().any(|c| c == "audio.gain.v1") =>
        {
            return Err(err("E_CAPABILITY", at, "audio.gain.v1"));
        }
        Effect::Audio { gain, .. } if !valid_audio_gain(*gain) => {
            return Err(err(
                "E_AUDIO_GAIN",
                at,
                "event gain must be finite and within 0..4",
            ));
        }
        Effect::Audio { asset, .. } if asset_kind(asset) != Some(AssetKind::Audio) => {
            return Err(err("E_ASSET_TYPE", at, asset));
        }
        Effect::AudioStop {
            target,
            duration_us,
        } => {
            if !requires.iter().any(|cap| cap == "audio.stop.v1") {
                return Err(err("E_CAPABILITY", at, "audio.stop.v1"));
            }
            if duration_us.0 > 60_000_000 || target == &def.id || !audio_task(target) {
                return Err(err(
                    "E_AUDIO_STOP",
                    at,
                    "stop requires an audio target and duration within 0..60s",
                ));
            }
        }
        _ => {}
    }
    if def
        .effect
        .scalar_track(0., 0.)
        .is_some_and(|(address, track, _)| !address.accepts(track.to))
    {
        return Err(err("E_VISUAL", at, "invalid target value"));
    }
    if let Some((TweenTarget::AudioInstance { task, .. }, track, _)) =
        def.effect.scalar_track(0., 0.)
    {
        if !requires.iter().any(|c| c == "audio.gain-tween.v1") {
            return Err(err("E_CAPABILITY", at, "audio.gain-tween.v1"));
        }
        if !matches!(track.easing, Easing::Linear) {
            return Err(err("E_AUDIO_STOP", at, "envelope tweens are linear"));
        }
        if task == def.id.as_str() || !audio_task(task.as_str()) {
            return Err(err("E_AUDIO_STOP", at, task.as_str()));
        }
    }
    for child in def.effect.compose_children().unwrap_or(&[]) {
        check_compose_child(child, def.scope, depth + 1, names, check)?;
    }
    Ok(())
}

/// Composition cue rules shared by source and package validation. Runs after
/// the top-level loop; `names` carries the cue's task ids so children cannot
/// shadow them.
pub(super) fn validate_composition<'a>(
    id: &str,
    cue: &'a Cue,
    requires: &[String],
    names: &mut BTreeSet<&'a String>,
    asset_kind: impl Fn(&str) -> Option<AssetKind>,
    audio_task: impl Fn(&str) -> bool,
    err: impl Fn(&str, &str, &str) -> Diagnostic,
) -> Result<()> {
    if !cue.effects.iter().any(|def| def.effect.uses_compose()) {
        return Ok(());
    }
    if !requires.iter().any(|c| c == "task.compose.v1") {
        return Err(err("E_CAPABILITY", id, "task.compose.v1"));
    }
    let total: usize = cue
        .effects
        .iter()
        .map(|def| def.effect.compose_leaves())
        .sum();
    if total > MAX_TASKS {
        return Err(err("E_LIMIT", id, "invalid cue size"));
    }
    let check = ComposeCheck {
        at: id,
        requires,
        asset_kind: &asset_kind,
        audio_task: &audio_task,
        err: &err,
    };
    for def in &cue.effects {
        if def.effect.compose_children().is_none() {
            continue;
        }
        if def.effect.compose_depth() > MAX_COMPOSE_DEPTH {
            return Err(err("E_LIMIT", id, "composition nesting"));
        }
        let ambient: BTreeSet<_> = cue
            .effects
            .iter()
            .filter(|other| other.id != def.id)
            .flat_map(subtree_writers)
            .collect();
        match &def.effect {
            Effect::Sequence { children } => {
                for child in children {
                    check_writer_tree(id, child, &ambient, &err)?;
                }
            }
            Effect::ParallelAll { children } => {
                let sets: Vec<_> = children.iter().map(subtree_writers).collect();
                for (index, child) in children.iter().enumerate() {
                    let mut inner = ambient.clone();
                    for (other, set) in sets.iter().enumerate() {
                        if other != index {
                            inner.extend(set.iter().cloned());
                        }
                    }
                    check_writer_tree(id, child, &inner, &err)?;
                }
            }
            _ => {}
        }
        for child in def.effect.compose_children().unwrap_or(&[]) {
            check_compose_child(child, def.scope, 1, names, &check)?;
        }
    }
    Ok(())
}
