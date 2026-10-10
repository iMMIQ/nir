//! Stock choice wrappers and pure game-state expressions. Source arrays and
//! UI callbacks are matched at compile time; the player receives typed NIR
//! interactions and variables, never a source interpreter.
use super::super::lsb::Command;
use super::super::ui_expr::{self, Op, Term};
use super::*;
use nir_format::ValueType;

const PREVIEW_ROOT: &str = "__nir_lm_preview_root";

fn preview_base(nodes: &BTreeMap<String, Node>) -> Result<BTreeMap<String, Node>> {
    if let Some(root) = nodes.get(PREVIEW_ROOT) {
        ensure!(
            root.parent.is_none()
                && root.asset.is_none()
                && root.color == [0.; 4]
                && root.timeline_binding.is_none()
                && root.bitmap_text.is_none(),
            "E_IMPORT_IMAGE_CHOICE: internal root collision"
        );
    }
    let mut removed = BTreeSet::from([PREVIEW_ROOT.to_owned()]);
    loop {
        let before = removed.len();
        for node in nodes.values() {
            if node.parent.as_ref().is_some_and(|p| removed.contains(p)) {
                removed.insert(node.id.clone());
            }
        }
        if removed.len() == before {
            break;
        }
    }
    Ok(nodes
        .iter()
        .filter(|(id, _)| !removed.contains(*id))
        .map(|(id, node)| (id.clone(), node.clone()))
        .collect())
}

fn preview_output_ops(adapter: &mut Adapter, index: i32, cancel: bool) -> Result<Vec<Value>> {
    let mut ops = vec![];
    for (target, literal) in [
        (
            CHOICE_RESULT,
            cancel.then(|| Literal::String(String::new())),
        ),
        ("選択番号", Some(Literal::Int(index))),
        ("最終選択値", Some(Literal::Variable(CHOICE_RESULT.into()))),
        ("最終選択番号", Some(Literal::Variable("選択番号".into()))),
    ] {
        if let Some(literal) = literal {
            let source = Expression {
                literal: None,
                operations: vec![(1, target.into(), vec![literal])],
                functions: BTreeMap::new(),
            };
            let operation = assignment(adapter, &source)?;
            ops.push(json!({"id":adapter.id("preview_output"),"operation":operation}));
        }
    }
    Ok(ops)
}

fn preview_close(adapter: &mut Adapter, click: &str, duration: u64) -> Result<String> {
    let mut blocks = vec![];
    if !click.is_empty() {
        let sound = adapter.sound(click, 1.)?;
        adapter.play_sound(
            &mut blocks,
            "se",
            json!({"type":"audio","bus":"sfx","asset":sound,"looped":false,"gain":1.}),
        );
    }
    adapter.fade_stop(&mut blocks, "voice", 0);
    adapter.scene(&mut blocks, duration);
    if !click.is_empty() {
        adapter.wait(&mut blocks, "se", json!({"type":"finished"}));
        adapter.audio.remove("se");
    }
    let function = adapter.id("image_choice_close");
    adapter.finish_function(&function, blocks, json!({"type":"return"}));
    Ok(function)
}

/// Source control flow is encoded by indentation. A successful arm falls
/// through past all following sibling Elseif/Else arms, including an outer
/// arm when the inner conditional is its final statement.
pub(super) fn successor(script: &Script, pc: usize) -> Result<usize> {
    skip_sibling_arms(script, pc, pc + 1)
}

fn skip_sibling_arms(script: &Script, pc: usize, mut next: usize) -> Result<usize> {
    while let Some(arm) = script
        .commands
        .get(next)
        .filter(|c| matches!(c.kind, 1 | 2))
    {
        ensure!(
            script.commands[pc].indent >= arm.indent,
            "E_IMPORT_CONDITION: invalid arm fallthrough"
        );
        verify_arm(script, next)?;
        let depth = arm.indent;
        next += 1;
        while script.commands.get(next).is_some_and(|c| c.indent > depth) {
            next += 1;
        }
    }
    Ok(next)
}

fn verify_arm(script: &Script, pc: usize) -> Result<()> {
    let arm = &script.commands[pc];
    let previous = script.commands[..pc]
        .iter()
        .rev()
        .find(|c| c.indent <= arm.indent)
        .context("E_IMPORT_CONDITION: orphan arm")?;
    ensure!(
        previous.indent == arm.indent && matches!(previous.kind, 0 | 1),
        "E_IMPORT_CONDITION: arm does not follow If/Elseif"
    );
    Ok(())
}

pub(super) fn condition_targets(script: &Script, pc: usize) -> Result<(usize, usize)> {
    let command = &script.commands[pc];
    ensure!(
        matches!(command.kind, 0 | 1),
        "E_IMPORT_CONDITION: invalid condition kind"
    );
    if command.kind == 1 {
        verify_arm(script, pc)?;
    }
    let yes = successor(script, pc)?;
    let mut no = pc + 1;
    while script
        .commands
        .get(no)
        .is_some_and(|c| c.indent > command.indent)
    {
        no += 1;
    }
    if let Some(arm) = script.commands.get(no).filter(|c| matches!(c.kind, 1 | 2)) {
        if arm.indent < command.indent {
            no = skip_sibling_arms(script, pc, no)?;
        } else {
            ensure!(
                arm.indent == command.indent,
                "E_IMPORT_CONDITION: invalid arm depth"
            );
            verify_arm(script, no)?;
            if arm.kind == 2 {
                no = successor(script, no)?;
            }
        }
    }
    Ok((yes, no))
}

