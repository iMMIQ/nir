use super::*;

pub(super) fn validate_runtime_function(
    view: &RuntimeProgramView,
    fid: &str,
    function: &Function,
) -> Result<()> {
    if function.uses_float80() && !view.requires.iter().any(|c| c == "story.float80.v1") {
        return Err(err("E_CAPABILITY", fid, "story.float80.v1"));
    }
    if !function.blocks.contains_key(&function.entry) || function.blocks.len() > 100_000 {
        return Err(err("E_BLOCK", fid, "invalid entry/block limit"));
    }
    let mut vars: BTreeMap<_, _> = view
        .variables
        .iter()
        .map(|(key, value)| (key.clone(), value.ty()))
        .collect();
    for (key, ty) in function.params.iter().chain(function.locals.iter()) {
        if vars.insert(key.clone(), *ty).is_some() {
            return Err(err("E_DUPLICATE", fid, key));
        }
    }
    let mut op_ids = BTreeSet::new();
    for (bid, block) in &function.blocks {
        let at = format!("{fid}/{bid}");
        for target in outgoing(&block.terminator) {
            if !function.blocks.contains_key(target) {
                return Err(err("E_BLOCK", &at, target));
            }
        }
        for op in &block.ops {
            if !op_ids.insert(&op.id) {
                return Err(err("E_DUPLICATE", &at, &op.id));
            }
            if let Operation::DialogueVoice { task, voice, wait } = &op.operation {
                if !view.requires.iter().any(|c| c == "text.voice-binding.v1") {
                    return Err(err("E_CAPABILITY", &op.id, "text.voice-binding.v1"));
                }
                let root = view.runtime_root().unwrap();
                if *wait == VoiceWaitPolicy::SampledRemaining {
                    if !view.requires.iter().any(|c| c == "text.voice-timer.v1") {
                        return Err(err("E_CAPABILITY", &op.id, "text.voice-timer.v1"));
                    }
                    // Code may precede its asset catalog. Catalog admission independently
                    // rejects audio without a positive duration before playback.
                    if voice.as_ref().is_some_and(|name|view.task_definitions.get(name).is_some_and(|defs|defs.iter().any(|effect|
                        matches!(effect.as_ref(),Effect::Audio{asset,..} if view.asset(asset).is_some_and(|a|a.duration_us.0==0))
                    ))) { return Err(err("E_VOICE_DURATION",&op.id,"sampled voice timer requires known asset duration")); }
                }
                let local = |name: &str| {
                    root.task_owners.get(name).map(String::as_str)
                        == view.function_execution_module(fid)
                };
                let dialogue = view.task_definitions.get(task);
                if !local(task)
                    || !dialogue.is_some_and(|defs| {
                        !defs.is_empty()
                            && defs
                                .iter()
                                .all(|e| matches!(e.as_ref(), Effect::Dialogue { .. }))
                    })
                    || voice.as_ref().is_some_and(|name| {
                        !local(name)
                            || !view.task_definitions.get(name).is_some_and(|defs| {
                                defs.iter().all(|e| {
                                    matches!(
                                        e.as_ref(),
                                        Effect::Audio {
                                            bus: AudioBus::Voice,
                                            ..
                                        }
                                    )
                                }) && defs.iter().any(|e| {
                                    matches!(e.as_ref(), Effect::Audio { looped: false, .. })
                                })
                            })
                    })
                {
                    return Err(err(
                        "E_VOICE_BINDING",
                        &op.id,
                        "expected local dialogue and non-looping Voice task",
                    ));
                }
            }
            if let Operation::DialogueVisibility {
                transition: Some(style),
                duration_us,
                ..
            } = &op.operation
            {
                if !view
                    .requires
                    .iter()
                    .any(|c| c == "text.window-transition.v1")
                {
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
                if style
                    .asset()
                    .is_some_and(|asset| view.asset_kind(asset) != Some(AssetKind::Image))
                {
                    return Err(err("E_ASSET", &op.id, "mask must reference an image"));
                }
            }
            match &op.operation {
                Operation::MenuAccess { .. }
                    if !view.requires.iter().any(|c| c == "player.menu-access.v1") =>
                {
                    return Err(err("E_CAPABILITY", &op.id, "player.menu-access.v1"));
                }
                Operation::AudioPause { .. }
                    if !view.requires.iter().any(|c| c == "audio.pause.v1") =>
                {
                    return Err(err("E_CAPABILITY", &op.id, "audio.pause.v1"));
                }
                Operation::ProfileValueRead { target, key }
                | Operation::ProfileValueAssign { target, key, .. } => {
                    if !view.requires.iter().any(|c| c == "story.profile-value.v1") {
                        return Err(err("E_CAPABILITY", &op.id, "story.profile-value.v1"));
                    }
                    if !view.variables.contains_key(target) || key.is_empty() || key.len() > 1024 {
                        return Err(err("E_PROFILE", &op.id, "invalid profile value target/key"));
                    }
                    if let Operation::ProfileValueAssign { value, .. } = &op.operation {
                        if view.variables.get(target).map(Value::ty)
                            != Some(expr_type(value, &vars, &op.id)?)
                        {
                            return Err(err("E_TYPE", &op.id, target));
                        }
                    }
                }
                Operation::ProfileRead { target, key } => {
                    if !view.requires.iter().any(|c| c == "story.profile-read.v1") {
                        return Err(err("E_CAPABILITY", &op.id, "story.profile-read.v1"));
                    }
                    if view.variables.get(target).map(Value::ty) != Some(ValueType::I32)
                        || key.is_empty()
                        || key.len() > 1024
                    {
                        return Err(err("E_PROFILE", &op.id, "invalid profile flag target/key"));
                    }
                }
                Operation::Assign { target, value }
                    if vars.get(target).copied() != Some(expr_type(value, &vars, &op.id)?) =>
                {
                    return Err(err("E_TYPE", &op.id, target));
                }
                Operation::Random { target, min, max }
                    if vars.get(target) != Some(&ValueType::I32) || min > max =>
                {
                    return Err(err("E_RANDOM", &op.id, "invalid bounds/target"));
                }
                Operation::DraftPatch { value, .. } if !value.is_finite() => {
                    return Err(err("E_VISUAL", &op.id, "non-finite patch"));
                }
                _ => {}
            }
        }
        match &block.terminator {
            Terminator::Branch { condition, .. }
                if expr_type(condition, &vars, &at)? != ValueType::Bool =>
            {
                return Err(err("E_TYPE", &at, "branch requires Bool"));
            }
            Terminator::Switch { value, cases, .. } => {
                let ty = expr_type(value, &vars, &at)?;
                if matches!(ty, ValueType::Bool | ValueType::F80)
                    || (ty == ValueType::I32 && cases.keys().any(|key| key.parse::<i32>().is_err()))
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
                let callee = view
                    .function_signature(function)
                    .ok_or_else(|| err("E_FUNCTION", &at, function))?;
                if args.len() != callee.params.len() {
                    return Err(err("E_CALL", &at, "argument count"));
                }
                for (key, ty) in &callee.params {
                    let expr = args.get(key).ok_or_else(|| err("E_CALL", &at, key))?;
                    if expr_type(expr, &vars, &at)? != *ty {
                        return Err(err("E_TYPE", &at, key));
                    }
                }
                if result
                    .as_ref()
                    .is_some_and(|target| vars.get(target).copied() != callee.returns)
                {
                    return Err(err("E_TYPE", &at, "return target"));
                }
            }
            Terminator::Return { value }
                if value
                    .as_ref()
                    .map(|e| expr_type(e, &vars, &at))
                    .transpose()?
                    != function.returns =>
            {
                return Err(err("E_TYPE", &at, "return type"));
            }
            Terminator::Activate { cue, .. }
                if view
                    .runtime_root()
                    .unwrap()
                    .cue_owners
                    .get(cue)
                    .map(String::as_str)
                    != view.function_execution_module(fid)
                    || view.cues.get(cue).is_none() =>
            {
                return Err(err("E_CUE", &at, cue));
            }
            Terminator::Await { conditions, .. }
                if conditions.is_empty()
                    || conditions.iter().any(|condition| {
                        view.runtime_root()
                            .unwrap()
                            .task_owners
                            .get(&condition.task)
                            .map(String::as_str)
                            != view.function_execution_module(fid)
                            || !view.task_definitions.contains_key(&condition.task)
                    }) =>
            {
                return Err(err(
                    "E_TASK",
                    &at,
                    "unknown, unloaded or cross-module task wait",
                ));
            }
            Terminator::Interact { choice, .. }
                if view
                    .runtime_root()
                    .unwrap()
                    .choice_owners
                    .get(choice)
                    .map(String::as_str)
                    != view.function_execution_module(fid)
                    || view.choices.get(choice).is_none() =>
            {
                return Err(err("E_CHOICE", &at, choice));
            }
            _ => {}
        }
        if let Terminator::Await {
            conditions,
            on_advance,
            ..
        } = &block.terminator
        {
            if on_advance.is_some() && !view.requires.iter().any(|c| c == "control.advance-wait.v1")
            {
                return Err(err("E_CAPABILITY", &at, "control.advance-wait.v1"));
            }
            for condition in conditions {
                if let Some(definitions) = view.task_definitions.get(&condition.task) {
                    // A looped-audio leaf anywhere in a def's tree makes its
                    // natural Finished unreachable: sequences stall at it and
                    // parallels never see all children finished. Ending it by
                    // control or a stop child resolves as cancellation, not
                    // as the awaited milestone.
                    if condition.milestone == Milestone::Finished
                        && definitions.iter().all(|effect| {
                            effect.as_ref().effect_tree_any(&|e| {
                                matches!(e, Effect::Audio { looped: true, .. })
                            })
                        })
                    {
                        return Err(err("E_INFINITE_WAIT", &at, &condition.task));
                    }
                    if let Milestone::Marker(marker) = &condition.milestone {
                        let exists = definitions.iter().any(|effect| match effect.as_ref() {
                            Effect::Dialogue { text, .. } => view
                                .texts
                                .get(text)
                                .is_some_and(|contract| contract.gates.contains(marker)),
                            _ => false,
                        });
                        if !exists {
                            return Err(err("E_MILESTONE", &at, marker));
                        }
                    }
                }
            }
        }
        if let Terminator::Interact {
            choice,
            branches,
            result,
            on_cancel,
            ..
        } = &block.terminator
        {
            if (result.is_some() || on_cancel.is_some())
                && !view.requires.iter().any(|c| c == "story.typed-result.v1")
            {
                return Err(err("E_CAPABILITY", &at, "story.typed-result.v1"));
            }
            if let Some(definition) = view.choices.get(choice) {
                if definition.options.len() != branches.len()
                    || definition
                        .options
                        .iter()
                        .any(|option| !branches.contains_key(&option.id))
                {
                    return Err(err("E_CHOICE", &at, "branch coverage"));
                }
                if let Some(target) = result {
                    let ty = view
                        .variables
                        .get(target)
                        .map(Value::ty)
                        .ok_or_else(|| err("E_VARIABLE", &at, target))?;
                    if definition
                        .options
                        .iter()
                        .any(|option| option.value.as_ref().is_none_or(|v| v.ty() != ty))
                    {
                        return Err(err("E_TYPE", &at, "typed-result option values"));
                    }
                }
            }
        }
        for expr in term_exprs(&block.terminator) {
            expr_type(expr, &vars, &at)?;
        }
    }
    validate_assignments(
        fid,
        function,
        view.variables.keys().chain(function.params.keys()),
    )?;
    Ok(())
}

pub fn expr_type(e: &Expr, vars: &BTreeMap<String, ValueType>, at: &str) -> Result<ValueType> {
    use BinaryOp::*;
    match e {
        Expr::Const { value } => Ok(value.ty()),
        Expr::Var { name } => vars
            .get(name)
            .copied()
            .ok_or_else(|| err("E_VARIABLE", at, name)),
        Expr::ToF80 { value } | Expr::ToI32 { value } => {
            if !matches!(expr_type(value, vars, at)?, ValueType::I32 | ValueType::F80) {
                return Err(err(
                    "E_TYPE",
                    at,
                    "numeric conversion requires numeric input",
                ));
            }
            Ok(if matches!(e, Expr::ToF80 { .. }) {
                ValueType::F80
            } else {
                ValueType::I32
            })
        }
        Expr::Not { value } => {
            if expr_type(value, vars, at)? != ValueType::Bool {
                return Err(err("E_TYPE", at, "not requires Bool"));
            }
            Ok(ValueType::Bool)
        }
        Expr::Binary { op, left, right } => {
            let l = expr_type(left, vars, at)?;
            let r = expr_type(right, vars, at)?;
            let result = match op {
                Add | Sub | Mul | Div | Rem
                    if matches!(l, ValueType::I32 | ValueType::F80) && r == l =>
                {
                    l
                }
                Lt | Le | Gt | Ge if matches!(l, ValueType::I32 | ValueType::F80) && r == l => {
                    ValueType::Bool
                }
                Eq | Ne if l == r => ValueType::Bool,
                And | Or if l == ValueType::Bool && r == l => ValueType::Bool,
                Concat if l == ValueType::String && r == l => ValueType::String,
                _ => return Err(err("E_TYPE", at, "binary operand types disagree")),
            };
            Ok(result)
        }
    }
}
pub(super) fn reads(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Var { name } => {
            out.insert(name.clone());
        }
        Expr::Not { value } | Expr::ToF80 { value } | Expr::ToI32 { value } => reads(value, out),
        Expr::Binary { left, right, .. } => {
            reads(left, out);
            reads(right, out);
        }
        _ => {}
    }
}
pub(super) fn check_reads(
    e: &Expr,
    initialized: &BTreeSet<&str>,
    assigned: &BTreeSet<&str>,
    at: &str,
) -> Result<()> {
    let mut r = BTreeSet::new();
    reads(e, &mut r);
    if let Some(v) = r
        .iter()
        .find(|v| !initialized.contains(v.as_str()) && !assigned.contains(v.as_str()))
    {
        return Err(err("E_UNINITIALIZED", at, v));
    }
    Ok(())
}

