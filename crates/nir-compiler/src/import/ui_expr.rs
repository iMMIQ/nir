//! Bounded, source-side expression normalization. This does not run scripts:
//! reads stay symbolic, writes and non-whitelisted calls are rejected.
use super::lsb::{Expression, Literal};
use anyhow::{bail, ensure, Result};
use serde::Serialize;
use std::collections::BTreeMap;

const MAX_NODES: usize = 256;
const MAX_TEXT: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Term {
    Int { value: i32 },
    String { value: String },
    Read { name: String },
    Apply { op: Op, args: Vec<Term> },
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Op {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    // Source operators are type-dependent (integer bitwise or logical).
    // Keep them symbolic until a lowering profile proves operand types.
    SourceOr,
    SourceAnd,
    SourceXor,
    Equal,
    NotEqual,
    Greater,
    Less,
    GreaterEqual,
    LessEqual,
    Concat,
    Index,
    Property,
    ArraySize,
    ObjectExists,
    IndexOfString,
    StringPosition,
    AddDelimiter,
    IsDelimiter,
    Not,
    Min,
    Max,
}
impl Term {
    pub fn truth(&self) -> Option<bool> {
        match self {
            Self::Int { value } => Some(*value != 0),
            _ => None,
        }
    }
    fn budget(&self) -> (usize, usize) {
        match self {
            Self::Int { .. } => (1, 0),
            Self::String { value } => (1, value.len()),
            Self::Read { name } => (1, name.len()),
            Self::Apply { args, .. } => args.iter().fold((1, 0), |(n, b), t| {
                let (tn, tb) = t.budget();
                (n + tn, b + tb)
            }),
        }
    }
}
pub(super) fn temporary(name: &str) -> bool {
    if name.is_empty() || name == "____arg" {
        return true;
    }
    name.strip_prefix("____")
        .map(|s| s.strip_prefix("d_").unwrap_or(s))
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}
fn operand(value: &Literal, locals: &BTreeMap<String, Term>) -> Result<Term> {
    Ok(match value {
        Literal::Int(value) => Term::Int { value: *value },
        Literal::String(value) => {
            ensure!(value.len() <= MAX_TEXT, "E_IMPORT_UI_EXPR_LIMIT: string");
            Term::String {
                value: value.clone(),
            }
        }
        Literal::Variable(name) => {
            ensure!(name.len() <= 1024, "E_IMPORT_UI_EXPR_LIMIT: variable");
            if temporary(name) {
                locals.get(name).cloned().ok_or_else(|| {
                    anyhow::anyhow!("E_IMPORT_UI_EXPR_TEMP: uninitialized temporary")
                })?
            } else {
                Term::Read { name: name.clone() }
            }
        }
        Literal::Unsupported => bail!("E_IMPORT_UI_EXPR_TYPE: unsupported numeric representation"),
    })
}
fn apply(op: Op, args: Vec<Term>) -> Result<Term> {
    let int = |value| Ok(Term::Int { value });
    match (op, args.as_slice()) {
        (Op::Not, [Term::Int { value }]) => return int(i32::from(*value == 0)),
        (op, [Term::Int { value: a }, Term::Int { value: b }]) => {
            let value = match op {
                Op::Add => a.checked_add(*b),
                Op::Subtract => a.checked_sub(*b),
                Op::Multiply => a.checked_mul(*b),
                // Do not guess the source's destination-dependent coercion or
                // rounding. Only an exact integer result can become a constant.
                Op::Divide if *b != 0 && a.checked_rem(*b) == Some(0) => a.checked_div(*b),
                Op::Divide => return Ok(Term::Apply { op, args }),
                Op::Remainder if *a >= 0 && *b > 0 => a.checked_rem(*b),
                Op::Remainder => return Ok(Term::Apply { op, args }),
                Op::Equal => Some(i32::from(a == b)),
                Op::NotEqual => Some(i32::from(a != b)),
                Op::Greater => Some(i32::from(a > b)),
                Op::Less => Some(i32::from(a < b)),
                Op::GreaterEqual => Some(i32::from(a >= b)),
                Op::LessEqual => Some(i32::from(a <= b)),
                Op::Min => Some(*a.min(b)),
                Op::Max => Some(*a.max(b)),
                _ => return Ok(Term::Apply { op, args }),
            };
            return int(value.ok_or_else(|| anyhow::anyhow!("E_IMPORT_UI_EXPR_OVERFLOW"))?);
        }
        (Op::Equal | Op::NotEqual, [Term::String { value: a }, Term::String { value: b }]) => {
            return int(i32::from((a == b) == (op == Op::Equal)));
        }
        (Op::Concat, [Term::String { value: a }, Term::String { value: b }]) => {
            ensure!(
                a.len() + b.len() <= MAX_TEXT,
                "E_IMPORT_UI_EXPR_LIMIT: concatenation"
            );
            return Ok(Term::String {
                value: format!("{a}{b}"),
            });
        }
        _ => {}
    }
    Ok(Term::Apply { op, args })
}

pub(super) fn normalize(expression: &Expression) -> Result<Option<Term>> {
    ensure!(
        expression.operations.len() <= 64,
        "E_IMPORT_UI_EXPR_LIMIT: instructions"
    );
    if expression.operations.is_empty() {
        return Ok(None);
    }
    ensure!(
        expression
            .functions
            .iter()
            .all(|(i, _)| expression.operations.get(*i).is_some_and(|o| o.0 == 11)),
        "E_IMPORT_UI_EXPR_CALL: mismatched function metadata"
    );
    let mut locals = BTreeMap::new();
    for (index, (opcode, destination, values)) in expression.operations.iter().enumerate() {
        ensure!(values.len() <= 3, "E_IMPORT_UI_EXPR_ARITY");
        ensure!(
            !locals.contains_key(destination),
            "E_IMPORT_UI_EXPR_WRITE: repeated temporary may write through a source alias"
        );
        ensure!(
            temporary(destination),
            "E_IMPORT_UI_EXPR_WRITE: expression writes source state"
        );
        let args = values
            .iter()
            .map(|v| operand(v, &locals))
            .collect::<Result<Vec<_>>>()?;
        // Bound the expanded tree before retaining it. A tiny bytecode stream
        // can otherwise repeatedly duplicate an earlier expression.
        let (nodes, bytes) = args.iter().fold((1, 0), |(n, b), t| {
            let (tn, tb) = t.budget();
            (n + tn, b + tb)
        });
        ensure!(
            nodes <= MAX_NODES && bytes <= MAX_TEXT,
            "E_IMPORT_UI_EXPR_LIMIT: expanded expression"
        );
        let (op, arity) = match opcode {
            0 | 1 => {
                ensure!(args.len() == 1, "E_IMPORT_UI_EXPR_ARITY");
                locals.insert(destination.clone(), args[0].clone());
                continue;
            }
            2 => (Op::Add, 2),
            3 => (Op::Subtract, 2),
            4 => (Op::Multiply, 2),
            5 => (Op::Divide, 2),
            6 => (Op::Remainder, 2),
            7 => (Op::SourceOr, 2),
            8 => (Op::SourceAnd, 2),
            9 => (Op::SourceXor, 2),
            10 => (Op::Index, 2),
            12 => (Op::Equal, 2),
            13 => (Op::Greater, 2),
            14 => (Op::Less, 2),
            15 => (Op::GreaterEqual, 2),
            16 => (Op::LessEqual, 2),
            19 => (Op::Concat, 2),
            20 => (Op::NotEqual, 2),
            11 => match expression.functions.get(&index) {
                Some(2) => (Op::Property, 2),
                Some(4) => (Op::ArraySize, 1),
                Some(15) => (Op::StringPosition, 2),
                Some(19) => (Op::ObjectExists, 1),
                Some(20) => (Op::Not, 1),
                Some(30) => (Op::IndexOfString, 2),
                Some(81) => (Op::IsDelimiter, 3),
                Some(126) => (Op::Min, 2),
                Some(127) => (Op::Max, 2),
                Some(133) => (Op::AddDelimiter, 2),
                _ => bail!("E_IMPORT_UI_EXPR_CALL: unrecognized or effectful function"),
            },
            _ => bail!("E_IMPORT_UI_EXPR_OP: unsupported operator {opcode}"),
        };
        ensure!(args.len() == arity, "E_IMPORT_UI_EXPR_ARITY");
        locals.insert(destination.clone(), apply(op, args)?);
    }
    let destination = &expression.operations.last().unwrap().1;
    ensure!(
        destination.is_empty() || destination == "____arg",
        "E_IMPORT_UI_EXPR_RESULT: no expression result"
    );
    Ok(locals.remove(destination))
}

/// Describe a single final assignment for profile matching. Earlier writes are
/// still rejected, and no value is ever written into a source environment.
pub(super) fn assignment(expression: &Expression) -> Result<(String, Term)> {
    ensure!(
        expression.operations.len() <= 64,
        "E_IMPORT_UI_EXPR_LIMIT: instructions"
    );
    let Some((opcode, destination, args)) = expression.operations.last() else {
        bail!("E_IMPORT_UI_EXPR_ASSIGNMENT: missing assignment");
    };
    ensure!(
        *opcode == 1 && args.len() == 1 && !temporary(destination) && destination.len() <= 1024,
        "E_IMPORT_UI_EXPR_ASSIGNMENT: expected final source assignment"
    );
    let mut value = expression.clone();
    value.operations.last_mut().unwrap().1 = "____arg".into();
    let result = normalize(&value)?
        .ok_or_else(|| anyhow::anyhow!("E_IMPORT_UI_EXPR_ASSIGNMENT: missing value"))?;
    Ok((destination.clone(), result))
}

/// Recognize the stock initializer's literal StringToArray write. It is an
/// explicitly described write, not an addition to the pure-call whitelist.
pub(super) fn literal_array_write(expression: &Expression) -> Result<(String, Vec<String>)> {
    ensure!(
        expression.operations.len() <= 64,
        "E_IMPORT_UI_ARRAY_LIMIT: instructions"
    );
    let calls: Vec<_> = expression
        .functions
        .iter()
        .filter(|(_, f)| **f == 29)
        .collect();
    ensure!(
        calls.len() == 1,
        "E_IMPORT_UI_ARRAY: expected one StringToArray"
    );
    let index = *calls[0].0;
    let Some((11, result, args)) = expression.operations.get(index) else {
        bail!("E_IMPORT_UI_ARRAY: invalid call metadata");
    };
    ensure!(
        temporary(result) && args.len() == 3,
        "E_IMPORT_UI_ARRAY: expected explicit delimiter and temporary result"
    );
    let Literal::Variable(destination) = &args[1] else {
        bail!("E_IMPORT_UI_ARRAY: expected named output");
    };
    ensure!(
        !temporary(destination) && !destination.starts_with('@') && destination.len() <= 1024,
        "E_IMPORT_UI_ARRAY: invalid output"
    );
    let tail = &expression.operations[index + 1..];
    ensure!(
        matches!(tail, [(1, name, values)] if (name.is_empty() || name == "____arg") && matches!(values.as_slice(), [Literal::Variable(v)] if v == result)),
        "E_IMPORT_UI_ARRAY: result must be discarded without further effects"
    );
    ensure!(
        expression.functions.keys().all(|i| *i <= index),
        "E_IMPORT_UI_ARRAY: trailing call metadata"
    );
    let resolve = |value: &Literal| -> Result<String> {
        let mut prefix = Expression {
            literal: None,
            operations: expression.operations[..index].to_vec(),
            functions: expression
                .functions
                .iter()
                .filter(|(i, _)| **i < index)
                .map(|(i, f)| (*i, *f))
                .collect(),
        };
        prefix
            .operations
            .push((1, "____arg".into(), vec![value.clone()]));
        let Some(Term::String { value }) = normalize(&prefix)? else {
            bail!("E_IMPORT_UI_ARRAY: nonliteral input or delimiter");
        };
        Ok(value)
    };
    let text = resolve(&args[0])?;
    let delimiter = resolve(&args[2])?;
    // Empty fields, escaping and alternate delimiter conventions need their
    // own source proof. This profile uses a plain, nonempty comma list.
    ensure!(delimiter == ",", "E_IMPORT_UI_ARRAY: unsupported delimiter");
    let values: Vec<_> = text.split(',').map(str::to_owned).collect();
    ensure!(
        values.len() <= 256
            && values
                .iter()
                .all(|s| !s.is_empty() && s.len() <= 1024 && !s.contains(['\r', '\n'])),
        "E_IMPORT_UI_ARRAY_LIMIT: invalid or excessive items"
    );
    Ok((destination.clone(), values))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn var(s: &str) -> Literal {
        Literal::Variable(s.into())
    }
    fn expr(operations: Vec<(u8, &str, Vec<Literal>)>) -> Expression {
        Expression {
            literal: None,
            operations: operations
                .into_iter()
                .map(|(op, name, values)| (op, name.into(), values))
                .collect(),
            functions: BTreeMap::new(),
        }
    }
    #[test]
    fn source_conditions_preserve_type_dependent_operators_and_object_reads() {
        for opcode in [7, 8, 9] {
            let e = expr(vec![(
                opcode,
                "____arg",
                vec![Literal::Int(2), Literal::Int(1)],
            )]);
            let result = normalize(&e).unwrap().unwrap();
            assert!(matches!(result, Term::Apply { .. }));
            assert_eq!(result.truth(), None);
        }
        let mut e = expr(vec![
            (11, "____0", vec![Literal::String("choice".into())]),
            (11, "____1", vec![var("____0")]),
            (8, "____arg", vec![var("read"), var("____1")]),
        ]);
        e.functions = BTreeMap::from([(0, 19), (1, 20)]);
        let result = normalize(&e).unwrap().unwrap();
        assert!(matches!(
            result,
            Term::Apply {
                op: Op::SourceAnd,
                ..
            }
        ));
        assert_eq!(result.truth(), None);
        // Recognizing an observational call must not hide an effectful one.
        e.functions.insert(0, 26);
        assert!(normalize(&e).is_err());
    }
    #[test]
    fn menu_list_operations_remain_symbolic_and_preserve_argument_order() {
        for (id, op) in [
            (15, Op::StringPosition),
            (30, Op::IndexOfString),
            (133, Op::AddDelimiter),
        ] {
            let mut e = expr(vec![(11, "____arg", vec![var("first"), var("second")])]);
            e.functions.insert(0, id);
            assert_eq!(
                normalize(&e).unwrap(),
                Some(Term::Apply {
                    op,
                    args: vec![
                        Term::Read {
                            name: "first".into()
                        },
                        Term::Read {
                            name: "second".into()
                        }
                    ]
                })
            );
            e.operations[0].2.pop();
            assert!(normalize(&e).is_err());
        }
        let mut e = expr(vec![
            (
                11,
                "____0",
                vec![Literal::String("\r\n".into()), var("labels")],
            ),
            (10, "____1", vec![var("source_labels"), var("i")]),
            (19, "____2", vec![var("____0"), var("____1")]),
            (1, "labels", vec![var("____2")]),
        ]);
        e.functions.insert(0, 133);
        let (destination, result) = assignment(&e).unwrap();
        assert_eq!(destination, "labels");
        assert!(matches!(result, Term::Apply { op: Op::Concat, .. }));
        assert!(normalize(&e).is_err());
    }
    #[test]
    fn normalizes_layout_reads_without_evaluating_engine_state() {
        let mut e = expr(vec![
            (
                11,
                "____0",
                vec![Literal::String("slider".into()), Literal::Int(5)],
            ),
            (2, "____1", vec![var("____0"), Literal::Int(16)]),
            (1, "____arg", vec![var("____1")]),
        ]);
        e.functions.insert(0, 2);
        let t = normalize(&e).unwrap().unwrap();
        assert_eq!(
            t,
            Term::Apply {
                op: Op::Add,
                args: vec![
                    Term::Apply {
                        op: Op::Property,
                        args: vec![
                            Term::String {
                                value: "slider".into()
                            },
                            Term::Int { value: 5 }
                        ]
                    },
                    Term::Int { value: 16 }
                ]
            }
        );
        assert_eq!(t.truth(), None);
    }
    #[test]
    fn folds_only_proven_constants_and_preserves_rounding_questions() {
        let e = expr(vec![
            (
                12,
                "____0",
                vec![Literal::String("".into()), Literal::String("".into())],
            ),
            (1, "____arg", vec![var("____0")]),
        ]);
        assert_eq!(normalize(&e).unwrap().unwrap().truth(), Some(true));
        let e = expr(vec![(5, "____arg", vec![Literal::Int(5), Literal::Int(2)])]);
        assert!(matches!(
            normalize(&e).unwrap(),
            Some(Term::Apply { op: Op::Divide, .. })
        ));
        for (a, b) in [(1, 0), (i32::MIN, -1)] {
            let e = expr(vec![(5, "____arg", vec![Literal::Int(a), Literal::Int(b)])]);
            assert!(matches!(
                normalize(&e).unwrap(),
                Some(Term::Apply { op: Op::Divide, .. })
            ));
        }
        let e = expr(vec![(
            4,
            "____arg",
            vec![Literal::Int(i32::MAX), Literal::Int(2)],
        )]);
        assert!(normalize(&e).unwrap_err().to_string().contains("OVERFLOW"));
    }
    #[test]
    fn rejects_writes_calls_missing_temps_and_expansion_bombs() {
        assert!(normalize(&expr(vec![(1, "global", vec![Literal::Int(1)])])).is_err());
        assert!(normalize(&expr(vec![(1, "____arg", vec![var("____9")])])).is_err());
        let mut call = expr(vec![(11, "____arg", vec![Literal::Int(1)])]);
        call.functions.insert(0, 26); // Random must never become a layout constant.
        call.literal = Some(Literal::Int(1)); // A cached value is not evidence of purity.
        assert!(normalize(&call).is_err());
        let mut bomb = expr(vec![(1, "____0", vec![var("external")])]);
        for i in 0..10 {
            let previous = format!("____{i}");
            bomb.operations.push((
                2,
                format!("____{}", i + 1),
                vec![var(&previous), var(&previous)],
            ));
        }
        bomb.operations
            .push((1, "____arg".into(), vec![var("____10")]));
        assert!(normalize(&bomb).unwrap_err().to_string().contains("LIMIT"));
    }
    #[test]
    fn source_alias_writes_cannot_hide_in_an_otherwise_constant_expression() {
        let e = expr(vec![
            (10, "____d_0", vec![var("items"), Literal::Int(0)]),
            (1, "____d_0", vec![Literal::Int(1)]),
            (1, "____arg", vec![Literal::Int(1)]),
        ]);
        assert!(normalize(&e).unwrap_err().to_string().contains("WRITE"));
        let mut e = expr(vec![(
            11,
            "____arg",
            vec![
                Literal::String("#".into()),
                var("component"),
                Literal::Int(1),
            ],
        )]);
        e.functions.insert(0, 81);
        assert!(matches!(
            normalize(&e).unwrap(),
            Some(Term::Apply {
                op: Op::IsDelimiter,
                ..
            })
        ));
        e.operations[0].2.pop();
        assert!(normalize(&e).is_err());
    }
    #[test]
    fn assignment_matching_never_accepts_an_earlier_side_effect() {
        let mut e = expr(vec![
            (10, "____d_0", vec![var("input"), Literal::Int(0)]),
            (4, "____1", vec![var("____d_0"), Literal::Int(1000)]),
            (1, "setting", vec![var("____1")]),
        ]);
        assert!(normalize(&e).is_err());
        let (name, value) = assignment(&e).unwrap();
        assert_eq!(name, "setting");
        assert!(matches!(
            value,
            Term::Apply {
                op: Op::Multiply,
                ..
            }
        ));
        e.operations
            .insert(0, (1, "side_effect".into(), vec![Literal::Int(1)]));
        assert!(assignment(&e).is_err());
    }
    #[test]
    fn literal_array_write_keeps_order_and_rejects_extra_effects() {
        let mut e = expr(vec![
            (1, "____0", vec![Literal::String(",".into())]),
            (
                11,
                "____1",
                vec![
                    Literal::String("second,first".into()),
                    var("items"),
                    var("____0"),
                ],
            ),
            (1, "____arg", vec![var("____1")]),
        ]);
        e.functions.insert(1, 29);
        assert!(normalize(&e).is_err());
        assert_eq!(
            literal_array_write(&e).unwrap(),
            ("items".into(), vec!["second".into(), "first".into()])
        );
        let mut bad = e.clone();
        bad.operations[0].1 = "source_write".into();
        assert!(literal_array_write(&bad).is_err());
        let mut bad = e.clone();
        bad.operations[1].2[0] = var("dynamic_input");
        assert!(literal_array_write(&bad).is_err());
        let mut bad = e.clone();
        bad.operations[2].1 = "source_write".into();
        assert!(literal_array_write(&bad).is_err());
        let mut bad = e.clone();
        bad.operations[1].2[0] = Literal::String(vec!["item"; 257].join(","));
        assert!(literal_array_write(&bad).is_err());
        e.operations[0].2[0] = Literal::String("".into());
        assert!(literal_array_write(&e).is_err());
    }
}