pub(super) fn expression(adapter: &Adapter, term: &Term) -> Result<(Value, ValueType)> {
    let constant = |value: Value, ty| (json!({"type":"const","value":value}), ty);
    Ok(match term {
        Term::Int { value } => constant(json!({"type":"i32","value":value}), ValueType::I32),
        Term::Float { value } => constant(json!({"type":"f80","value":value}), ValueType::F80),
        Term::String { value } => {
            constant(json!({"type":"string","value":value}), ValueType::String)
        }
        Term::Read { name } => {
            ensure!(
                !adapter.unsupported_status.contains(name),
                "E_IMPORT_GAME_VAR: unsupported persistent value {name}"
            );
            let value = adapter
                .variables
                .get(name)
                .with_context(|| format!("E_IMPORT_GAME_VAR: undeclared {name}"))?;
            let ty = match value["type"].as_str() {
                Some("i32") => ValueType::I32,
                Some("f80") => ValueType::F80,
                Some("string") => ValueType::String,
                _ => bail!("E_IMPORT_GAME_VAR: unsupported type"),
            };
            (json!({"type":"var","name":name}), ty)
        }
        Term::Apply {
            op: op @ (Op::ToInt | Op::ToFloat),
            args,
        } if args.len() == 1 => {
            let (value, ty) = expression(adapter, &args[0])?;
            ensure!(
                matches!(ty, ValueType::I32 | ValueType::F80),
                "E_IMPORT_GAME_EXPR: nonnumeric conversion"
            );
            (
                json!({"type":if *op == Op::ToInt { "to_i32" } else { "to_f80" }, "value":value}),
                if *op == Op::ToInt {
                    ValueType::I32
                } else {
                    ValueType::F80
                },
            )
        }
        Term::Apply { op: Op::Not, args } if args.len() == 1 => (
            json!({"type":"not","value":condition(adapter, &args[0])?}),
            ValueType::Bool,
        ),
        Term::Apply { op, args } if args.len() == 2 => {
            if matches!(op, Op::SourceAnd | Op::SourceOr) {
                (
                    json!({"type":"binary","op":if *op==Op::SourceAnd {"and"}else{"or"},
                    "left":condition(adapter,&args[0])?,"right":condition(adapter,&args[1])?}),
                    ValueType::Bool,
                )
            } else {
                let (mut left, mut lt) = expression(adapter, &args[0])?;
                let (mut right, mut rt) = expression(adapter, &args[1])?;
                if matches!(
                    (lt, rt),
                    (ValueType::I32, ValueType::F80) | (ValueType::F80, ValueType::I32)
                ) {
                    if lt == ValueType::I32 {
                        left = json!({"type":"to_f80","value":left});
                        lt = ValueType::F80;
                    }
                    if rt == ValueType::I32 {
                        right = json!({"type":"to_f80","value":right});
                        rt = ValueType::F80;
                    }
                }
                ensure!(lt == rt, "E_IMPORT_GAME_EXPR: operand types differ");
                let (op, ty) = match op {
                    Op::Add => ("add", lt),
                    Op::Subtract => ("sub", lt),
                    Op::Multiply => ("mul", lt),
                    Op::Divide => ("div", lt),
                    Op::Remainder => ("rem", lt),
                    Op::Concat => ("concat", ValueType::String),
                    Op::Equal => ("eq", ValueType::Bool),
                    Op::NotEqual => ("ne", ValueType::Bool),
                    Op::Greater => ("gt", ValueType::Bool),
                    Op::Less => ("lt", ValueType::Bool),
                    Op::GreaterEqual => ("ge", ValueType::Bool),
                    Op::LessEqual => ("le", ValueType::Bool),
                    _ => bail!("E_IMPORT_GAME_EXPR: unsupported pure operation"),
                };
                ensure!(
                    matches!(op, "eq" | "ne")
                        || (op == "concat" && lt == ValueType::String)
                        || (op != "concat" && matches!(lt, ValueType::I32 | ValueType::F80)),
                    "E_IMPORT_GAME_EXPR: unsupported operands"
                );
                (
                    json!({"type":"binary","op":op,"left":left,"right":right}),
                    ty,
                )
            }
        }
        _ => bail!("E_IMPORT_GAME_EXPR: unsupported source operation"),
    })
}
pub(super) fn condition(adapter: &Adapter, term: &Term) -> Result<Value> {
    let (value, ty) = expression(adapter, term)?;
    Ok(match ty {
        ValueType::Bool => value,
        ValueType::I32 | ValueType::F80 => {
            let zero = if ty == ValueType::F80 {
                json!({"type":"f80","value":nir_format::Float80::from_i32(0)})
            } else {
                json!({"type":"i32","value":0})
            };
            json!({"type":"binary","op":"ne","left":value,"right":{"type":"const","value":zero}})
        }
        _ => bail!("E_IMPORT_GAME_EXPR: condition must be numeric"),
    })
}
pub(super) fn assignment(adapter: &mut Adapter, source: &Expression) -> Result<Value> {
    if source.functions.values().any(|function| *function == 26) {
        return random_assignment(adapter, source);
    }
    let (name, term) = ui_expr::assignment(source)?;
    // Source declaration type3 is a flag. Preserve only its two authored
    // textual constants; ordinary integer/string variables keep strict typing.
    let term = if adapter.boolean_variables.contains(&name) {
        match term {
            Term::String { value } if value == "TRUE" => Term::Int { value: 1 },
            Term::String { value } if value == "FALSE" => Term::Int { value: 0 },
            other => other,
        }
    } else {
        term
    };
    ensure!(
        !adapter.unsupported_status.contains(&name),
        "E_IMPORT_GAME_VAR: unsupported persistent write {name}"
    );
    if let Some(key) = adapter.status_flags.get(&name) {
        ensure!(
            term == Term::Int { value: 1 },
            "E_IMPORT_GAME_VAR: persistent flags must be monotonic {name}"
        );
        return Ok(json!({"type":"profile_merge","key":key}));
    }
    if let Some(key) = adapter.status_values.get(&name).cloned() {
        let (value, ty) = expression(adapter, &term)?;
        let (value, ty) = numeric::coerce(adapter, &name, value, ty);
        ensure!(
            adapter.variables[&name]["type"]
                == match ty {
                    ValueType::I32 => "i32",
                    ValueType::F80 => "f80",
                    ValueType::String => "string",
                    _ => bail!("E_IMPORT_GAME_VAR: unsupported persistent assignment type"),
                },
            "E_IMPORT_GAME_VAR: persistent assignment changes type"
        );
        return Ok(json!({"type":"profile_value_assign","target":name,"key":key,"value":value}));
    }
    ensure!(
        !name.starts_with('@') && !name.starts_with("_tmp"),
        "E_IMPORT_GAME_VAR: system/temporary write"
    );
    let (value, ty) = expression(adapter, &term)?;
    let (value, ty) = numeric::coerce(adapter, &name, value, ty);
    let initial = match ty {
        ValueType::I32 => json!({"type":"i32","value":0}),
        ValueType::F80 => json!({"type":"f80","value":nir_format::Float80::from_i32(0)}),
        ValueType::String => json!({"type":"string","value":""}),
        _ => bail!("E_IMPORT_GAME_VAR: assignment needs numeric coercion"),
    };
    let declared = adapter
        .variables
        .entry(name.clone())
        .or_insert(initial.clone());
    ensure!(
        declared["type"] == initial["type"],
        "E_IMPORT_GAME_VAR: assignment changes type"
    );
    Ok(json!({"type":"assign","target":name,"value":value}))
}

/// This source page owns an unused local flag: its only occurrences are the
/// existence-guarded declaration and this terminal cleanup. The declaration
/// lies outside the imported new-game entry, so no NIR storage exists to erase.
/// Keep the certificate narrow; deleting live/global variables is unsupported.
pub(super) fn dead_local_cleanup(
    adapter: &mut Adapter,
    script: &Script,
    pc: usize,
    name: &str,
) -> Result<()> {
    ensure!(
        script.version == 117
            && script.source_sha256
                == "7e16261286d74e40ec0ef71551b8076670bac16cb6949edc59e511ccce0e77c4"
            && matches!(&script.commands[pc].body, Body::VariableDelete(deleted) if deleted == name)
            && script.commands[pc].line == 4
            && matches!(script.commands.get(pc + 1), Some(c)
                if !c.muted && c.indent == 0 && matches!(&c.body, Body::Exit(e) if e.flag().ok() == Some(true)))
            && !adapter.variables.contains_key(name)
            && !adapter.status_flags.contains_key(name)
            && !adapter.status_values.contains_key(name),
        "E_IMPORT_GAME_VAR_DELETE: deletion of a live or uncertified variable {name}"
    );
    // Also reject any later admitted route trying to make the erased source
    // local observable, rather than silently recreating it as a NIR global.
    adapter.unsupported_status.insert(name.to_owned());
    Ok(())
}

/// Function 26 dispatches to the source RTL integer Random: unsigned
/// multiply-high by the positive bound, returning 0..bound-1. NIR keeps that
/// range with its captured, unbiased RNG; source seed sequences are not copied.
/// This effectful write is deliberately separate from pure UI expressions.
fn random_assignment(adapter: &mut Adapter, source: &Expression) -> Result<Value> {
    let [(call, temporary, args), (write, name, value)] = source.operations.as_slice() else {
        bail!("E_IMPORT_RANDOM: expected one bounded draw and final assignment");
    };
    ensure!(
        source.literal.is_none()
            && source.functions == BTreeMap::from([(0, 26)])
            && *call == 11
            && ui_expr::temporary(temporary)
            && *write == 1
            && matches!(value.as_slice(), [Literal::Variable(result)] if result == temporary),
        "E_IMPORT_RANDOM: unsupported draw expression"
    );
    let [Literal::Int(bound)] = args.as_slice() else {
        bail!("E_IMPORT_RANDOM: positive literal integer bound required");
    };
    ensure!(*bound > 0, "E_IMPORT_RANDOM: positive bound required");
    ensure!(
        !ui_expr::temporary(name)
            && !name.starts_with('@')
            && !name.starts_with("_tmp")
            && name.len() <= 1024
            && !adapter.unsupported_status.contains(name)
            && !adapter.status_flags.contains_key(name)
            && !adapter.status_values.contains_key(name),
        "E_IMPORT_RANDOM: ordinary game variable required"
    );
    let declared = adapter
        .variables
        .entry(name.clone())
        .or_insert_with(|| json!({"type":"i32","value":0}));
    ensure!(
        declared["type"] == "i32",
        "E_IMPORT_RANDOM: integer destination required"
    );
    adapter.warnings.insert(
        "Authored bounded Random uses NIR's saved RNG and retains the source range; original engine seeds and random sequences are not reproduced.".into(),
    );
    Ok(json!({"type":"random","target":name,"min":0,"max":bound-1}))
}