/// Forward must-analysis shared by source and runtime validation. Globals and
/// parameters are always initialized, so only local assignment facts flow
/// through the graph. Revisit successors only when their incoming facts change.
pub(super) fn validate_assignments<'a>(
    fid: &str,
    function: &'a Function,
    initialized: impl Iterator<Item = &'a String>,
) -> Result<()> {
    let initialized: BTreeSet<_> = initialized.map(String::as_str).collect();
    let entry = function.entry.as_str();
    let mut incoming = BTreeMap::from([(entry, BTreeSet::<&str>::new())]);
    let mut queue = VecDeque::from([entry]);
    let mut queued = BTreeSet::from([entry]);
    while let Some(bid) = queue.pop_front() {
        queued.remove(bid);
        let block = &function.blocks[bid];
        let mut assigned = incoming[bid].clone();
        for op in &block.ops {
            if let Operation::Assign { target, .. } | Operation::Random { target, .. } =
                &op.operation
            {
                if !initialized.contains(target.as_str()) {
                    assigned.insert(target.as_str());
                }
            }
        }
        if let Terminator::Call {
            result: Some(result),
            ..
        } = &block.terminator
        {
            if !initialized.contains(result.as_str()) {
                assigned.insert(result.as_str());
            }
        }
        for target in outgoing(&block.terminator) {
            let changed = match incoming.entry(target) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(assigned.clone());
                    true
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let old = entry.get_mut();
                    let count = old.len();
                    old.retain(|name| assigned.contains(name));
                    old.len() != count
                }
            };
            if changed && queued.insert(target) {
                queue.push_back(target);
            }
        }
    }
    // Keep deterministic block order and the original diagnostic locations.
    for (bid, block) in &function.blocks {
        if let Some(mut assigned) = incoming.get(bid.as_str()).cloned() {
            for op in &block.ops {
                match &op.operation {
                    Operation::Assign { target, value }
                    | Operation::ProfileValueAssign { target, value, .. } => {
                        check_reads(value, &initialized, &assigned, &op.id)?;
                        if !initialized.contains(target.as_str()) {
                            assigned.insert(target.as_str());
                        }
                    }
                    Operation::Random { target, .. } if !initialized.contains(target.as_str()) => {
                        assigned.insert(target.as_str());
                    }
                    _ => {}
                }
            }
            for expr in term_exprs(&block.terminator) {
                check_reads(expr, &initialized, &assigned, &format!("{fid}/{bid}"))?;
            }
        }
    }
    Ok(())
}
pub(super) fn outgoing(t: &Terminator) -> Vec<&str> {
    match t {
        Terminator::Goto { target } => vec![target],
        Terminator::Branch { yes, no, .. } => vec![yes, no],
        Terminator::Switch { cases, default, .. } => cases
            .values()
            .map(String::as_str)
            .chain(std::iter::once(default.as_str()))
            .collect(),
        Terminator::Call { next, .. } | Terminator::Activate { next, .. } => vec![next],
        Terminator::Await {
            next,
            on_cancelled,
            on_failed,
            on_advance,
            ..
        } => [next.as_str(), on_cancelled.as_str(), on_failed.as_str()]
            .into_iter()
            .chain(on_advance.iter().map(String::as_str))
            .collect(),
        Terminator::Interact {
            branches,
            on_empty,
            on_cancel,
            ..
        } => branches
            .values()
            .map(String::as_str)
            .chain(std::iter::once(on_empty.as_str()))
            .chain(on_cancel.iter().map(String::as_str))
            .collect(),
        _ => vec![],
    }
}
pub(super) fn term_exprs(t: &Terminator) -> Vec<&Expr> {
    match t {
        Terminator::Branch { condition, .. } => vec![condition],
        Terminator::Switch { value, .. } => vec![value],
        Terminator::Call { args, .. } => args.values().collect(),
        Terminator::Return { value } => value.iter().collect(),
        _ => vec![],
    }
}
