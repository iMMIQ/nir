use super::*;

pub(crate) fn validate_timeline(
    id: &str,
    timeline: &SpriteTimeline,
    requires: &[String],
) -> Result<()> {
    if !requires.iter().any(|c| c == "stage.sprite-timeline.v1") {
        return Err(err("E_CAPABILITY", id, "stage.sprite-timeline.v1"));
    }
    if timeline.id != id || !timeline.valid() {
        return Err(err("E_TIMELINE", id, "invalid immutable keyframes"));
    }
    if timeline
        .tracks
        .iter()
        .flat_map(|t| &t.frames)
        .any(|f| f.transform.is_some())
        && !requires.iter().any(|c| c == "stage.sprite-transform.v1")
    {
        return Err(err("E_CAPABILITY", id, "stage.sprite-transform.v1"));
    }
    Ok(())
}
pub(crate) fn validate_timeline_bindings<'a>(
    nodes: &[Node],
    at: &str,
    get: impl Fn(&str) -> Option<&'a SpriteTimeline>,
) -> Result<()> {
    let mut owned = BTreeSet::new();
    for root in nodes.iter().filter(|n| n.timeline_binding.is_some()) {
        let id = root.timeline_binding.as_deref().unwrap();
        let timeline = get(id).ok_or_else(|| err("E_TIMELINE", at, "timeline is not resident"))?;
        for track in &timeline.tracks {
            let node = nodes
                .iter()
                .find(|n| n.id == track.node)
                .ok_or_else(|| err("E_TIMELINE", at, "keyframe target missing"))?;
            if node.id == root.id
                || node.bitmap_text.is_some()
                || node.timeline_binding.is_some()
                || nodes
                    .iter()
                    .any(|child| child.parent.as_deref() == Some(node.id.as_str()))
                || !owned.insert(&node.id)
                || (node.clip.is_some() && track.frames.iter().any(|f| f.transform.is_some()))
            {
                return Err(err(
                    "E_TIMELINE",
                    at,
                    "keyframes require exclusive descendant leaves",
                ));
            }
            let mut parent = node.parent.as_deref();
            let mut seen = BTreeSet::new();
            while parent != Some(root.id.as_str()) {
                let key =
                    parent.ok_or_else(|| err("E_TIMELINE", at, "target outside timeline root"))?;
                if !seen.insert(key) {
                    return Err(err("E_TIMELINE", at, "cyclic timeline hierarchy"));
                }
                parent = nodes
                    .iter()
                    .find(|n| n.id == key)
                    .and_then(|n| n.parent.as_deref());
            }
        }
    }
    Ok(())
}

pub(super) fn validate_scene_nodes(nodes: &[Node], id: &str, root: &RuntimeProgram) -> Result<()> {
    if nodes.len() > MAX_NODES {
        return Err(err("E_LIMIT", id, "too many nodes"));
    }
    let mut ids = BTreeSet::new();
    for node in nodes {
        if node.sprite_transform.is_some()
            && nodes
                .iter()
                .any(|child| child.parent.as_deref() == Some(node.id.as_str()))
        {
            return Err(err("E_VISUAL", id, "sprite transforms require leaf nodes"));
        }
        validate_bitmap_node(
            node,
            id,
            &root.variables,
            &root.requires,
            root.title_scene.as_deref(),
        )?;
        if !ids.insert(&node.id) {
            return Err(err("E_DUPLICATE", id, &node.id));
        }
        if ![
            node.x,
            node.y,
            node.width,
            node.height,
            node.scale,
            node.opacity,
        ]
        .iter()
        .chain(node.color.iter())
        .all(|v| v.is_finite())
            || node.width < 0.
            || node.height < 0.
            || node.scale < 0.
            || !(0.0..=1.0).contains(&node.opacity)
        {
            return Err(err("E_VISUAL", id, &node.id));
        }
        if node
            .clip
            .is_some_and(|clip| !clip.iter().all(|v| v.is_finite()) || clip[2] < 0. || clip[3] < 0.)
        {
            return Err(err("E_VISUAL", id, &node.id));
        }
        if let Some(asset) = &node.asset {
            if root.assets.get(asset).map(|a| a.kind) != Some(AssetKind::Image) {
                return Err(err("E_ASSET_TYPE", id, asset));
            }
        }
    }
    for node in nodes {
        let mut seen = BTreeSet::new();
        let mut parent = node.parent.as_ref();
        while let Some(key) = parent {
            if key == &node.id || !seen.insert(key) {
                return Err(err("E_SCENE_CYCLE", id, key));
            }
            parent = nodes
                .iter()
                .find(|candidate| &candidate.id == key)
                .ok_or_else(|| err("E_NODE", id, key))?
                .parent
                .as_ref();
        }
    }
    Ok(())
}