fn append_literal(e: &Expression) -> Result<Option<String>> {
    let Some((&index, _)) = e.functions.iter().find(|(_, f)| **f == 66) else {
        return Ok(None);
    };
    ensure!(
        e.functions.len() == 1,
        "E_IMPORT_CHOICE: nested append function"
    );
    let (op, destination, args) = &e.operations[index];
    ensure!(
        *op == 11
            && args.len() == 2
            && matches!(&args[0], Literal::Variable(name) if name == "_tmp")
            && ui_expr::temporary(destination),
        "E_IMPORT_CHOICE: unknown option array write"
    );
    ensure!(
        e.operations[index + 1..]
            .iter()
            .all(|(op, d, args)| *op == 1
                && ui_expr::temporary(d)
                && matches!(args.as_slice(),[Literal::Variable(v)]if v==destination)),
        "E_IMPORT_CHOICE: effects after append"
    );
    let mut value = e.clone();
    value.operations.truncate(index);
    value.functions.clear();
    value
        .operations
        .push((1, "____arg".into(), vec![args[1].clone()]));
    let Some(Term::String { value }) = ui_expr::normalize(&value)? else {
        bail!("E_IMPORT_CHOICE: dynamic option text");
    };
    ensure!(
        !value.is_empty() && value.len() <= 16 * 1024,
        "E_IMPORT_CHOICE: invalid option text"
    );
    Ok(Some(value))
}

/// A bounded standard inline text-choice wrapper. Cleanup is matched by the
/// three declared temporary names, so unrelated authored commands are never
/// swallowed while locating the following dispatch.
pub(super) fn inline_choice(
    adapter: &mut Adapter,
    script: &Script,
    page: &str,
    pc: usize,
    id: &str,
    path: &RoutePath,
    routes: &mut Routes,
) -> Result<bool> {
    let start = &script.commands[pc];
    if !matches!(&start.body, Body::Wait(e) if e.len()==3 && references(&e[0],"選択実行中")) {
        return Ok(false);
    }
    let end = (pc + 1..script.commands.len().min(pc + 256))
        .find(|i| matches!(&script.commands[*i].body,Body::VariableDelete(name)if name=="_tmpid" || name=="_tmp3"));
    let Some(end) = end else {
        return Ok(false);
    };
    // Do not consume the next choice wrapper across a chart boundary. Numeric
    // input uses the same opening wait, then jumps before any choice temporaries.
    if script.commands[pc + 1..=end]
        .iter()
        .any(|c| !c.muted && matches!(c.kind, 3 | 4 | 6))
    {
        return Ok(false);
    }
    if matches!(&script.commands[end].body,Body::VariableDelete(name)if name=="_tmp3") {
        return Ok(false);
    }
    let body = &script.commands[pc..=end];
    let calls: Vec<_> = body
        .iter()
        .filter_map(|c| match &c.body {
            Body::Call {
                target,
                condition,
                params,
                ..
            } if target.page.replace('\\', "/") == CHOICE_EXECUTOR => Some((c, condition, params)),
            _ => None,
        })
        .collect();
    if calls.is_empty() {
        return Ok(false);
    }
    ensure!(
        calls.len() == 1 && calls[0].1.flag()? && calls[0].2.len() == 8,
        "E_IMPORT_CHOICE: unexpected stock executor"
    );
    // The source countdown flag is false (no timer); style/alignment and
    // sounds are covered by the declared stock-choice UI adaptation.
    ensure!(
        literal_int(&calls[0].2[4])? == 0,
        "E_IMPORT_CHOICE: countdown needs adaptation"
    );
    let mut flow = super::super::ui_flow::Flow::default();
    let mut options = vec![];
    let mut ordinal = 0i32;
    let mut declared = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    for (offset, c) in body.iter().enumerate() {
        if c.muted {
            continue;
        }
        flow.enter(c, pc + offset)?;
        match &c.body {
            Body::Variable {
                name,
                value_type,
                initial,
                scope,
            } => {
                ensure!(
                    matches!(name.as_str(), "_tmp" | "_tmpno" | "_tmpid")
                        && *scope == 2
                        && initial.operations.is_empty()
                        && *value_type == if name == "_tmpno" { 1 } else { 4 },
                    "E_IMPORT_CHOICE: temporary declaration changed"
                );
                ensure!(
                    declared.insert(name.clone()),
                    "E_IMPORT_CHOICE: duplicate temporary"
                );
            }
            Body::VariableDelete(name) => {
                ensure!(
                    declared.contains(name) && deleted.insert(name.clone()),
                    "E_IMPORT_CHOICE: unexpected cleanup"
                );
            }
            Body::Calc(e) => {
                if let Some(text) = append_literal(e)? {
                    let mut visible = json!({"type":"const","value":{"type":"bool","value":true}});
                    for guard in flow.guards() {
                        ensure!(
                            !guard.muted && guard.loop_target.is_none(),
                            "E_IMPORT_CHOICE: unsupported option scope"
                        );
                        let term = ui_expr::normalize(&guard.source)?
                            .context("E_IMPORT_CHOICE: empty guard")?;
                        let mut predicate = condition(adapter, &term)?;
                        if !guard.expected {
                            predicate = json!({"type":"not","value":predicate});
                        }
                        visible =
                            json!({"type":"binary","op":"and","left":visible,"right":predicate});
                    }
                    options.push((text, visible, ordinal));
                    ordinal += 1;
                } else {
                    ensure!(
                        e.functions
                            .values()
                            .all(|f| matches!(*f, 28 | 29 | 133 | 165)),
                        "E_IMPORT_CHOICE: unknown wrapper function"
                    );
                    ensure!(
                        e.operations.iter().all(|(_, d, _)| ui_expr::temporary(d)
                            || matches!(
                                d.as_str(),
                                "_tmpno"
                                    | "_tmpid"
                                    | "選択実行中"
                                    | "選択番号"
                                    | "選択値"
                                    | "右クリックメニュー一時禁止"
                            )),
                        "E_IMPORT_CHOICE: unknown wrapper write"
                    );
                    if let Ok((name, Term::Int { value })) = ui_expr::assignment(e) {
                        if name == "_tmpno" {
                            ensure!(
                                value == ordinal - 1,
                                "E_IMPORT_CHOICE: nonsequential source ordinal"
                            );
                        }
                    }
                }
            }
            Body::Condition(e) => {
                ensure!(
                    e.functions.values().all(|f| matches!(*f, 20 | 28)),
                    "E_IMPORT_CHOICE: wrapper condition call"
                );
            }
            Body::Call {
                target,
                condition,
                params,
                ..
            } => {
                let target = target.page.replace('\\', "/");
                ensure!(
                    condition.flag()?
                        && (target == CHOICE_EXECUTOR
                            || (target == "ノベルシステム/メッセージボックス/終了.lsb"
                                && params.is_empty())),
                    "E_IMPORT_CHOICE: wrapper call changed"
                );
            }
            Body::Wait(e) => {
                ensure!(
                    e.len() == 3
                        && literal_int(&e[1])? == 0
                        && literal_int(&e[2])? == 0
                        && e[0].functions.values().all(|f| matches!(*f, 2 | 20))
                        && e[0].operations.iter().any(|(_, _, args)| args.iter().any(
                            |v| matches!(v,Literal::String(name)if name=="メッセージボックス")
                        )),
                    "E_IMPORT_CHOICE: wrapper wait changed"
                );
            }
            Body::Delete(e) => {
                ensure!(
                    literal_string(e)? == "VOICE",
                    "E_IMPORT_CHOICE: wrapper deletes another object"
                );
            }
            Body::Other if c.kind == 2 || c.kind == 27 => {}
            _ => bail!(
                "E_IMPORT_CHOICE: unsupported wrapper {}:{}",
                adapter.location.source,
                c.line
            ),
        }
    }
    ensure!(
        declared == deleted && declared.len() == 3 && (1..=64).contains(&options.len()),
        "E_IMPORT_CHOICE: incomplete bounded wrapper"
    );
    let mut cleanup_blocks = vec![];
    adapter.fade_stop(&mut cleanup_blocks, "voice", 0);
    let cleanup = adapter.id("choice_cleanup");
    adapter.finish_function(&cleanup, cleanup_blocks, json!({"type":"return"}));
    let after = successor(script, end)?;
    // A finite, guaranteed nonempty choice commits one of these exact strings.
    // When the following pure dispatch covers every string, lower the winning
    // jump directly instead of admitting its impossible all-false fallthrough.
    let direct = choice_chain(script, after)?.and_then(|(dispatch, end)| {
        let strict = script.commands[after..end].iter().all(|c| {
            !c.muted
                && c.indent == 0
                && matches!(&c.body, Body::Jump(_, condition)
                if ui_expr::normalize(condition).ok().flatten().is_some_and(|term| matches!(term,
                    Term::Apply { op:Op::Equal, args } if matches!(args.as_slice(),
                        [Term::Read { name }, Term::String { .. }] if name == CHOICE_RESULT))))
        });
        let nonempty = options.iter().any(|(_, visible, _)| {
            *visible == json!({"type":"const","value":{"type":"bool","value":true}})
        });
        let covered = options
            .iter()
            .all(|(label, _, _)| dispatch.iter().any(|(name, _)| name == label));
        if !strict || !nonempty || !covered {
            return None;
        }
        let mut targets = BTreeMap::new();
        for (label, target) in dispatch {
            targets.entry(label).or_insert(target);
        }
        Some(targets)
    });
    let next = if direct.is_none() {
        let next = adapter.route_block(script, page, after, routes)?;
        adapter.enqueue_route(script, page, after, path.clone(), routes)?;
        Some(next)
    } else {
        None
    };
    let mut definitions = vec![];
    let mut branches = BTreeMap::new();
    adapter
        .variables
        .entry(CHOICE_RESULT.into())
        .or_insert(json!({"type":"string","value":""}));
    adapter
        .variables
        .entry("選択番号".into())
        .or_insert(json!({"type":"i32","value":-1}));
    for (index, (label, visible, ordinal)) in options.into_iter().enumerate() {
        let destination = if let Some(targets) = &direct {
            let (destination, target_script, pc) =
                adapter.route_target(page, &targets[&label], routes)?;
            let target = adapter.route_block(&target_script, &destination, pc, routes)?;
            adapter.enqueue_route(&target_script, &destination, pc, path.clone(), routes)?;
            target
        } else {
            next.as_ref().unwrap().clone()
        };
        let option = format!("o{index}");
        let text = adapter.intern_text(
            "inline_choice",
            index,
            crate::AuthorTextDoc {
                source_revision: 1,
                contract_revision: 1,
                spans: vec![Span::Text {
                    id: "s0".into(),
                    text: label.clone(),
                    emphasis: false,
                }],
            },
        );
        definitions.push(json!({"id":option,"text":text,"value":{"type":"string","value":label},"visible":visible}));
        let branch = adapter.id("route");
        let op = adapter.id("op");
        routes.blocks.insert(branch.clone(),json!({"ops":[{"id":op,"operation":{"type":"assign","target":"選択番号","value":{"type":"const","value":{"type":"i32","value":ordinal}}}}],"terminator":{"type":"call","function":cleanup,"next":destination}}));
        branches.insert(option, branch);
    }
    let choice = adapter.intern_choice(json!({"options":definitions}));
    let empty = adapter.id("route");
    let reset_value = adapter.id("op");
    let reset_number = adapter.id("op");
    if let Some(next) = next {
        routes.blocks.insert(empty.clone(),json!({"ops":[
        {"id":reset_value,"operation":{"type":"assign","target":CHOICE_RESULT,"value":{"type":"const","value":{"type":"string","value":""}}}},
        {"id":reset_number,"operation":{"type":"assign","target":"選択番号","value":{"type":"const","value":{"type":"i32","value":-1}}}}],
        "terminator":{"type":"call","function":cleanup,"next":next}}));
    } else {
        routes.blocks.insert(empty.clone(), json!({"ops":[],"terminator":{"type":"fault",
            "code":"E_IMPORT_CHOICE_EMPTY","message":"A guaranteed nonempty imported choice became empty"}}));
    }
    routes.blocks.insert(id.to_owned(),json!({"ops":[],"terminator":{"type":"interact","choice":choice,"branches":branches,"on_empty":empty,"result":CHOICE_RESULT}}));
    routes.choice_sites += 1;
    Ok(true)
}

