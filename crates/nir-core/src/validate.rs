use nir_format::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) enum Instruction {
    Op(Op),
    Term(Terminator),
}
#[derive(Debug, Clone)]
pub struct ValidatedProgram {
    program: Arc<Program>,
    instructions: Arc<Vec<Instruction>>,
    block_offsets: Arc<BTreeMap<String, BTreeMap<String, usize>>>,
}
impl ValidatedProgram {
    pub fn new(p: Program) -> Result<Self> {
        validate(&p)?;
        let mut instructions = vec![];
        let mut block_offsets = BTreeMap::new();
        for (fid, f) in &p.functions {
            let mut blocks = BTreeMap::new();
            for (bid, b) in &f.blocks {
                blocks.insert(bid.clone(), instructions.len());
                instructions.extend(b.ops.iter().cloned().map(Instruction::Op));
                instructions.push(Instruction::Term(b.terminator.clone()));
            }
            block_offsets.insert(fid.clone(), blocks);
        }
        Ok(Self {
            program: Arc::new(p),
            instructions: Arc::new(instructions),
            block_offsets: Arc::new(block_offsets),
        })
    }
    pub fn program(&self) -> &Program {
        &self.program
    }
    pub(crate) fn instruction(&self, function: &str, block: &str, op: usize) -> &Instruction {
        &self.instructions[self.block_offsets[function][block] + op]
    }
}
fn err(code: &str, at: &str, message: &str) -> Diagnostic {
    Diagnostic::new(code, at, message)
}
pub fn expr_type(e: &Expr, vars: &BTreeMap<String, ValueType>, at: &str) -> Result<ValueType> {
    use BinaryOp::*;
    match e {
        Expr::Const { value } => Ok(value.ty()),
        Expr::Var { name } => vars
            .get(name)
            .copied()
            .ok_or_else(|| err("E_VARIABLE", at, name)),
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
                Add | Sub | Mul | Div | Rem if l == ValueType::I32 && r == l => ValueType::I32,
                Lt | Le | Gt | Ge if l == ValueType::I32 && r == l => ValueType::Bool,
                Eq | Ne if l == r => ValueType::Bool,
                And | Or if l == ValueType::Bool && r == l => ValueType::Bool,
                Concat if l == ValueType::String && r == l => ValueType::String,
                _ => return Err(err("E_TYPE", at, "binary operand types disagree")),
            };
            Ok(result)
        }
    }
}
fn reads(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Var { name } => {
            out.insert(name.clone());
        }
        Expr::Not { value } => reads(value, out),
        Expr::Binary { left, right, .. } => {
            reads(left, out);
            reads(right, out);
        }
        _ => {}
    }
}
fn check_reads(e: &Expr, assigned: &BTreeSet<String>, at: &str) -> Result<()> {
    let mut r = BTreeSet::new();
    reads(e, &mut r);
    if let Some(v) = r.difference(assigned).next() {
        return Err(err("E_UNINITIALIZED", at, v));
    }
    Ok(())
}
fn outgoing(t: &Terminator) -> Vec<&str> {
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
            ..
        } => vec![next, on_cancelled, on_failed],
        Terminator::Interact {
            branches, on_empty, ..
        } => branches
            .values()
            .map(String::as_str)
            .chain(std::iter::once(on_empty.as_str()))
            .collect(),
        _ => vec![],
    }
}
fn term_exprs(t: &Terminator) -> Vec<&Expr> {
    match t {
        Terminator::Branch { condition, .. } => vec![condition],
        Terminator::Switch { value, .. } => vec![value],
        Terminator::Call { args, .. } => args.values().collect(),
        Terminator::Return { value } => value.iter().collect(),
        _ => vec![],
    }
}
fn validate(p: &Program) -> Result<()> {
    if p.format != FORMAT_VERSION {
        return Err(err("E_VERSION", "program", "unsupported semantic version"));
    }
    if p.functions.len() > 4096 || p.texts.len() > 100_000 || p.assets.len() > 10_000 {
        return Err(err("E_LIMIT", "program", "table limit"));
    }
    for cap in &p.requires {
        if !CAPABILITIES.contains(&cap.as_str()) {
            return Err(err("E_CAPABILITY", "requires", cap));
        }
    }
    if p.game_id.is_empty()
        || !p.functions.contains_key(&p.entry)
        || p.stage.width == 0
        || p.stage.height == 0
        || p.stage.width > 8192
        || p.stage.height > 8192
    {
        return Err(err("E_PROGRAM", "program", "invalid game, entry or stage"));
    }
    if !p.functions[&p.entry].params.is_empty() {
        return Err(err("E_CALL", "entry", "entry requires arguments"));
    }
    if !p.locales.contains_key(&p.default_locale) {
        return Err(err("E_LOCALE", "program", "missing default locale"));
    }
    for (locale, texts) in &p.locales {
        if locale != "zh-Hans" && locale != "en" {
            return Err(err(
                "E_CAPABILITY",
                locale,
                "locale is not in the tested language profile",
            ));
        }
        for (id, c) in &p.texts {
            let d = texts
                .get(id)
                .ok_or_else(|| err("E_TRANSLATION", locale, id))?;
            if d.revision != c.revision {
                return Err(err("E_TEXT_REVISION", id, locale));
            }
            let mut ids = BTreeSet::new();
            let mut gates = vec![];
            for span in &d.spans {
                let sid = match span {
                    Span::Text { id, text, .. } => {
                        if text.len() > 128 * 1024 {
                            return Err(err("E_LIMIT", id, "text too long"));
                        }
                        id
                    }
                    Span::Break { id } => id,
                    Span::Gate { id } => {
                        gates.push(id.clone());
                        id
                    }
                    Span::Param { id, name } => {
                        if !c.params.contains_key(name) {
                            return Err(err("E_TEXT_PARAM", id, name));
                        }
                        id
                    }
                };
                if !ids.insert(sid) {
                    return Err(err("E_DUPLICATE", id, "span identity"));
                }
            }
            if gates != c.gates {
                return Err(err("E_GATE", id, "gate order/count mismatch"));
            }
            for (param, ty) in &c.params {
                if p.variables.get(param).map(Value::ty) != Some(*ty) {
                    return Err(err("E_TEXT_PARAM", id, param));
                }
            }
        }
        if texts.keys().any(|id| !p.texts.contains_key(id)) {
            return Err(err("E_TEXT_CONTRACT", locale, "unexpected text"));
        }
    }
    for (id, a) in &p.assets {
        if a.bytes > MAX_INPUT_BYTES as u64 * 16 || a.width > 8192 || a.height > 8192 {
            return Err(err("E_LIMIT", id, "asset too large"));
        }
    }
    for (id, nodes) in &p.scenes {
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
    for (id, cue) in &p.cues {
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
        let mut names = BTreeSet::new();
        let mut writers = BTreeSet::new();
        let mut stage_count = 0;
        let mut dialogue_count = 0;
        for def in &cue.effects {
            if !names.insert(&def.id) {
                return Err(err("E_DUPLICATE", id, &def.id));
            }
            task_defs.entry(&def.id).or_default().push(&def.effect);
            match &def.effect {
                Effect::StagePresent { scene, .. } => {
                    stage_count += 1;
                    if !p.scenes.contains_key(scene) {
                        return Err(err("E_SCENE", id, scene));
                    }
                }
                Effect::Dialogue { text, speaker, .. } => {
                    dialogue_count += 1;
                    if !p.texts.contains_key(text)
                        || (!speaker.is_empty() && !p.texts.contains_key(speaker))
                    {
                        return Err(err("E_TEXT", id, text));
                    }
                }
                Effect::Audio { asset, .. }
                    if p.assets.get(asset).map(|a| a.kind) != Some(AssetKind::Audio) =>
                {
                    return Err(err("E_ASSET_TYPE", id, asset));
                }
                Effect::Clip {
                    node, property, to, ..
                } => {
                    if !to.is_finite()
                        || (*property == Property::Opacity && !(0.0..=1.0).contains(to))
                        || (*property == Property::Scale && *to < 0.)
                    {
                        return Err(err("E_VISUAL", id, node));
                    }
                    if !writers.insert((node, property)) {
                        return Err(err("E_OWNERSHIP", id, node));
                    }
                }
                _ => {}
            }
        }
        if stage_count > 1 || dialogue_count > 1 {
            return Err(err("E_CUE", id, "at most one stage and dialogue per cue"));
        }
    }
    for (id, c) in &p.choices {
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
    for (fid, f) in &p.functions {
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
                match &op.operation {
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
                    if ty == ValueType::Bool
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
                        .functions
                        .get(function)
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
                Terminator::Await { conditions, .. } => {
                    if conditions.is_empty() {
                        return Err(err("E_WAIT", &at, "empty All"));
                    }
                    for c in conditions {
                        let defs = task_defs
                            .get(c.task.as_str())
                            .ok_or_else(|| err("E_TASK", &at, &c.task))?;
                        if c.milestone == Milestone::Finished
                            && defs
                                .iter()
                                .all(|e| matches!(e, Effect::Audio { looped: true, .. }))
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
                    choice, branches, ..
                } => {
                    let c = p
                        .choices
                        .get(choice)
                        .ok_or_else(|| err("E_CHOICE", &at, choice))?;
                    if c.options.len() != branches.len()
                        || c.options.iter().any(|o| !branches.contains_key(&o.id))
                    {
                        return Err(err("E_CHOICE", &at, "branch coverage"));
                    }
                }
                _ => {}
            }
        }
        // Forward must-analysis: entry facts flow until loop joins reach a fixed point.
        let initial: BTreeSet<_> = p.variables.keys().chain(f.params.keys()).cloned().collect();
        let mut incoming = BTreeMap::from([(f.entry.clone(), initial)]);
        let mut changed = true;
        while changed {
            changed = false;
            for (bid, b) in &f.blocks {
                let Some(mut assigned) = incoming.get(bid).cloned() else {
                    continue;
                };
                for op in &b.ops {
                    match &op.operation {
                        Operation::Assign { target, .. } | Operation::Random { target, .. } => {
                            assigned.insert(target.clone());
                        }
                        _ => {}
                    }
                }
                if let Terminator::Call {
                    result: Some(r), ..
                } = &b.terminator
                {
                    assigned.insert(r.clone());
                }
                for next in outgoing(&b.terminator) {
                    let merged = match incoming.get(next) {
                        Some(old) => old.intersection(&assigned).cloned().collect(),
                        None => assigned.clone(),
                    };
                    if incoming.get(next) != Some(&merged) {
                        incoming.insert(next.to_owned(), merged);
                        changed = true;
                    }
                }
            }
        }
        for (bid, b) in &f.blocks {
            if let Some(mut assigned) = incoming.get(bid).cloned() {
                for op in &b.ops {
                    match &op.operation {
                        Operation::Assign { target, value } => {
                            check_reads(value, &assigned, &op.id)?;
                            assigned.insert(target.clone());
                        }
                        Operation::Random { target, .. } => {
                            assigned.insert(target.clone());
                        }
                        _ => {}
                    }
                }
                for e in term_exprs(&b.terminator) {
                    check_reads(e, &assigned, &format!("{fid}/{bid}"))?;
                }
            }
        }
    }
    Ok(())
}
