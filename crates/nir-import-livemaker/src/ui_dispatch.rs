//! Check the stock selection callback's entry dispatch. This is not a general
//! source interpreter and does not certify other branch bodies or array lifetime.
use super::{
    lsb::{Body, Expression, Script},
    ui_expr::{self, Op, Term},
};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeSet;
fn read(name: &str) -> Term {
    Term::Read { name: name.into() }
}
fn text(value: &str) -> Term {
    Term::String {
        value: value.into(),
    }
}
fn apply(op: Op, args: Vec<Term>) -> Term {
    Term::Apply { op, args }
}
fn parameter() -> Term {
    apply(Op::Index, vec![read("@ParamStr"), Term::Int { value: 0 }])
}
fn equals(expression: &Expression, value: Term) -> Result<bool> {
    Ok(ui_expr::normalize(expression)? == Some(value))
}

pub(super) fn verify(script: &Script) -> Result<()> {
    ensure!(
        script
            .commands
            .get(5)
            .is_some_and(|c| c.indent == 0 && c.kind == 0),
        "E_IMPORT_MENU_DISPATCH: extra entry operation before case chain"
    );
    let prefix = script
        .commands
        .get(..5)
        .context("E_IMPORT_MENU_DISPATCH: missing entry prefix")?;
    for (c, (kind, indent)) in prefix
        .iter()
        .zip([(15, 0), (0, 0), (14, 1), (2, 0), (14, 1)])
    {
        ensure!(
            c.kind == kind && c.indent == indent && !c.muted && (!c.not_update || kind == 2),
            "E_IMPORT_MENU_DISPATCH: unsupported entry control flow"
        );
    }
    ensure!(
        matches!(&prefix[0].body,Body::Variable {name,value_type:4,initial,scope:2} if name=="val"&&initial.operations.is_empty()),
        "E_IMPORT_MENU_DISPATCH: unsupported action local"
    );
    let Body::Condition(e) = &prefix[1].body else {
        anyhow::bail!("E_IMPORT_MENU_DISPATCH: missing special action condition")
    };
    ensure!(
        equals(e, apply(Op::Equal, vec![parameter(), text("回想終了")]))?,
        "E_IMPORT_MENU_DISPATCH: unknown special action condition"
    );
    for (index, value) in [
        (2, text("回想終了")),
        (
            4,
            apply(
                Op::Index,
                vec![
                    read("システムメニュー項目値"),
                    apply(
                        Op::IndexOfString,
                        vec![read("システムメニュー項目名"), parameter()],
                    ),
                ],
            ),
        ),
    ] {
        let Body::Calc(e) = &prefix[index].body else {
            anyhow::bail!("E_IMPORT_MENU_DISPATCH: missing action assignment")
        };
        ensure!(
            ui_expr::assignment(e)? == ("val".into(), value),
            "E_IMPORT_MENU_DISPATCH: action lookup differs from label/action pairing"
        );
    }
    let mut cases = BTreeSet::new();
    let mut fallback = false;
    let mut exited = false;
    for c in &script.commands[5..] {
        if c.indent != 0 {
            continue;
        }
        ensure!(
            !c.muted && (!c.not_update || c.kind == 2),
            "E_IMPORT_MENU_DISPATCH: muted or deferred dispatch boundary"
        );
        if let Body::Exit(e) = &c.body {
            ensure!(
                fallback && equals(e, Term::Int { value: 1 })?,
                "E_IMPORT_MENU_DISPATCH: missing unconditional callback exit"
            );
            exited = true;
            break;
        }
        if c.kind == 2 {
            ensure!(
                !cases.is_empty() && !fallback,
                "E_IMPORT_MENU_DISPATCH: misplaced fallback"
            );
            fallback = true;
            continue;
        }
        ensure!(
            !fallback && c.kind == if cases.is_empty() { 0 } else { 1 },
            "E_IMPORT_MENU_DISPATCH: extra root operation or disconnected branch"
        );
        let Body::Condition(e) = &c.body else {
            anyhow::bail!("E_IMPORT_MENU_DISPATCH: missing case condition")
        };
        let Some(Term::Apply {
            op: Op::Equal,
            args,
        }) = ui_expr::normalize(e)?
        else {
            anyhow::bail!("E_IMPORT_MENU_DISPATCH: unsupported case predicate")
        };
        let [Term::Read { name }, Term::String { value }] = args.as_slice() else {
            anyhow::bail!("E_IMPORT_MENU_DISPATCH: indirect case predicate")
        };
        ensure!(
            name == "val" && !value.is_empty() && cases.insert(value.clone()),
            "E_IMPORT_MENU_DISPATCH: duplicate or ambiguous case"
        );
    }
    ensure!(
        exited && cases.contains("自動テキスト送り"),
        "E_IMPORT_MENU_DISPATCH: incomplete Auto dispatch/exit"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::lsb::{Command, Literal};
    use super::*;
    fn fixture() -> Script {
        let expr = |operations: Vec<(u8, &str, Vec<Literal>)>, functions| Expression {
            literal: None,
            operations: operations
                .into_iter()
                .map(|(o, n, a)| (o, n.into(), a))
                .collect(),
            functions,
        };
        let var = |s: &str| Literal::Variable(s.into());
        let string = |s: &str| Literal::String(s.into());
        let command = |kind, indent, body| Command {
            kind,
            indent,
            body,
            muted: false,
            not_update: false,
            line: 0,
            offset: 0,
        };
        Script {
            version: 116,
            source_sha256: "a".repeat(64),
            commands: vec![
                command(
                    15,
                    0,
                    Body::Variable {
                        name: "val".into(),
                        value_type: 4,
                        initial: Expression::default(),
                        scope: 2,
                    },
                ),
                command(
                    0,
                    0,
                    Body::Condition(expr(
                        vec![
                            (10, "____0", vec![var("@ParamStr"), Literal::Int(0)]),
                            (12, "____arg", vec![var("____0"), string("回想終了")]),
                        ],
                        Default::default(),
                    )),
                ),
                command(
                    14,
                    1,
                    Body::Calc(expr(
                        vec![(1, "val", vec![string("回想終了")])],
                        Default::default(),
                    )),
                ),
                command(2, 0, Body::Other),
                command(
                    14,
                    1,
                    Body::Calc(expr(
                        vec![
                            (10, "____0", vec![var("@ParamStr"), Literal::Int(0)]),
                            (
                                11,
                                "____1",
                                vec![var("システムメニュー項目名"), var("____0")],
                            ),
                            (
                                10,
                                "____2",
                                vec![var("システムメニュー項目値"), var("____1")],
                            ),
                            (1, "val", vec![var("____2")]),
                        ],
                        std::collections::BTreeMap::from([(1, 30)]),
                    )),
                ),
                command(
                    0,
                    0,
                    Body::Condition(expr(
                        vec![(12, "____arg", vec![var("val"), string("自動テキスト送り")])],
                        Default::default(),
                    )),
                ),
                command(2, 0, Body::Other),
                command(
                    6,
                    0,
                    Body::Exit(expr(
                        vec![(1, "____arg", vec![Literal::Int(1)])],
                        Default::default(),
                    )),
                ),
            ],
        }
    }
    #[test]
    fn rejects_wrong_lookup_extra_root_effects_duplicates_and_missing_exit() {
        verify(&fixture()).unwrap();
        let mut structural_flags = fixture();
        // Stock Else markers suppress refresh. They select a branch but do
        // not add another statement to its body; no refresh equivalence claim.
        structural_flags.commands[3].not_update = true;
        structural_flags.commands[6].not_update = true;
        verify(&structural_flags).unwrap();
        for case in 0..9 {
            let mut s = fixture();
            match case {
                0 => {
                    if let Body::Calc(e) = &mut s.commands[4].body {
                        e.operations[1].2[0] = Literal::Variable("other_labels".into());
                    }
                }
                1 => s.commands[4].indent = 0,
                2 => s.commands[5].muted = true,
                3 => {
                    let mut c = s.commands[5].clone();
                    c.kind = 1;
                    s.commands.insert(6, c);
                }
                4 => {
                    let mut c = s.commands[2].clone();
                    c.indent = 0;
                    s.commands.insert(6, c);
                }
                5 => {
                    s.commands.pop();
                }
                6 => {
                    if let Body::Exit(e) = &mut s.commands[7].body {
                        e.operations[0].2[0] = Literal::Int(0);
                    }
                }
                7 => {
                    s.commands.insert(5, s.commands[2].clone());
                }
                _ => {
                    let mut c = s.commands[2].clone();
                    c.indent = 0;
                    s.commands.insert(7, c);
                }
            }
            assert!(verify(&s).is_err(), "{case}");
        }
    }
}
