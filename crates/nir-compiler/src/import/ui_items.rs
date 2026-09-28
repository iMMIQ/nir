//! Source menu label/action pairs, extracted from the stock initializer.
//! This is declaration evidence; it does not certify callback dispatch or UI.
use super::{
    lsb::{Body, Literal, Script},
    ui_expr, Source, SourceLocation,
};
use anyhow::{ensure, Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const SOURCE: &str = "ノベルシステム/■初期化.lsb";
const LABELS: &str = "システムメニュー項目名";
const ACTIONS: &str = "システムメニュー項目値";

#[derive(Serialize)]
struct InitializerWrite {
    location: SourceLocation,
    not_update: bool,
}

#[derive(Serialize)]
pub(super) struct MenuItems {
    format: u32,
    status: &'static str,
    source: &'static str,
    source_sha256: String,
    source_version: u32,
    writes: BTreeMap<String, InitializerWrite>,
    pub items: Vec<MenuItem>,
}
#[derive(Serialize)]
pub(super) struct MenuItem {
    pub index: usize,
    pub label: String,
    pub action: String,
}
impl MenuItems {
    pub fn load(source: &mut Source) -> Result<Self> {
        let (_, script) = source.read(SOURCE)?;
        Self::from_script(&script)
    }
    fn from_script(script: &Script) -> Result<Self> {
        let mut declarations = BTreeSet::new();
        let mut arrays = BTreeMap::new();
        let mut writes = BTreeMap::new();
        for (index, c) in script.commands.iter().enumerate() {
            if c.muted {
                continue;
            }
            match &c.body {
                Body::Variable {
                    name,
                    value_type,
                    initial,
                    scope,
                } if [LABELS, ACTIONS].contains(&name.as_str()) => {
                    ensure!(
                        c.indent == 0
                            && *value_type == 4
                            && *scope == 0
                            && initial.operations.is_empty(),
                        "E_IMPORT_MENU_ITEMS: unsupported array declaration"
                    );
                    ensure!(
                        declarations.insert(name.clone()),
                        "E_IMPORT_MENU_ITEMS: duplicate array declaration"
                    );
                    ensure!(
                        !arrays.contains_key(name),
                        "E_IMPORT_MENU_ITEMS: declaration after initializer"
                    );
                }
                Body::Calc(expression) => {
                    let refers = expression.operations.iter().any(|(_, name, args)| [LABELS, ACTIONS].contains(&name.as_str()) || args.iter().any(|a| matches!(a, Literal::Variable(v) if [LABELS, ACTIONS].contains(&v.as_str()))));
                    if !refers {
                        continue;
                    }
                    ensure!(
                        c.indent == 0,
                        "E_IMPORT_MENU_ITEMS: conditional array initializer"
                    );
                    let (name, values) = ui_expr::literal_array_write(expression)
                        .context("E_IMPORT_MENU_ITEMS: unsupported menu array write")?;
                    ensure!(
                        [LABELS, ACTIONS].contains(&name.as_str()) && declarations.contains(&name),
                        "E_IMPORT_MENU_ITEMS: missing declaration or indirect array use"
                    );
                    ensure!(
                        arrays.insert(name.clone(), values).is_none(),
                        "E_IMPORT_MENU_ITEMS: repeated array write"
                    );
                    writes.insert(
                        name,
                        InitializerWrite {
                            location: SourceLocation {
                                source: SOURCE.into(), index, line: c.line, byte: c.offset, command: c.name().into(),
                            },
                            not_update: c.not_update,
                        },
                    );
                }
                Body::GetProperty { destination, .. } if [LABELS, ACTIONS].contains(&destination.as_str()) => {
                    anyhow::bail!("E_IMPORT_MENU_ITEMS: property read overwrites menu array");
                }
                Body::LoopUpdate { expression, .. } if expression.operations.iter().any(|(_, name, args)| [LABELS, ACTIONS].contains(&name.as_str()) || args.iter().any(|a| matches!(a, Literal::Variable(v) if [LABELS, ACTIONS].contains(&v.as_str())))) => {
                    anyhow::bail!("E_IMPORT_MENU_ITEMS: loop update refers to menu array");
                }
                _ => {}
            }
        }
        let labels = arrays
            .remove(LABELS)
            .context("E_IMPORT_MENU_ITEMS: missing labels")?;
        let actions = arrays
            .remove(ACTIONS)
            .context("E_IMPORT_MENU_ITEMS: missing actions")?;
        ensure!(
            labels.len() == actions.len(),
            "E_IMPORT_MENU_ITEMS: labels/actions length mismatch"
        );
        // Source dispatch looks up a label's first index. Duplicate labels
        // would make the requested action ambiguous and cannot be rebound.
        ensure!(
            labels.iter().collect::<BTreeSet<_>>().len() == labels.len(),
            "E_IMPORT_MENU_ITEMS: ambiguous duplicate labels"
        );
        ensure!(
            actions.iter().collect::<BTreeSet<_>>().len() == actions.len(),
            "E_IMPORT_MENU_ITEMS: duplicate action identifiers"
        );
        Ok(Self {
            format: 1,
            status: "declarations_extracted_not_lowered",
            source: SOURCE,
            source_sha256: script.source_sha256.clone(),
            source_version: script.version,
            writes,
            items: labels
                .into_iter()
                .zip(actions)
                .enumerate()
                .map(|(index, (label, action))| MenuItem {
                    index,
                    label,
                    action,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::lsb::{Command, Expression};
    use super::*;
    fn script(labels: &str, actions: &str) -> Script {
        let command = |body| Command {
            kind: if matches!(body, Body::Variable { .. }) {
                15
            } else {
                14
            },
            body,
            indent: 0,
            muted: false,
            not_update: false,
            line: 0,
            offset: 0,
        };
        let mut commands = vec![];
        for (name, list) in [(LABELS, labels), (ACTIONS, actions)] {
            commands.push(command(Body::Variable {
                name: name.into(),
                value_type: 4,
                initial: Expression::default(),
                scope: 0,
            }));
            commands.push(command(Body::Calc(Expression {
                literal: None,
                operations: vec![
                    (
                        11,
                        "____0".into(),
                        vec![
                            Literal::String(list.into()),
                            Literal::Variable(name.into()),
                            Literal::String(",".into()),
                        ],
                    ),
                    (1, "____arg".into(), vec![Literal::Variable("____0".into())]),
                ],
                functions: BTreeMap::from([(0, 29)]),
            })));
        }
        Script {
            source_sha256: "a".repeat(64),
            version: 116,
            commands,
        }
    }
    #[test]
    fn menu_actions_follow_source_ids_not_display_labels() {
        let mut source = script("Load,Save", "save,load");
        for c in &mut source.commands {
            c.not_update = true;
        }
        let model = MenuItems::from_script(&source).unwrap();
        assert_eq!(model.items[0].label, "Load");
        assert_eq!(model.items[0].action, "save");
        assert_eq!(model.items[1].index, 1);
        assert_eq!(model.writes.len(), 2);
        assert_eq!(model.source_sha256, "a".repeat(64));
        assert!(model.writes.values().all(|w| w.not_update));
    }
    #[test]
    fn refuses_ambiguous_or_dynamic_menu_tables() {
        for (labels, actions) in [
            ("A,B", "one"),
            ("A,A", "one,two"),
            ("A,B", "one,one"),
            ("A,", "one,two"),
        ] {
            assert!(MenuItems::from_script(&script(labels, actions)).is_err());
        }
        let mut p = script("A,B", "one,two");
        p.commands[1].indent = 1;
        assert!(MenuItems::from_script(&p).is_err());
        p.commands[1].indent = 0;
        p.commands.push(p.commands[1].clone());
        assert!(MenuItems::from_script(&p).is_err());
    }
}
