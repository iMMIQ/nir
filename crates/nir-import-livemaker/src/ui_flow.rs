//! Lexical branch paths for source UI declarations. These are prerequisites for
//! lowering, not a reachability proof: calls, jumps, loops and mutations remain
//! source operations until a profile explicitly handles them.
use super::{
    lsb::{Body, Command, Expression},
    ui_expr::{self, Term},
};
use anyhow::{ensure, Context, Result};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Normalized {
    Empty,
    Value { expression: Term },
    Unsupported { diagnostic: String },
}
impl Normalized {
    pub fn new(e: &Expression) -> Self {
        match ui_expr::normalize(e) {
            Ok(Some(expression)) => Self::Value { expression },
            Ok(None) => Self::Empty,
            Err(error) => Self::Unsupported {
                diagnostic: error.to_string(),
            },
        }
    }
    pub fn truth(&self) -> Option<bool> {
        match self {
            Self::Value { expression } => expression.truth(),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub(super) struct Guard {
    pub index: usize,
    pub expected: bool,
    pub muted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_target: Option<u32>,
    pub source: Expression,
    pub normalized: Normalized,
}
impl Guard {
    fn new(command: &Command, index: usize, e: &Expression, target: Option<u32>) -> Self {
        Self {
            index,
            expected: true,
            muted: command.muted,
            loop_target: target,
            source: e.clone(),
            normalized: Normalized::new(e),
        }
    }
    pub fn excludes(&self) -> bool {
        !self.muted && self.normalized.truth().is_some_and(|v| v != self.expected)
    }
}
struct Frame {
    depth: u32,
    previous: Vec<Guard>,
    active: Vec<Guard>,
    branch: bool,
    closed: bool,
}
#[derive(Default)]
pub(super) struct Flow {
    frames: Vec<Frame>,
}
impl Flow {
    pub fn enter(&mut self, command: &Command, index: usize) -> Result<()> {
        self.frames.retain(|f| f.depth <= command.indent);
        if command.kind == 1 || command.kind == 2 {
            let frame = self
                .frames
                .last_mut()
                .context("E_IMPORT_UI_FLOW: branch without If")?;
            ensure!(
                frame.depth == command.indent && frame.branch && !frame.closed,
                "E_IMPORT_UI_FLOW: invalid Else/Elseif scope"
            );
            frame.active = frame.previous.clone();
            if let Body::Condition(e) = &command.body {
                let guard = Guard::new(command, index, e, None);
                frame.active.push(guard.clone());
                frame.previous.push(Guard {
                    expected: false,
                    ..guard
                });
            } else {
                ensure!(
                    command.kind == 2,
                    "E_IMPORT_UI_FLOW: missing Elseif condition"
                );
                frame.closed = true;
            }
        } else {
            self.frames.retain(|f| f.depth < command.indent);
            let guard = match &command.body {
                Body::Condition(e) if command.kind == 0 => {
                    Some(Guard::new(command, index, e, None))
                }
                Body::LoopCondition { condition, target } => {
                    Some(Guard::new(command, index, condition, Some(*target)))
                }
                _ => None,
            };
            if let Some(guard) = guard {
                self.frames.push(Frame {
                    depth: command.indent,
                    previous: vec![Guard {
                        expected: false,
                        ..guard.clone()
                    }],
                    active: vec![guard],
                    branch: command.kind == 0,
                    closed: false,
                });
            }
        }
        ensure!(
            self.frames
                .iter()
                .map(|f| f.active.len() + f.previous.len())
                .sum::<usize>()
                <= 128,
            "E_IMPORT_UI_FLOW_LIMIT: more than 64 branch prerequisites"
        );
        Ok(())
    }
    pub fn guards(&self) -> Vec<Guard> {
        self.frames
            .iter()
            .flat_map(|f| f.active.iter().cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::lsb::Literal;
    use super::*;
    fn command(kind: u8, indent: u32, value: i32) -> Command {
        Command {
            kind,
            indent,
            muted: false,
            not_update: false,
            line: 0,
            offset: 0,
            body: if kind <= 1 {
                Body::Condition(Expression {
                    literal: None,
                    operations: vec![(1, "____arg".into(), vec![Literal::Int(value)])],
                    functions: Default::default(),
                })
            } else {
                Body::Other
            },
        }
    }
    #[test]
    fn nested_else_paths_include_prior_negations_and_end_at_parent_scope() {
        let mut flow = Flow::default();
        flow.enter(&command(0, 0, 0), 0).unwrap();
        flow.enter(&command(8, 1, 0), 1).unwrap();
        assert!(flow.guards().iter().any(Guard::excludes));
        flow.enter(&command(1, 0, 1), 2).unwrap();
        flow.enter(&command(0, 1, 0), 3).unwrap();
        flow.enter(&command(2, 1, 0), 4).unwrap();
        flow.enter(&command(8, 2, 0), 5).unwrap();
        assert_eq!(flow.guards().len(), 3);
        assert!(!flow.guards().iter().any(Guard::excludes));
        flow.enter(&command(2, 0, 0), 6).unwrap();
        flow.enter(&command(8, 1, 0), 7).unwrap();
        assert!(flow.guards().iter().any(Guard::excludes));
        flow.enter(&command(8, 0, 0), 8).unwrap();
        assert!(flow.guards().is_empty());
        assert!(flow.enter(&command(2, 0, 0), 9).is_err());
    }
    #[test]
    fn muted_condition_is_not_used_to_prove_exclusion() {
        let mut flow = Flow::default();
        let mut c = command(0, 0, 0);
        c.muted = true;
        flow.enter(&c, 0).unwrap();
        flow.enter(&command(8, 1, 0), 1).unwrap();
        assert!(!flow.guards().iter().any(Guard::excludes));
        assert!(flow.guards()[0].muted);
    }
}