/// Pure conditional appends to the stock disabled/hidden-label lists. The selected
/// filter list remains empty; arbitrary list writes/calls are never interpreted.
pub(super) fn preview_option_filters(
    adapter: &Adapter,
    commands: &[Command],
) -> Result<BTreeMap<String, BTreeMap<String, Value>>> {
    let mut flow = super::super::ui_flow::Flow::default();
    let mut filters = BTreeMap::<String, BTreeMap<String, Value>>::new();
    ensure!(
        commands.len() <= 128,
        "E_IMPORT_IMAGE_CHOICE: filter instruction limit"
    );
    for (index, command) in commands.iter().enumerate() {
        ensure!(
            !command.muted && command.not_update,
            "E_IMPORT_IMAGE_CHOICE: filter scope"
        );
        flow.enter(command, index)?;
        match &command.body {
            Body::Condition(_) if command.kind == 0 && command.indent <= 8 => {}
            Body::Other if command.kind == 2 && command.indent <= 8 => {}
            Body::Calc(source) => {
                let (name, term) = ui_expr::assignment(source)?;
                let Term::Apply {
                    op: Op::Concat,
                    args,
                } = term
                else {
                    bail!("E_IMPORT_IMAGE_CHOICE: unknown filter append");
                };
                ensure!(
                    matches!(name.as_str(), "_tmp" | "_tmp2") && args.len() == 2,
                    "E_IMPORT_IMAGE_CHOICE: unsupported filter destination"
                );
                ensure!(
                    args[0]
                        == Term::Apply {
                            op: Op::AddDelimiter,
                            args: vec![
                                Term::String {
                                    value: "\r\n".into()
                                },
                                Term::Read { name: name.clone() }
                            ]
                        },
                    "E_IMPORT_IMAGE_CHOICE: filter delimiter or list changed"
                );
                let Term::String { value: label } = &args[1] else {
                    bail!("E_IMPORT_IMAGE_CHOICE: dynamic hidden label");
                };
                ensure!(
                    !label.is_empty() && label.len() <= 1024 && !label.contains(['\r', '\n']),
                    "E_IMPORT_IMAGE_CHOICE: invalid hidden label"
                );
                let mut predicate = json!({"type":"const","value":{"type":"bool","value":true}});
                for guard in flow.guards() {
                    ensure!(
                        !guard.muted && guard.loop_target.is_none(),
                        "E_IMPORT_IMAGE_CHOICE: filter loop"
                    );
                    let term = ui_expr::normalize(&guard.source)?
                        .context("E_IMPORT_IMAGE_CHOICE: empty filter condition")?;
                    let mut value = condition(adapter, &term)?;
                    if !guard.expected {
                        value = json!({"type":"not","value":value});
                    }
                    predicate = json!({"type":"binary","op":"and","left":predicate,"right":value});
                }
                filters.entry(name.clone()).or_default().entry(label.clone()).and_modify(|old| {
                    *old = json!({"type":"binary","op":"or","left":old.clone(),"right":predicate.clone()});
                }).or_insert(predicate);
            }
            _ => bail!("E_IMPORT_IMAGE_CHOICE: unsupported filter command"),
        }
    }
    ensure!(
        filters.values().map(BTreeMap::len).sum::<usize>() <= 64,
        "E_IMPORT_IMAGE_CHOICE: filter label limit"
    );
    Ok(filters)
}

/// Freeze disabled/hidden images in ordinary scene opacity before the authored fade.
/// DraftPatch captures the prepared base scene; no browser callback or source
/// interpreter decides whether an image is visible.
fn filtered_preview_enter(
    adapter: &mut Adapter,
    targets: &[(String, Value)],
    captures: &[(String, Value)],
    duration: u64,
) -> String {
    let mut initial = vec![];
    adapter.scene(&mut initial, 0);
    let initial_fn = adapter.id("choice_prepare");
    adapter.finish_function(&initial_fn, initial, json!({"type":"return"}));
    let mut present = vec![];
    adapter.scene(&mut present, duration);
    let present_fn = adapter.id("choice_present");
    adapter.finish_function(&present_fn, present, json!({"type":"return"}));
    let mut blocks = BTreeMap::new();
    let capture_ops: Vec<_> = captures
        .iter()
        .map(|(target, value)| {
            let id = adapter.id("op");
            json!({"id":id,"operation":{"type":"assign","target":target,"value":value}})
        })
        .collect();
    blocks.insert(
        "capture".to_owned(),
        json!({"ops":capture_ops,
        "terminator":{"type":"goto","target":"prepare"}}),
    );
    blocks.insert("prepare".to_owned(), json!({"ops":[],"terminator":{
        "type":"call","function":initial_fn,"next":if targets.is_empty(){"present"}else{"filter0"}}}));
    for (index, (node, predicate)) in targets.iter().enumerate() {
        let next = if index + 1 == targets.len() {
            "present".into()
        } else {
            format!("filter{}", index + 1)
        };
        blocks.insert(
            format!("filter{index}"),
            json!({"ops":[],"terminator":{
            "type":"branch","condition":predicate,"yes":format!("show{index}"),"no":next}}),
        );
        let op = adapter.id("op");
        blocks.insert(
            format!("show{index}"),
            json!({"ops":[{"id":op,"operation":{
            "type":"draft_patch","node":node,"property":"opacity","value":1.}}],
            "terminator":{"type":"goto","target":next}}),
        );
    }
    blocks.insert(
        "present".to_owned(),
        json!({"ops":[],"terminator":{
        "type":"call","function":present_fn,"next":"done"}}),
    );
    blocks.insert(
        "done".to_owned(),
        json!({"ops":[],"terminator":{"type":"return"}}),
    );
    let function = adapter.id("image_choice_enter");
    adapter
        .functions
        .insert(function.clone(), json!({"entry":"capture","blocks":blocks}));
    function
}

