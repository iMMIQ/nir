//! Stock temporary-hide callback contract. This recognizes a bounded source
//! control-flow pattern; it does not execute component loops or source events.
use super::{
    lsb::{Body, Command, Expression, Literal, Script},
    ui_expr::{self, Op, Term},
};
use anyhow::{ensure, Context, Result};

fn read(name: &str) -> Term {
    Term::Read { name: name.into() }
}
fn text(value: &str) -> Term {
    Term::String {
        value: value.into(),
    }
}
fn int(value: i32) -> Term {
    Term::Int { value }
}
fn apply(op: Op, args: Vec<Term>) -> Term {
    Term::Apply { op, args }
}
fn exists(t: Term) -> Term {
    apply(Op::ObjectExists, vec![t])
}
fn not(t: Term) -> Term {
    apply(Op::Not, vec![t])
}
fn ne(a: Term, b: Term) -> Term {
    apply(Op::NotEqual, vec![a, b])
}
fn prop(t: Term, n: i32) -> Term {
    apply(Op::Property, vec![t, int(n)])
}
fn at() -> Term {
    apply(Op::Index, vec![read("ss"), read("i")])
}

#[derive(Clone)]
enum Expected {
    Var(&'static str, u8),
    Cabinet(bool, Vec<Term>),
    Set(Term, i32, Term),
    Get(Term, &'static str),
    If(Term),
    Else,
    Collect,
    Clear,
    Init(bool),
    While(Term, usize),
    Loop(bool, usize),
    Flip,
    Event,
}
fn contract() -> Vec<(u32, Expected)> {
    use Expected::*;
    let mes = read("メッセージボックス");
    let base = read("メッセージボックス土台");
    let choice = text("選択メニュー");
    let parent = prop(choice.clone(), 1);
    let has_base = apply(
        Op::SourceAnd,
        vec![ne(base.clone(), text("")), exists(base.clone())],
    );
    let loop_condition = apply(
        Op::Less,
        vec![read("i"), apply(Op::ArraySize, vec![read("ss")])],
    );
    let keep = apply(
        Op::SourceOr,
        vec![
            not(apply(Op::IsDelimiter, vec![text("#"), at(), int(1)])),
            not(prop(at(), 47)),
        ],
    );
    let mut waiting = apply(
        Op::SourceAnd,
        vec![not(read("@LClick")), not(read("@RClick"))],
    );
    for key in [32, 13, 27] {
        waiting = apply(
            Op::SourceAnd,
            vec![
                waiting,
                not(apply(Op::Index, vec![read("@KeyClick"), int(key)])),
            ],
        );
    }
    vec![
        (1, Var("i", 1)),
        (1, Var("ss", 4)),
        (1, Var("vismes", 3)),
        (1, Var("vismesb", 3)),
        (1, Var("vismenu", 3)),
        (
            1,
            Cabinet(
                false,
                [
                    "メッセージボックス",
                    "メッセージボックス土台",
                    "名前ラベル",
                    "テキスト顔",
                    "選択メニュー",
                ]
                .into_iter()
                .map(text)
                .collect(),
            ),
        ),
        (1, Set(text("メニュー背景"), 80, int(1))),
        (1, Set(text("システムメニュー"), 47, int(0))),
        (1, If(exists(mes.clone()))),
        (2, Get(mes.clone(), "vismes")),
        (2, Set(mes.clone(), 47, int(0))),
        (1, If(has_base.clone())),
        (2, Get(base.clone(), "vismesb")),
        (2, Set(base.clone(), 47, int(0))),
        (1, If(exists(choice.clone()))),
        (2, If(ne(parent.clone(), text("")))),
        (3, Get(parent.clone(), "vismenu")),
        (3, Set(parent.clone(), 47, int(0))),
        (2, Else),
        (3, Get(choice.clone(), "vismenu")),
        (3, Set(choice.clone(), 47, int(0))),
        (1, Collect),
        (1, Init(true)),
        (1, While(loop_condition.clone(), 29)),
        (2, If(keep)),
        (3, Clear),
        (2, Else),
        (3, Set(at(), 47, int(0))),
        (1, Loop(true, 23)),
        (1, Flip),
        (1, Init(false)),
        (1, While(waiting, 34)),
        (2, Event),
        (1, Loop(false, 31)),
        (1, Flip),
        (1, If(exists(mes.clone()))),
        (2, Set(mes, 47, read("vismes"))),
        (1, If(has_base)),
        (2, Set(base, 47, read("vismesb"))),
        (1, If(exists(choice.clone()))),
        (2, If(ne(parent.clone(), text("")))),
        (3, Set(parent, 47, read("vismenu"))),
        (2, Else),
        (3, Set(choice, 47, read("vismenu"))),
        (1, Init(true)),
        (1, While(loop_condition, 49)),
        (2, If(ne(at(), text("")))),
        (3, Set(at(), 47, int(1))),
        (1, Loop(true, 45)),
        (1, Cabinet(true, vec![])),
        (1, Set(text("メニュー背景"), 80, int(0))),
        (1, Set(text("システムメニュー"), 47, int(1))),
    ]
}
fn same(e: &Expression, t: &Term) -> Result<bool> {
    Ok(ui_expr::normalize(e)?.as_ref() == Some(t))
}
fn assignment(e: &Expression, increment: bool) -> Result<bool> {
    Ok(ui_expr::assignment(e)?
        == (
            "i".into(),
            if increment {
                apply(Op::Add, vec![read("i"), int(1)])
            } else {
                int(0)
            },
        ))
}
// ListCompo writes an output array. It is deliberately absent from normalize's
// pure-call whitelist, and only this exact isolated call shape is accepted.
fn collect(e: &Expression) -> bool {
    let [call, ret] = e.operations.as_slice() else {
        return false;
    };
    e.functions.len() == 1
        && e.functions.get(&0) == Some(&32)
        && call.0 == 11
        && ui_expr::temporary(&call.1)
        && call.1 != "____arg"
        && matches!(call.2.as_slice(),[Literal::Variable(v)] if v=="ss")
        && ret.0 == 1
        && ret.1 == "____arg"
        && matches!(ret.2.as_slice(),[Literal::Variable(v)] if v==&call.1)
}
// A ____d_ temporary obtained by indexing is a writable array-slot reference.
// Do not normalize this as an innocuous local reassignment.
fn clear(e: &Expression) -> bool {
    let [value, slot, write] = e.operations.as_slice() else {
        return false;
    };
    e.functions.is_empty()
        && value.0 == 1
        && ui_expr::temporary(&value.1)
        && matches!(value.2.as_slice(),[Literal::String(s)] if s.is_empty())
        && slot.0 == 10
        && slot.1.starts_with("____d_")
        && ui_expr::temporary(&slot.1)
        && slot.1 != value.1
        && matches!(slot.2.as_slice(),[Literal::Variable(a),Literal::Variable(i)] if a=="ss"&&i=="i")
        && write.0 == 1
        && write.1 == slot.1
        && matches!(write.2.as_slice(),[Literal::Variable(v)] if v==&value.1)
}
fn matches(c: &Command, expected: &Expected, base: usize) -> Result<bool> {
    use Expected::*;
    Ok(match (expected, &c.body) {
        (
            Var(name, ty),
            Body::Variable {
                name: n,
                value_type,
                initial,
                scope,
            },
        ) => {
            c.kind == 15
                && n == name
                && value_type == ty
                && *scope == 2
                && ui_expr::normalize(initial)?.is_none()
        }
        (
            Cabinet(save, wanted),
            Body::Cabinet {
                properties,
                act,
                targets,
            },
        ) => {
            c.kind == if *save { 59 } else { 60 }
                && properties.len() == 1
                && properties
                    .get(&1)
                    .is_some_and(|e| same(e, &text("キャビネット")).unwrap_or(false))
                && same(act, &int(1))?
                && targets.len() == wanted.len()
                && targets
                    .iter()
                    .zip(wanted)
                    .all(|(e, t)| same(e, t).unwrap_or(false))
        }
        (
            Set(t, p, v),
            Body::SetProperty {
                target,
                property,
                value,
            },
        ) => c.kind == 18 && same(target, t)? && same(property, &int(*p))? && same(value, v)?,
        (
            Get(t, n),
            Body::GetProperty {
                target,
                property,
                destination,
            },
        ) => c.kind == 17 && same(target, t)? && same(property, &int(47))? && destination == n,
        (If(t), Body::Condition(e)) => c.kind == 0 && same(e, t)?,
        (Else, Body::Other) => c.kind == 2,
        (Collect, Body::Calc(e)) => c.kind == 14 && collect(e),
        (Clear, Body::Calc(e)) => c.kind == 14 && clear(e),
        (
            Init(value),
            Body::LoopUpdate {
                expression,
                target: None,
            },
        ) => {
            c.kind == 32
                && if *value {
                    assignment(expression, false)?
                } else {
                    ui_expr::normalize(expression)?.is_none()
                }
        }
        (While(t, end), Body::LoopCondition { condition, target }) => {
            c.kind == 31 && *target as usize == base + end && same(condition, t)?
        }
        (
            Loop(value, begin),
            Body::LoopUpdate {
                expression,
                target: Some(target),
            },
        ) => {
            c.kind == 33
                && *target as usize == base + begin
                && if *value {
                    assignment(expression, true)?
                } else {
                    ui_expr::normalize(expression)?.is_none()
                }
        }
        (
            Flip,
            Body::Flip {
                parameters,
                targets,
            },
        ) => {
            c.kind == 13
                && targets.is_empty()
                && parameters.len() == 9
                && [
                    ("wipe", Some(3)),
                    ("time", Some(200)),
                    ("reverse", Some(0)),
                    ("act", Some(1)),
                    ("delete", Some(0)),
                    ("parameter_0", Some(8)),
                    ("parameter_1", None),
                    ("source", None),
                    ("stop_event", Some(1)),
                ]
                .into_iter()
                .all(|(key, value)| {
                    parameters
                        .get(key)
                        .is_some_and(|e| ui_expr::normalize(e).ok() == Some(value.map(int)))
                })
        }
        (Event, Body::Other) => c.kind == 46,
        _ => false,
    })
}
pub(super) fn verify(script: &Script) -> Result<()> {
    ensure!(script.version == 116, "E_IMPORT_PEEK: unsupported version");
    super::ui_dispatch::verify(script)?;
    let head = script
        .commands
        .get(5)
        .context("E_IMPORT_PEEK: missing hide branch")?;
    let Body::Condition(e) = &head.body else {
        anyhow::bail!("E_IMPORT_PEEK: missing hide condition")
    };
    ensure!(
        same(e, &apply(Op::Equal, vec![read("val"), text("文字を消す")]))?,
        "E_IMPORT_PEEK: unknown first action"
    );
    let base = 6;
    let end = (base..script.commands.len())
        .find(|i| script.commands[*i].indent == 0)
        .context("E_IMPORT_PEEK: missing next branch")?;
    let expected = contract();
    ensure!(
        end - base == expected.len(),
        "E_IMPORT_PEEK: additional or missing hide operations"
    );
    for (i, (c, (indent, wanted))) in script.commands[base..end].iter().zip(&expected).enumerate() {
        ensure!(
            c.indent == *indent
                && !c.muted
                && (!c.not_update || matches!(wanted, Expected::Else))
                && matches(c, wanted, base)?,
            "E_IMPORT_PEEK: unsupported hide operation {} ({})",
            base + i,
            c.name()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    fn var(name: &str) -> Literal {
        Literal::Variable(name.into())
    }
    #[test]
    fn component_list_output_is_not_a_pure_call_or_arbitrary_callback() {
        let e = Expression {
            literal: None,
            operations: vec![
                (11, "____7".into(), vec![var("ss")]),
                (1, "____arg".into(), vec![var("____7")]),
            ],
            functions: BTreeMap::from([(0, 32)]),
        };
        assert!(collect(&e));
        assert!(ui_expr::normalize(&e).is_err());
        for case in 0..4 {
            let mut e = e.clone();
            match case {
                0 => {
                    e.functions.insert(0, 26);
                }
                1 => e.operations[0].2[0] = var("other"),
                2 => e.operations[1].1 = "external".into(),
                _ => e
                    .operations
                    .push((1, "external".into(), vec![Literal::Int(1)])),
            }
            assert!(!collect(&e));
        }
    }
    #[test]
    fn array_slot_filter_requires_the_same_index_reference_and_empty_replacement() {
        let e = Expression {
            literal: None,
            operations: vec![
                (1, "____0".into(), vec![Literal::String("".into())]),
                (10, "____d_1".into(), vec![var("ss"), var("i")]),
                (1, "____d_1".into(), vec![var("____0")]),
            ],
            functions: BTreeMap::new(),
        };
        assert!(clear(&e));
        assert!(ui_expr::normalize(&e).is_err());
        for case in 0..4 {
            let mut e = e.clone();
            match case {
                0 => e.operations[0].2[0] = Literal::String("changed".into()),
                1 => e.operations[1].2[1] = var("j"),
                2 => e.operations[2].1 = "other".into(),
                _ => e.operations[2].2[0] = Literal::Int(0),
            }
            assert!(!clear(&e));
        }
    }
    #[test]
    fn polling_yields_events_without_clearing_history() {
        let mut c = Command {
            kind: 46,
            indent: 2,
            muted: false,
            not_update: false,
            line: 0,
            offset: 0,
            body: Body::Other,
        };
        assert!(matches(&c, &Expected::Event, 6).unwrap());
        c.kind = 22;
        assert!(!matches(&c, &Expected::Event, 6).unwrap());
    }
    #[test]
    fn wait_loop_targets_are_command_indices_not_source_line_numbers() {
        let e = Expression {
            literal: None,
            operations: vec![(1, "____arg".into(), vec![var("waiting")])],
            functions: BTreeMap::new(),
        };
        let mut c = Command {
            kind: 31,
            indent: 1,
            muted: false,
            not_update: false,
            line: 500,
            offset: 900,
            body: Body::LoopCondition {
                condition: e,
                target: 40,
            },
        };
        assert!(matches(&c, &Expected::While(read("waiting"), 34), 6).unwrap());
        if let Body::LoopCondition { target, .. } = &mut c.body {
            *target = 500;
        }
        assert!(!matches(&c, &Expected::While(read("waiting"), 34), 6).unwrap());
    }
}