pub(super) fn validate_bitmap_node(
    node: &Node,
    scene: &str,
    variables: &BTreeMap<String, Value>,
    requires: &[String],
    title: Option<&str>,
) -> Result<()> {
    if node.inherit_existence {
        if !requires.iter().any(|c| c == "stage.sprite-lifecycle.v1") {
            return Err(err("E_CAPABILITY", scene, "stage.sprite-lifecycle.v1"));
        }
        if title == Some(scene) || node.parent.is_some() || node.timeline_binding.is_none() {
            return Err(err(
                "E_TIMELINE",
                scene,
                "invalid inherited animated existence",
            ));
        }
    }
    if node.timeline_binding.is_some() {
        if !requires.iter().any(|c| c == "stage.sprite-timeline.v1") {
            return Err(err("E_CAPABILITY", scene, "stage.sprite-timeline.v1"));
        }
        if title == Some(scene) || node.bitmap_text.is_some() {
            return Err(err("E_TIMELINE", scene, "invalid timeline binding"));
        }
    }
    if let Some(transform) = node.sprite_transform {
        if !requires
            .iter()
            .any(|cap| cap == "stage.sprite-transform.v1")
        {
            return Err(err("E_CAPABILITY", scene, "stage.sprite-transform.v1"));
        }
        if !transform.valid(node.width, node.height)
            || node.clip.is_some()
            || node.bitmap_text.is_some()
        {
            return Err(err("E_VISUAL", scene, "invalid leaf sprite transform"));
        }
    }
    if node.offset != [0.; 2] {
        if !requires.iter().any(|cap| cap == "stage.sprite-wave.v1") {
            return Err(err("E_CAPABILITY", scene, "stage.sprite-wave.v1"));
        }
        if node
            .offset
            .iter()
            .any(|n| !n.is_finite() || n.abs() > 8192.)
        {
            return Err(err("E_VISUAL", scene, "invalid sprite offset"));
        }
    }
    if !node.preserve_pose.is_empty() {
        if !requires
            .iter()
            .any(|cap| cap == "stage.sprite-continuity.v1")
        {
            return Err(err("E_CAPABILITY", scene, "stage.sprite-continuity.v1"));
        }
        if title == Some(scene)
            || node.preserve_pose.iter().collect::<BTreeSet<_>>().len() != node.preserve_pose.len()
        {
            return Err(err("E_VISUAL", scene, "invalid preserved pose properties"));
        }
    }
    if let Some(recipe) = &node.bitmap_text {
        if !requires.iter().any(|cap| cap == "stage.bitmap-text.v1") {
            return Err(err("E_CAPABILITY", scene, "stage.bitmap-text.v1"));
        }
        if !recipe.valid(variables) || node.asset.is_none() || title == Some(scene) {
            return Err(err(
                "E_BITMAP_TEXT",
                scene,
                "invalid bitmap recipe or title placement",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_static_package(
    view: &RuntimeProgramView,
    package: &ModuleStatic,
) -> Result<()> {
    let root = view.runtime_root().unwrap();
    if package
        .activation_recipes
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
        != package.cues.keys().cloned().collect::<BTreeSet<_>>()
    {
        return Err(err(
            "E_RECIPE",
            &package.module,
            "activation recipe set differs from cues",
        ));
    }
    for (id, timeline) in &package.sprite_timelines {
        validate_timeline(id, timeline, &view.requires)?;
    }
    for (id, nodes) in &package.scenes {
        validate_scene_nodes(nodes, id, root)?;
        validate_timeline_bindings(nodes, id, |key| package.sprite_timelines.get(key))?;
    }
    if view
        .title_scene
        .as_deref()
        .is_some_and(|title| root.scene_owners.get(title) == Some(&package.module))
        && package
            .scenes
            .get(view.title_scene.as_ref().unwrap())
            .is_some_and(|nodes| nodes.as_slice() != view.title_nodes.as_slice())
    {
        return Err(err(
            "E_SCENE",
            "title_scene",
            "root title nodes differ from static scene",
        ));
    }
    for (id, cue) in &package.cues {
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
        let mut stages = 0;
        let mut dialogues = 0;
        for def in &cue.effects {
            validate_effect_extensions(
                &def.effect,
                &view.requires,
                &view.theme,
                &|asset| view.asset(asset).map(|a| a.duration_us.0),
                id,
            )?;
            if !names.insert(&def.id) {
                return Err(err("E_DUPLICATE", id, &def.id));
            }
            if matches!(def.effect, Effect::Tween { .. })
                && !root.requires.iter().any(|c| c == "tween.target.v1")
            {
                return Err(err("E_CAPABILITY", id, "tween.target.v1"));
            }
            if let Some((address, track, _)) = def.effect.scalar_track(0., 0.) {
                if !address.accepts(track.to) {
                    return Err(err("E_VISUAL", id, "invalid target value"));
                }
                if let TweenTarget::AudioInstance { task, .. } = &address {
                    if !root.requires.iter().any(|c| c == "audio.gain-tween.v1") {
                        return Err(err("E_CAPABILITY", id, "audio.gain-tween.v1"));
                    }
                    if !matches!(track.easing, Easing::Linear) {
                        return Err(err("E_AUDIO_STOP", id, "envelope tweens are linear"));
                    }
                    if task == &def.id
                        || root.task_owners.get(task) != Some(&package.module)
                        || !view.task_definitions.get(task).is_some_and(|defs| {
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
                    if root.timeline_owners.get(timeline) != Some(&package.module)
                        || view
                            .sprite_timelines
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
                        if !root.requires.iter().any(|c| c == cap) {
                            return Err(err("E_CAPABILITY", id, cap));
                        }
                    }
                    if let Some(asset) = transition.asset() {
                        if view.asset_kind(asset) != Some(AssetKind::Image) {
                            return Err(err("E_ASSET", id, "mask must reference an image"));
                        }
                    }
                    stages += 1;
                    if root.scene_owners.get(scene) != Some(&package.module)
                        || view.scenes.get(scene).is_none()
                    {
                        return Err(err("E_SCENE", id, scene));
                    }
                    validate_image_inheritance(
                        view.scenes[scene].as_slice(),
                        inherit_images,
                        inherit_image_geometry,
                        id,
                    )?;
                }
                Effect::Dialogue { text, speaker, .. } => {
                    dialogues += 1;
                    if root.text_owners.get(text) != Some(&package.module)
                        || view.texts.get(text).is_none()
                        || (!speaker.is_empty()
                            && (root.text_owners.get(speaker) != Some(&package.module)
                                || view.texts.get(speaker).is_none()))
                    {
                        return Err(err("E_TEXT", id, text));
                    }
                }
                Effect::AudioStop {
                    target,
                    duration_us,
                } => {
                    if !view.requires.iter().any(|cap| cap == "audio.stop.v1") {
                        return Err(err("E_CAPABILITY", id, "audio.stop.v1"));
                    }
                    if root.task_owners.get(target) != Some(&package.module)
                        || duration_us.0 > 60_000_000
                        || target == &def.id
                        || !view.task_definitions.get(target).is_some_and(|defs| {
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
                    if *gain != 1.0 && !view.requires.iter().any(|c| c == "audio.gain.v1") =>
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
                } => {
                    if view.asset_kind(&image.asset) != Some(AssetKind::Image) {
                        return Err(err("E_ASSET_TYPE", id, &image.asset));
                    }
                    if let Some(asset) = view.asset(&image.asset) {
                        if [asset.width, asset.height] != image.size {
                            return Err(err(
                                "E_VISUAL",
                                id,
                                "dialogue decoration size must match image asset dimensions",
                            ));
                        }
                    }
                }
                Effect::Audio { asset, .. } if view.asset_kind(asset) != Some(AssetKind::Audio) => {
                    return Err(err("E_ASSET_TYPE", id, asset));
                }
                _ => {}
            }
        }
        if stages > 1 || dialogues > 1 {
            return Err(err("E_CUE", id, "at most one stage and dialogue per cue"));
        }
        validate_composition(
            id,
            cue,
            &view.requires,
            &mut names,
            |asset| view.asset_kind(asset),
            |target| {
                root.task_owners.get(target) == Some(&package.module)
                    && view.task_definitions.get(target).is_some_and(|defs| {
                        !defs.is_empty()
                            && defs
                                .iter()
                                .all(|effect| matches!(effect.as_ref(), Effect::Audio { .. }))
                    })
            },
            err,
        )?;
        let recipe = &package.activation_recipes[id];
        if recipe.iter().any(|asset| {
            view.asset_kind(asset)
                .is_none_or(|kind| kind == AssetKind::Font)
        }) {
            return Err(err(
                "E_RECIPE",
                id,
                "recipe references a missing or non-runtime asset",
            ));
        }
        let mut expected = BTreeSet::new();
        for definition in &cue.effects {
            definition
                .effect
                .collect_dialogue_style_assets(&view.theme, &mut expected);
        }
        let mut all_scenes_known = true;
        for effect in &cue.effects {
            let mut texts = BTreeSet::new();
            effect.effect.collect_text_ids(&mut texts);
            for text in texts {
                if let Some(identity) = root.text_contracts.get(&text) {
                    for image in &identity.images {
                        expected.insert(image.asset.clone());
                        if !recipe.contains(&image.asset) {
                            return Err(err(
                                "E_RECIPE",
                                id,
                                "inline image missing from activation recipe",
                            ));
                        }
                    }
                }
            }
            if let Effect::Audio { asset, .. } = &effect.effect {
                expected.insert(asset.clone());
                if !recipe.contains(asset) {
                    return Err(err(
                        "E_RECIPE",
                        id,
                        "audio asset missing from activation recipe",
                    ));
                }
            }
            if effect.effect.uses_compose() {
                let mut child_audio = BTreeSet::new();
                effect.effect.collect_audio_assets(&mut child_audio);
                for asset in child_audio {
                    if !recipe.contains(&asset) {
                        return Err(err(
                            "E_RECIPE",
                            id,
                            "audio asset missing from activation recipe",
                        ));
                    }
                    expected.insert(asset);
                }
            }
            if let Effect::StagePresent {
                scene, transition, ..
            } = &effect.effect
            {
                if let Some(asset) = transition.asset() {
                    expected.insert(asset.to_owned());
                    if !recipe.contains(asset) {
                        return Err(err("E_RECIPE", id, "mask missing from activation recipe"));
                    }
                }
                let nodes = view.scenes.get(scene).map(Vec::as_slice).or_else(|| {
                    (view.title_scene.as_deref() == Some(scene))
                        .then_some(view.title_nodes.as_slice())
                });
                if let Some(nodes) = nodes {
                    for asset in effect.effect.stage_image_assets(nodes) {
                        expected.insert(asset.clone());
                    }
                    if effect
                        .effect
                        .stage_image_assets(nodes)
                        .any(|asset| !recipe.contains(asset))
                    {
                        return Err(err(
                            "E_RECIPE",
                            id,
                            "scene image missing from activation recipe",
                        ));
                    }
                } else {
                    all_scenes_known = false;
                }
            }
        }
        if (all_scenes_known && recipe != &expected)
            || (!all_scenes_known && !expected.is_subset(recipe))
        {
            return Err(err(
                "E_RECIPE",
                id,
                "activation recipe differs from cue consumers",
            ));
        }
    }
    for (id, choice) in &package.choices {
        if choice.uses_float80() && !view.requires.iter().any(|c| c == "story.float80.v1") {
            return Err(err("E_CAPABILITY", id, "story.float80.v1"));
        }
        validate_choice_images(id, choice, &view.requires, |asset| {
            view.asset_index.get(asset).map(|a| a.kind)
        })?;
        let mut ids = BTreeSet::new();
        for option in &choice.options {
            if !ids.insert(&option.id) {
                return Err(err("E_DUPLICATE", id, &option.id));
            }
            if root.text_owners.get(&option.text) != Some(&package.module)
                || view.texts.get(&option.text).is_none()
            {
                return Err(err("E_TEXT", id, &option.text));
            }
            let vars = view
                .variables
                .iter()
                .map(|(key, value)| (key.clone(), value.ty()))
                .collect();
            for expression in [&option.visible, &option.enabled].into_iter().flatten() {
                if expr_type(expression, &vars, id)? != ValueType::Bool {
                    return Err(err("E_TYPE", id, "choice predicate must be Bool"));
                }
            }
        }
        if choice.timeout_us.is_some()
            && !choice
                .default
                .as_ref()
                .is_some_and(|default| ids.contains(default))
        {
            return Err(err(
                "E_CHOICE_DEFAULT",
                id,
                "timeout requires a valid default",
            ));
        }
    }
    for (id, contract) in &package.text_contracts {
        let Some(identity) = root.text_contracts.get(id) else {
            return Err(err("E_TEXT_CONTRACT", id, "text identity missing"));
        };
        if contract.source_revision != identity.source_revision
            || contract.contract_revision != identity.contract_revision
            || contract.meaning_revision != identity.meaning_revision
            || contract.contract_digest != identity.contract_digest
            || contract.images != identity.images
            || contract.contract_digest != text_contract_digest(contract)
            || contract
                .params
                .iter()
                .any(|(param, ty)| view.variables.get(param).map(Value::ty) != Some(*ty))
        {
            return Err(err(
                "E_TEXT_REVISION",
                id,
                "contract differs from root identity",
            ));
        }
    }
    Ok(())
}