/// Bounded stock preview wrapper, including pure conditional hidden labels.
pub(super) fn image_choice(
    adapter: &mut Adapter,
    script: &Script,
    page: &str,
    pc: usize,
    id: &str,
    path: &RoutePath,
    routes: &mut Routes,
) -> Result<bool> {
    if !matches!(&script.commands[pc].body, Body::Wait(e) if e.first().is_some_and(|e| references(e,"選択実行中")))
    {
        return Ok(false);
    }
    let Some(call_pc) = (pc + 5..script.commands.len().min(pc + 134)).find(|index|
        matches!(&script.commands[*index].body, Body::Call {target,..} if target.page.ends_with(PREVIEW_EXECUTOR))) else {
        return Ok(false);
    };
    let Some(tail) = script.commands.get(call_pc..call_pc + 6) else {
        return Ok(false);
    };
    let body: Vec<_> = script.commands[pc..pc + 5].iter().chain(tail).collect();
    let filters = preview_option_filters(adapter, &script.commands[pc + 5..call_pc])?;
    if !filters.is_empty() {
        let Body::Call { target, .. } = &script.commands[call_pc].body else {
            unreachable!()
        };
        let (_, helper) = adapter.source.read(&target.page)?;
        ensure!(
            helper.version == 117
                && matches!(
                    helper.source_sha256.as_str(),
                    "796b52e73e9cdd8a59a12537865ad1dd41e63707f2be1f4cc777413a3f8720bd"
                        | "5795a8e97d0f0a8d2b94682d4e13f7059a9aaecf3f8c19df21dbd6a3587d8894"
                ),
            "E_IMPORT_IMAGE_CHOICE: unverified hidden-list helper"
        );
    }
    let Body::Call {
        target,
        condition,
        params,
        ..
    } = &body[5].body
    else {
        return Ok(false);
    };
    if !target.page.ends_with(PREVIEW_EXECUTOR) {
        return Ok(false);
    }
    ensure!(
        body.iter()
            .enumerate()
            .all(|(i, c)| !c.muted && c.not_update == !matches!(i, 0 | 5) && c.indent == 0),
        "E_IMPORT_IMAGE_CHOICE: custom wrapper scope"
    );
    ensure!(
        target.line == 0 && condition.flag()? && params.len() == 11,
        "E_IMPORT_IMAGE_CHOICE: custom executor"
    );
    let Body::Wait(wait) = &body[0].body else {
        unreachable!()
    };
    ensure!(
        wait.len() == 3 && literal_int(&wait[1])? == 0 && literal_int(&wait[2])? == 0,
        "E_IMPORT_IMAGE_CHOICE: timed wrapper wait"
    );
    let expected = Term::Apply {
        op: Op::SourceAnd,
        args: vec![
            Term::Apply {
                op: Op::Property,
                args: vec![
                    Term::String {
                        value: "メッセージボックス".into(),
                    },
                    Term::Int { value: 138 },
                ],
            },
            Term::Apply {
                op: Op::Not,
                args: vec![Term::Read {
                    name: "選択実行中".into(),
                }],
            },
        ],
    };
    ensure!(
        ui_expr::normalize(&wait[0])? == Some(expected),
        "E_IMPORT_IMAGE_CHOICE: changed wrapper wait predicate"
    );
    for (index, value) in [(1, 1), (6, 0)] {
        let Body::Calc(e) = &body[index].body else {
            bail!("E_IMPORT_IMAGE_CHOICE: missing selection flag");
        };
        ensure!(
            ui_expr::assignment(e)? == ("選択実行中".into(), Term::Int { value }),
            "E_IMPORT_IMAGE_CHOICE: changed selection flag"
        );
    }
    for (index, name) in [(2, "_tmp"), (3, "_tmp2"), (4, "_tmp3")] {
        ensure!(
            matches!(&body[index].body,Body::Variable{name:n,value_type:4,initial,scope:2}if n==name && initial.operations.is_empty()),
            "E_IMPORT_IMAGE_CHOICE: custom option filters"
        );
        ensure!(
            matches!(&body[index+6].body,Body::VariableDelete(n)if n==name),
            "E_IMPORT_IMAGE_CHOICE: changed cleanup"
        );
    }
    ensure!(
        matches!(&body[7].body,Body::Delete(e)if literal_string(e).ok()==Some("VOICE")),
        "E_IMPORT_IMAGE_CHOICE: changed voice cleanup"
    );
    let _object = literal_string(&params[0])?;
    for (index, name) in [(7, "_tmp"), (8, "_tmp2"), (10, "_tmp3")] {
        ensure!(
            ui_expr::normalize(&params[index])? == Some(Term::Read { name: name.into() }),
            "E_IMPORT_IMAGE_CHOICE: unknown option filter"
        );
    }
    let cancel = literal_int(&params[6])?;
    ensure!(
        matches!(cancel, 0 | 1) && literal_int(&params[9])? == 0,
        "E_IMPORT_IMAGE_CHOICE: unsupported preview flags"
    );
    let enter = literal_int(&params[2])?;
    let close = literal_int(&params[3])?;
    ensure!(
        (0..=60000).contains(&enter) && (-1..=60000).contains(&close),
        "E_IMPORT_IMAGE_CHOICE: transition duration"
    );
    if cancel == 1 || close == -1 {
        let (_, helper) = adapter.source.read(&target.page)?;
        ensure!(
            helper.version == 117
                && matches!(
                    helper.source_sha256.as_str(),
                    "796b52e73e9cdd8a59a12537865ad1dd41e63707f2be1f4cc777413a3f8720bd"
                        | "5795a8e97d0f0a8d2b94682d4e13f7059a9aaecf3f8c19df21dbd6a3587d8894"
                ),
            "E_IMPORT_IMAGE_CHOICE: unverified cancel/retained-menu helper"
        );
    }
    let _hover = literal_string(&params[4])?; // Existing declared hover-sound adaptation.
    let click = literal_string(&params[5])?.to_owned();
    let source = literal_string(&params[1])?.replace('\\', "/");
    let bytes = read_binary(&adapter.source.path(&source)?)?;
    ensure!(
        menu_dimensions(&bytes)? == adapter.stage,
        "E_IMPORT_IMAGE_CHOICE: different stage"
    );
    let buttons = menu(&bytes)?;
    ensure!(
        (1..=64).contains(&buttons.len()),
        "E_IMPORT_IMAGE_CHOICE: option count"
    );
    ensure!(
        filters
            .values()
            .flat_map(|list| list.keys())
            .all(|label| buttons.iter().any(|button| &button.label == label)),
        "E_IMPORT_IMAGE_CHOICE: hidden label absent from menu"
    );
    ensure!(
        filters.is_empty()
            || buttons
                .iter()
                .map(|button| &button.label)
                .collect::<BTreeSet<_>>()
                .len()
                == buttons.len(),
        "E_IMPORT_IMAGE_CHOICE: ambiguous menu filter labels"
    );
    let directory = Path::new(&source)
        .parent()
        .context("E_IMPORT_IMAGE_CHOICE: menu directory")?;
    let base_nodes = preview_base(&adapter.nodes)?;
    adapter.nodes = base_nodes.clone();
    adapter.nodes.insert(
        PREVIEW_ROOT.into(),
        serde_json::from_value(json!({
            "id":PREVIEW_ROOT,"x":0,"y":0,"width":adapter.stage[0],"height":adapter.stage[1],
            "color":[0.,0.,0.,0.],"order":10000
        }))?,
    );
    let mut definitions = vec![];
    let mut branches = BTreeMap::new();
    let mut filter_targets = vec![];
    let mut filter_captures = vec![];
    let mut layers = vec![];
    for (index, button) in buttons.into_iter().enumerate() {
        let hidden = filters
            .get("_tmp2")
            .and_then(|list| list.get(&button.label));
        let disabled = filters.get("_tmp").and_then(|list| list.get(&button.label));
        let mut visible = hidden.map_or_else(
            || json!({"type":"const","value":{"type":"bool","value":true}}),
            |hidden| json!({"type":"not","value":hidden}),
        );
        let mut enabled = disabled.map_or_else(
            || json!({"type":"const","value":{"type":"bool","value":true}}),
            |disabled| json!({"type":"not","value":disabled}),
        );
        if !filters.is_empty() {
            for (key, predicate) in [("visible", &mut visible), ("enabled", &mut enabled)] {
                let identity = serde_json::to_vec(&(
                    &adapter.location.source,
                    adapter.location.index,
                    index,
                    key,
                ))?;
                let variable = format!("__nir_choice_{}", nir_content::digest(&identity));
                ensure!(
                    !adapter.unsupported_status.contains(&variable)
                        && !adapter.status_values.contains_key(&variable)
                        && !adapter.status_flags.contains_key(&variable),
                    "E_IMPORT_IMAGE_CHOICE: capture collides with source variable"
                );
                let declared = adapter
                    .variables
                    .entry(variable.clone())
                    .or_insert_with(|| json!({"type":"bool","value":false}));
                ensure!(
                    declared["type"] == "bool",
                    "E_IMPORT_IMAGE_CHOICE: capture type collision"
                );
                filter_captures.push((variable.clone(), predicate.clone()));
                *predicate = json!({"type":"var","name":variable});
            }
        }
        let (asset, size) = adapter.image(&directory.join(&button.source).to_string_lossy())?;
        let hover = if button.selected.is_empty() {
            None
        } else {
            Some(
                adapter
                    .image(&directory.join(&button.selected).to_string_lossy())?
                    .0,
            )
        };
        let disabled_asset = if button.disabled.is_empty() {
            None
        } else {
            let (image, disabled_size) =
                adapter.image(&directory.join(&button.disabled).to_string_lossy())?;
            ensure!(
                disabled_size == size,
                "E_IMPORT_IMAGE_CHOICE: disabled image size"
            );
            Some(image)
        };
        let rect = [
            button.x as f32,
            button.y as f32,
            size[0] as f32,
            size[1] as f32,
        ];
        let normal = if disabled_asset.is_some() {
            json!({"type":"binary","op":"and","left":visible,"right":enabled})
        } else {
            visible.clone()
        };
        let disabled_visible = json!({"type":"binary","op":"and","left":visible,
            "right":{"type":"not","value":enabled}});
        let mut layer_ids = [None, None, None];
        for (variant, (image, opacity, predicate)) in [
            (Some(asset.clone()), 1., Some(normal)),
            (hover.clone(), 0., None),
            (disabled_asset.clone(), 0., Some(disabled_visible)),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(image) = image {
                let key = nir_content::digest(
                    format!("{page}:{pc}:{source}:{index}:{variant}").as_bytes(),
                );
                let node = format!("__nir_preview.{}", &key[..24]);
                ensure!(
                    !adapter.nodes.contains_key(&node),
                    "E_IMPORT_IMAGE_CHOICE: internal node collision"
                );
                layer_ids[variant] = Some(node.clone());
                adapter.committed_images.remove(&node);
                if !filters.is_empty() {
                    if let Some(predicate) = predicate {
                        filter_targets.push((node.clone(), predicate));
                    }
                }
                adapter.nodes.insert(
                    node.clone(),
                    Node {
                        id: node,
                        parent: Some(PREVIEW_ROOT.into()),
                        asset: Some(image),
                        x: rect[0],
                        y: rect[1],
                        width: rect[2],
                        height: rect[3],
                        scale: 1.,
                        opacity: if filters.is_empty() { opacity } else { 0. },
                        color: [1.; 4],
                        order: 10001 + index as i32,
                        clip: None,
                        timeline_binding: None,
                        inherit_existence: false,
                        sprite_transform: None,
                        bitmap_text: None,
                        preserve_pose: vec![],
                        offset: [0.; 2],
                    },
                );
            }
        }
        layers.push(layer_ids);
        let label = if button.label.is_empty() {
            format!("画像 {}", index + 1)
        } else {
            button.label.clone()
        };
        let text = adapter.intern_text(
            "image_choice",
            index,
            crate::AuthorTextDoc {
                source_revision: 1,
                contract_revision: 1,
                spans: vec![Span::Text {
                    id: "s0".into(),
                    text: label,
                    emphasis: false,
                }],
            },
        );
        let option = format!("o{index}");
        let mut definition = json!({"id":option,"text":text,"value":{"type":"string","value":button.label},"image":{"asset":asset,"hover_asset":hover,"disabled_asset":disabled_asset,"rect":rect}});
        if !filters.is_empty() {
            definition["visible"] = visible;
            definition["enabled"] = enabled;
        }
        definitions.push(definition);
        branches.insert(option, format!("choose{index}"));
    }
    let enter_fn = if filters.is_empty() {
        let mut enter_blocks = vec![];
        adapter.scene(&mut enter_blocks, enter as u64 * 1000);
        let enter_fn = adapter.id("image_choice_enter");
        adapter.finish_function(&enter_fn, enter_blocks, json!({"type":"return"}));
        enter_fn
    } else {
        filtered_preview_enter(
            adapter,
            &filter_targets,
            &filter_captures,
            enter as u64 * 1000,
        )
    };
    let preview_nodes = adapter.nodes.clone();
    let entry_audio = adapter.audio.clone();
    let entry_committed = adapter.committed_images.clone();
    let after = successor(script, call_pc + 5)?;
    adapter
        .variables
        .entry(CHOICE_RESULT.into())
        .or_insert(json!({"type":"string","value":""}));
    adapter
        .variables
        .entry("選択番号".into())
        .or_insert(json!({"type":"i32","value":-1}));
    let mut shared_close = None;
    if close >= 0 {
        adapter.nodes = base_nodes.clone();
        shared_close = Some(preview_close(adapter, &click, close as u64 * 1000)?);
    }
    for block in branches.values_mut() {
        let index: usize = block.strip_prefix("choose").unwrap().parse()?;
        adapter.audio = entry_audio.clone();
        adapter.committed_images = entry_committed.clone();
        let close_fn = if let Some(close_fn) = &shared_close {
            adapter.nodes = base_nodes.clone();
            adapter.audio.remove("voice");
            adapter.audio.remove("se");
            close_fn.clone()
        } else {
            adapter.nodes = preview_nodes.clone();
            // Keep captured hidden/disabled opacity while committing the
            // selected button's settle image to the retained stage.
            for (node, _) in &filter_targets {
                adapter.nodes.get_mut(node).unwrap().preserve_pose =
                    vec![nir_format::Property::Opacity];
            }
            if let Some(hover) = &layers[index][1] {
                let normal = adapter
                    .nodes
                    .get_mut(layers[index][0].as_ref().unwrap())
                    .unwrap();
                normal.opacity = 0.;
                normal.preserve_pose.clear();
                let hover = adapter.nodes.get_mut(hover).unwrap();
                hover.opacity = 1.;
                hover.preserve_pose.clear();
            }
            preview_close(adapter, &click, 0)?
        };
        let next = adapter.route_block(script, page, after, routes)?;
        adapter.enqueue_route(script, page, after, path.clone(), routes)?;
        *block = adapter.id("route");
        let ops = preview_output_ops(adapter, index as i32, false)?;
        routes.blocks.insert(
            block.clone(),
            json!({"ops":ops,"terminator":{"type":"call","function":close_fn,"next":next}}),
        );
    }
    let cancel_block = if cancel == 1 {
        adapter.nodes = base_nodes;
        adapter.audio = entry_audio;
        adapter.committed_images = entry_committed;
        let close_fn = preview_close(adapter, "", 0)?;
        let next = adapter.route_block(script, page, after, routes)?;
        adapter.enqueue_route(script, page, after, path.clone(), routes)?;
        let block = adapter.id("preview_cancel");
        let ops = preview_output_ops(adapter, -1, true)?;
        routes.blocks.insert(
            block.clone(),
            json!({"ops":ops,"terminator":{"type":"call","function":close_fn,"next":next}}),
        );
        Some(block)
    } else {
        None
    };
    let choice = adapter.intern_choice(json!({"options":definitions}));
    let interact = adapter.id("route");
    routes.blocks.insert(
        id.into(),
        json!({"ops":[
            {"id":adapter.id("preview_reset_value"),"operation":{"type":"assign","target":CHOICE_RESULT,"value":{"type":"const","value":{"type":"string","value":""}}}},
            {"id":adapter.id("preview_reset_index"),"operation":assignment(adapter,&Expression{literal:None,operations:vec![(1,"選択番号".into(),vec![Literal::Int(-1)])],functions:BTreeMap::new()})?}
        ],"terminator":{"type":"call","function":enter_fn,"next":interact}}),
    );
    routes.blocks.insert(interact,json!({"ops":[],"terminator":{"type":"interact","choice":choice,"branches":branches,"on_empty":"failed","result":CHOICE_RESULT,"on_cancel":cancel_block}}));
    routes.choice_sites += 1;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constant_numbered_filename_assignment_executes_and_restores_as_a_string() {
        let dir = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(dir.path()).unwrap());
        let source = Expression {
            literal: None,
            operations: vec![
                (
                    19,
                    "____0".into(),
                    vec![Literal::String("images/picture".into()), Literal::Int(10)],
                ),
                (
                    19,
                    "____1".into(),
                    vec![
                        Literal::Variable("____0".into()),
                        Literal::String(".gal".into()),
                    ],
                ),
                (
                    1,
                    "filename".into(),
                    vec![Literal::Variable("____1".into())],
                ),
            ],
            functions: BTreeMap::new(),
        };
        let operation = assignment(&mut adapter, &source).unwrap();
        let mut program: nir_format::Program =
            serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
        program.variables.extend(
            serde_json::from_value::<BTreeMap<String, nir_format::Value>>(json!(adapter.variables))
                .unwrap(),
        );
        program.functions.insert("main".into(), serde_json::from_value(json!({
            "entry":"start","blocks":{"start":{"ops":[{"id":"filename","operation":operation}],"terminator":{"type":"return"}}}
        })).unwrap());
        let validated = nir_core::ValidatedProgram::new(program).unwrap();
        let mut core =
            nir_core::Core::new(validated.clone(), "concat".into(), "en".into()).unwrap();
        core.step(nir_core::CoreInput::None, 1000);
        assert!(core.state().fault.is_none());
        let restored = nir_core::Core::restore(validated, core.snapshot(), "concat").unwrap();
        for state in [core.state(), restored.state()] {
            assert_eq!(
                state.variables["filename"],
                nir_format::Value::String("images/picture10.gal".into())
            );
        }
        let mut dynamic = source;
        adapter
            .variables
            .insert("counter".into(), json!({"type":"i32","value":10}));
        dynamic.operations[0].2[1] = Literal::Variable("counter".into());
        assert!(assignment(&mut adapter, &dynamic).is_err());
    }
    #[test]
    fn dead_local_cleanup_rejects_observable_variables_and_nonterminal_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Adapter::new(Source::new(dir.path()).unwrap());
        let int = |value| Expression {
            literal: Some(Literal::Int(value)),
            operations: vec![],
            functions: BTreeMap::new(),
        };
        let command = |kind, line, body| Command {
            kind,
            line,
            body,
            indent: 0,
            muted: false,
            not_update: false,
            offset: 0,
        };
        let exit = |line| command(6, line, Body::Exit(int(1)));
        let name = "unused_local_flag";
        let mut s = Script {
            version: 117,
            source_sha256: String::new(),
            commands: vec![command(16, 4, Body::VariableDelete(name.into())), exit(5)],
        };
        s.source_sha256 = "7e16261286d74e40ec0ef71551b8076670bac16cb6949edc59e511ccce0e77c4".into();
        assert!(dead_local_cleanup(&mut a, &s, 0, "other").is_err());
        a.variables
            .insert(name.into(), json!({"type":"i32","value":0}));
        assert!(dead_local_cleanup(&mut a, &s, 0, name).is_err());
        a.variables.remove(name);
        s.commands[1].body = Body::Exit(int(0));
        assert!(dead_local_cleanup(&mut a, &s, 0, name).is_err());
        s.commands[1] = exit(5);
        dead_local_cleanup(&mut a, &s, 0, name).unwrap();
        assert!(expression(&a, &Term::Read { name: name.into() }).is_err());
        let write = Expression {
            literal: None,
            operations: vec![(1, name.into(), vec![Literal::Int(1)])],
            functions: BTreeMap::new(),
        };
        assert!(assignment(&mut a, &write).is_err());
        s.source_sha256 = "0".repeat(64);
        assert!(dead_local_cleanup(&mut a, &s, 0, name).is_err());
    }
    #[test]
    fn textual_flag_constants_require_source_boolean_declaration_and_keep_profile_rules() {
        let temp = tempfile::tempdir().unwrap();
        let mut a = Adapter::new(Source::new(temp.path()).unwrap());
        a.variables
            .insert("flag".into(), json!({"type":"i32","value":0}));
        let assign = |name: &str, text: &str| Expression {
            literal: None,
            operations: vec![(1, name.into(), vec![Literal::String(text.into())])],
            functions: BTreeMap::new(),
        };
        assert!(assignment(&mut a, &assign("flag", "TRUE")).is_err());
        a.boolean_variables.insert("flag".into());
        for (text, value) in [("TRUE", 1), ("FALSE", 0)] {
            assert_eq!(
                assignment(&mut a, &assign("flag", text)).unwrap(),
                json!({"type":"assign","target":"flag","value":{"type":"const","value":{"type":"i32","value":value}}})
            );
        }
        assert!(assignment(&mut a, &assign("flag", "yes")).is_err());
        a.variables
            .insert("label".into(), json!({"type":"string","value":""}));
        assert_eq!(
            assignment(&mut a, &assign("label", "TRUE")).unwrap()["value"]["value"],
            json!({"type":"string","value":"TRUE"})
        );
        a.status_flags
            .insert("flag".into(), "fixture.progress".into());
        assert_eq!(
            assignment(&mut a, &assign("flag", "TRUE")).unwrap(),
            json!({"type":"profile_merge","key":"fixture.progress"})
        );
        assert!(assignment(&mut a, &assign("flag", "FALSE")).is_err());
    }
    #[test]
    fn preview_hidden_lists_accept_only_pure_literal_label_appends() {
        let temp = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
        adapter
            .variables
            .insert("flag".into(), json!({"type":"i32","value":0}));
        let command = |kind, indent, body| Command {
            kind,
            indent,
            muted: false,
            not_update: true,
            line: 0,
            offset: 0,
            body,
        };
        let condition = Expression {
            literal: None,
            operations: vec![(
                12,
                "____arg".into(),
                vec![Literal::Variable("flag".into()), Literal::Int(1)],
            )],
            functions: BTreeMap::new(),
        };
        let append = Expression {
            literal: None,
            operations: vec![
                (
                    11,
                    "____0".into(),
                    vec![
                        Literal::String("\r\n".into()),
                        Literal::Variable("_tmp".into()),
                    ],
                ),
                (
                    19,
                    "____1".into(),
                    vec![
                        Literal::Variable("____0".into()),
                        Literal::String("neutral.option".into()),
                    ],
                ),
                (1, "_tmp".into(), vec![Literal::Variable("____1".into())]),
            ],
            functions: BTreeMap::from([(0, 133)]),
        };
        let commands = [
            command(0, 0, Body::Condition(condition)),
            command(14, 1, Body::Calc(append.clone())),
        ];
        let hidden = preview_option_filters(&adapter, &commands).unwrap();
        assert_eq!(
            hidden["_tmp"].keys().collect::<Vec<_>>(),
            vec!["neutral.option"]
        );
        let predicate: nir_format::Expr =
            serde_json::from_value(hidden["_tmp"]["neutral.option"].clone()).unwrap();
        assert!(!predicate.uses_float80());
        let mut wrong = append.clone();
        wrong.operations.last_mut().unwrap().1 = "_tmp3".into();
        assert!(preview_option_filters(&adapter, &[command(14, 0, Body::Calc(wrong))]).is_err());
        let mut dynamic = append.clone();
        dynamic.operations[1].2[1] = Literal::Variable("flag".into());
        assert!(preview_option_filters(&adapter, &[command(14, 0, Body::Calc(dynamic))]).is_err());
        let mut random = append;
        random.functions.insert(0, 26);
        assert!(preview_option_filters(&adapter, &[command(14, 0, Body::Calc(random))]).is_err());
    }

    #[test]
    fn filtered_preview_freezes_both_image_pose_and_offer_through_fade_and_restore() {
        use nir_core::{Core, CoreInput, ValidatedProgram};
        use nir_format::{Program, Value as TypedValue};
        let temp = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
        let visible = json!({"type":"binary","op":"eq","left":{"type":"var","name":"flag"},
            "right":{"type":"const","value":{"type":"i32","value":1}}});
        let captured = json!({"type":"var","name":"captured"});
        for (id, color) in [("shown", [1., 0., 0., 1.]), ("hidden", [0., 1., 0., 1.])] {
            adapter.nodes.insert(
                id.into(),
                serde_json::from_value(json!({
                "id":id,"x":0,"y":0,"width":32,"height":24,"opacity":0.,"color":color}))
                .unwrap(),
            );
        }
        let function = filtered_preview_enter(
            &mut adapter,
            &[
                ("shown".into(), captured.clone()),
                ("hidden".into(), json!({"type":"not","value":captured})),
            ],
            &[("captured".into(), visible)],
            500_000,
        );
        let mut p: Program =
            serde_json::from_str(include_str!("../../../../fixtures/rain.json")).unwrap();
        p.variables.insert("flag".into(), TypedValue::I32(1));
        p.variables
            .insert("captured".into(), TypedValue::Bool(false));
        for (id, scene) in adapter.scenes {
            p.scenes.insert(id, scene);
        }
        for (id, cue) in adapter.cues {
            p.cues.insert(id, serde_json::from_value(cue).unwrap());
        }
        for (id, f) in adapter.functions {
            p.functions.insert(id, serde_json::from_value(f).unwrap());
        }
        p.choices.get_mut("route").unwrap().options[0].visible =
            Some(serde_json::from_value(captured.clone()).unwrap());
        p.choices.get_mut("route").unwrap().options[1].visible =
            Some(serde_json::from_value(json!({"type":"not","value":captured})).unwrap());
        p.functions.insert("main".into(), serde_json::from_value(json!({"entry":"enter","blocks":{
            "enter":{"ops":[],"terminator":{"type":"call","function":function,"next":"choice"}},
            "choice":{"ops":[],"terminator":{"type":"interact","choice":"route","branches":{"walk":"done","stay":"done"},"on_empty":"done"}},
            "done":{"ops":[],"terminator":{"type":"end","outcome":"done"}}}})).unwrap());
        let validated = ValidatedProgram::new(p).unwrap();
        let mut core = Core::new(validated.clone(), "filters".into(), "en".into()).unwrap();
        core.step(CoreInput::None, 1000);
        for _ in 0..2 {
            let activation = core.state().pending.as_ref().unwrap().id;
            core.step(CoreInput::Prepared { activation }, 1000);
        }
        assert!(core.state().fault.is_none(), "{:?}", core.state().fault);
        let stage = &core.state().tasks[&core.state().handles["stage"]];
        assert_eq!(
            stage
                .target
                .iter()
                .find(|n| n.id == "shown")
                .unwrap()
                .opacity,
            1.
        );
        assert_eq!(
            stage
                .target
                .iter()
                .find(|n| n.id == "hidden")
                .unwrap()
                .opacity,
            0.
        );
        core.step(CoreInput::Time { delta_us: 200_000 }, 1000);
        let mut snapshot = core.snapshot();
        snapshot.variables.insert("flag".into(), TypedValue::I32(0));
        let mut restored = Core::restore(validated, snapshot, "filters").unwrap();
        restored.step(CoreInput::Time { delta_us: 300_000 }, 1000);
        assert!(
            restored.state().fault.is_none(),
            "{:?}",
            restored.state().fault
        );
        assert_eq!(restored.state().choice.as_ref().unwrap().options.len(), 1);
        assert_eq!(
            restored
                .sample_scene()
                .iter()
                .find(|n| n.id == "hidden")
                .unwrap()
                .opacity,
            0.
        );
    }

    #[test]
    fn authored_random_is_an_effectful_bounded_integer_write() {
        let temp = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
        let source = Expression {
            literal: None,
            operations: vec![
                (11, "____0".into(), vec![Literal::Int(100)]),
                (1, "chance".into(), vec![Literal::Variable("____0".into())]),
            ],
            functions: BTreeMap::from([(0, 26)]),
        };
        assert_eq!(
            assignment(&mut adapter, &source).unwrap(),
            json!({"type":"random","target":"chance","min":0,"max":99})
        );
        assert!(ui_expr::assignment(&source).is_err());
        assert!(ui_expr::normalize(&source).is_err());
        for bound in [0, -1] {
            let mut invalid = source.clone();
            invalid.operations[0].2 = vec![Literal::Int(bound)];
            assert!(assignment(&mut adapter, &invalid).is_err());
        }
        let mut dynamic = source.clone();
        dynamic.operations[0].2 = vec![Literal::Variable("chance".into())];
        assert!(assignment(&mut adapter, &dynamic).is_err());
        let mut nested = source.clone();
        nested.operations.insert(
            1,
            (
                2,
                "____1".into(),
                vec![Literal::Variable("____0".into()), Literal::Int(1)],
            ),
        );
        assert!(assignment(&mut adapter, &nested).is_err());
        adapter
            .status_flags
            .insert("chance".into(), "chance.unlocked".into());
        assert!(assignment(&mut adapter, &source).is_err());
        adapter.status_flags.clear();
        adapter.variables.insert(
            "chance".into(),
            json!({"type":"f80","value":nir_format::Float80::from_i32(0)}),
        );
        assert!(assignment(&mut adapter, &source).is_err());
    }

    #[test]
    fn extended_source_math_promotes_integers_without_changing_destination_type() {
        let temp = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
        adapter.variables.insert(
            "tax".into(),
            json!({"type":"f80","value":nir_format::Float80::from_i32(0)}),
        );
        let source = Expression {
            literal: None,
            operations: vec![(1, "tax".into(), vec![Literal::Int(1)])],
            functions: BTreeMap::new(),
        };
        let operation = assignment(&mut adapter, &source).unwrap();
        assert_eq!(operation["value"]["type"], "to_f80");
        let mixed = Term::Apply {
            op: Op::Multiply,
            args: vec![Term::Int { value: 100 }, Term::Read { name: "tax".into() }],
        };
        let (value, ty) = expression(&adapter, &mixed).unwrap();
        assert_eq!(ty, ValueType::F80);
        assert_eq!(value["left"]["type"], "to_f80");
        let converted = Term::Apply {
            op: Op::ToInt,
            args: vec![mixed],
        };
        assert_eq!(expression(&adapter, &converted).unwrap().1, ValueType::I32);
    }

    #[test]
    #[ignore = "private external extracted game fixture; set NIR_LIVENOVEL_SOURCE"]
    fn external_route_preflight() {
        let root = std::env::var("NIR_LIVENOVEL_SOURCE").expect("NIR_LIVENOVEL_SOURCE");
        let source = Source::new(Path::new(&root)).unwrap();
        let entry = source.entry().unwrap();
        let mut adapter = Adapter::new(source);
        match adapter.run(&entry) {
            Ok(story) => {
                crate::project::validate_generated_fragment(&story).unwrap();
                if let Ok(out) = std::env::var("NIR_LIVENOVEL_PROBE") {
                    fs::write(
                        out,
                        serde_json::to_vec(
                            &json!({"story":story,"texts":adapter.texts,"menus":adapter.menus,"gain_sensitive_buses":adapter.gain_reads,"missing_menu_sounds":adapter.missing_menu_sounds}),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                }
                println!(
                    "PREFLIGHT_OK pages={} functions={} assets={} choices={} gain_sensitive_buses={:?} missing_menu_sounds={:?} story_bytes={}",
                    adapter.texts.len(),
                    adapter.functions.len(),
                    adapter.assets.len(),
                    adapter.choice_sites,
                    adapter.gain_reads,
                    adapter.missing_menu_sounds,
                    serde_json::to_vec(&story).unwrap().len()
                );
            }
            Err(error) => panic!("PREFLIGHT_ERROR: {error:#}; emitted pages={} functions={} scenes={} cues={} gain_reads={:?} geometry_reads={:?}",
                adapter.texts.len(), adapter.functions.len(), adapter.scenes.len(), adapter.cues.len(), adapter.gain_reads, adapter.geometry_reads),
        }
    }
}
