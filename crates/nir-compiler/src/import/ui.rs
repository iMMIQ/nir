//! Source-side UI analysis, not a runtime script interpreter. Property identities
//! and parsed expressions remain available for checked profile lowering.
use super::{
    lsb::{Body, Expression, Literal, Script},
    ui_flow::{Flow, Guard, Normalized},
    ImportDiagnostic, Source, SourceLocation,
};
use anyhow::{ensure, Result};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Default, Serialize)]
pub struct UiInventory {
    pub object_commands: usize,
    pub property_writes: usize,
    pub literal_properties: usize,
    pub computed_properties: usize,
    pub empty_properties: usize,
    pub deferred_commands: usize,
}
impl UiInventory {
    pub(super) fn from_script(script: &Script) -> Self {
        let mut result = Self::default();
        for command in &script.commands {
            let expressions: Vec<_> = match &command.body {
                Body::Object(properties) => {
                    result.object_commands += 1;
                    properties.values().collect()
                }
                Body::SetProperty {
                    target,
                    property,
                    value,
                } => {
                    result.property_writes += 1;
                    vec![target, property, value]
                }
                _ => continue,
            };
            result.deferred_commands += usize::from(command.not_update);
            for expression in expressions {
                if expression.operations.is_empty() {
                    result.empty_properties += 1;
                } else if expression.literal.is_some() {
                    result.literal_properties += 1;
                } else {
                    result.computed_properties += 1;
                }
            }
        }
        result
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UiStatement {
    HistoryCall {
        parameters: BTreeMap<String, Expression>,
        normalized_parameters: BTreeMap<String, Normalized>,
    },
    HistoryFormat {
        name: Expression,
        target: Expression,
        normalized: Box<[Normalized; 2]>,
    },
    Cabinet {
        properties: BTreeMap<u16, Expression>,
        normalized_properties: BTreeMap<u16, Normalized>,
        act: Expression,
        normalized_act: Normalized,
        targets: Vec<Expression>,
        normalized_targets: Vec<Normalized>,
    },
    Flip {
        parameters: BTreeMap<String, Expression>,
        normalized_parameters: BTreeMap<String, Normalized>,
        targets: Vec<Expression>,
        normalized_targets: Vec<Normalized>,
    },
    Variable {
        name: String,
        value_type: u8,
        scope: u8,
        initial: Expression,
        normalized: Normalized,
    },
    GetProperty {
        target: Expression,
        property: Expression,
        destination: String,
        normalized: Box<[Normalized; 2]>,
    },
    Calculation {
        expression: Expression,
        assignment: Assignment,
    },
    LoopUpdate {
        expression: Expression,
        target: Option<u32>,
        assignment: Assignment,
    },
    Object {
        properties: BTreeMap<u16, Expression>,
        normalized: BTreeMap<u16, Normalized>,
    },
    SetProperty {
        target: Expression,
        property: Expression,
        value: Box<Expression>,
        normalized: Box<[Normalized; 3]>,
    },
}
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Assignment {
    SingleWrite {
        destination: String,
        value: super::ui_expr::Term,
    },
    Unresolved {
        diagnostic: String,
    },
}
impl Assignment {
    fn new(expression: &Expression) -> Self {
        match super::ui_expr::assignment(expression) {
            Ok((destination, value)) => Self::SingleWrite { destination, value },
            Err(error) => Self::Unresolved {
                diagnostic: error.to_string(),
            },
        }
    }
}
#[derive(Serialize)]
struct UiCommand {
    location: SourceLocation,
    scope_depth: u32,
    muted: bool,
    not_update: bool,
    statement: UiStatement,
    guards: Vec<Guard>,
    excluded_by_constant_branch: bool,
}
#[derive(Serialize)]
struct SourceIdentity {
    sha256: String,
    version: u32,
}
#[derive(Serialize)]
pub(super) struct UiAnalysis {
    format: u32,
    scope: &'static str,
    status: &'static str,
    declaration_property_base: u8,
    runtime_property_base: u8,
    sources: BTreeMap<String, SourceIdentity>,
    commands: Vec<UiCommand>,
    data_flow: Vec<UiCommand>,
    errors: BTreeMap<String, String>,
}
impl UiAnalysis {
    pub(super) fn diagnostics(&self) -> Vec<ImportDiagnostic> {
        let mut out = vec![];
        for command in &self.commands {
            if command.muted {
                continue;
            }
            let location = &command.location;
            // Report one actionable source location per creator rather than
            // pretending that retaining its properties implements the page.
            let UiStatement::Object { properties, .. } = &command.statement else {
                continue;
            };
            let computed = properties
                .values()
                .filter(|e| !e.operations.is_empty() && e.literal.is_none())
                .count();
            let message = if location.command == "SliderNew" {
                let integer = |id| match properties.get(&id).and_then(|e| e.literal.as_ref()) {
                    Some(Literal::Int(n)) => Some(*n),
                    _ => None,
                };
                format!("E_IMPORT_UI_UNMAPPED: source slider retained (min={:?}, max={:?}, step={:?}, {} computed properties). Callback binding, units and source part geometry require lowering; this control is not emitted.", integer(124), integer(125), integer(128), computed)
            } else {
                format!("E_IMPORT_UI_UNMAPPED: source UI object retained ({} declared properties, {} computed). Source conditions, layout and callbacks are not yet lowered; this object is not emitted.",properties.len(),computed)
            };
            out.push(ImportDiagnostic {
                severity: "warning".into(),
                source: location.source.clone(),
                index: location.index,
                line: location.line,
                byte: location.byte,
                command: location.command.clone(),
                message,
            });
        }
        for (source, message) in &self.errors {
            out.push(ImportDiagnostic {
                severity: "warning".into(),
                source: source.clone(),
                index: 0,
                line: 0,
                byte: 0,
                command: "UiAnalysis".into(),
                message: format!("E_IMPORT_UI_ANALYSIS: {message}"),
            });
        }
        for command in &self.data_flow {
            if command.muted || command.excluded_by_constant_branch {
                continue;
            }
            if matches!(
                command.statement,
                UiStatement::HistoryCall { .. } | UiStatement::HistoryFormat { .. }
            ) {
                let location = &command.location;
                out.push(ImportDiagnostic {
                    severity: "warning".into(),
                    source: location.source.clone(),
                    index: location.index,
                    line: location.line,
                    byte: location.byte,
                    command: location.command.clone(),
                    message: "E_IMPORT_UI_HISTORY_UNMAPPED: source history parameters retained in original units. Continuous scrolling, page gaps and formatter semantics require lowering; no entry-based NIR history action is substituted.".into(),
                });
            }
        }
        out
    }
}

/// Bounded audit of the recognized profile's system-menu and history directories.
/// It makes no claim about reachability or other UI script families.
pub(super) fn analyze_system_menu(source: &mut Source) -> Result<UiAnalysis> {
    let mut report = UiAnalysis {
        format: 3,
        scope: "livenovel116-system-menu-and-history-directories",
        status: "parsed_not_lowered",
        declaration_property_base: 1,
        runtime_property_base: 0,
        sources: BTreeMap::new(),
        commands: vec![],
        data_flow: vec![],
        errors: BTreeMap::new(),
    };
    for path in super::files(&source.root)? {
        let relative = path
            .strip_prefix(&source.root)?
            .to_string_lossy()
            .replace('\\', "/");
        if !(relative.starts_with("ノベルシステム/システムメニュー/")
            || relative.starts_with("ノベルシステム/シナリオ回想/"))
            || !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("lsb"))
        {
            continue;
        }
        let script = match source.read(&relative) {
            Ok((_, script)) => script,
            Err(error) => {
                report.errors.insert(relative, format!("{error:#}"));
                continue;
            }
        };
        report.sources.insert(
            relative.clone(),
            SourceIdentity {
                sha256: script.source_sha256.clone(),
                version: script.version,
            },
        );
        let mut flow = Flow::default();
        for (index, command) in script.commands.iter().enumerate() {
            if let Err(error) = flow.enter(command, index) {
                report
                    .errors
                    .insert(relative.clone(), format!("{error:#} at command {index}"));
                break;
            }
            let statement = match &command.body {
                Body::HistoryCall { parameters } => UiStatement::HistoryCall {
                    parameters: parameters.clone(),
                    normalized_parameters: parameters
                        .iter()
                        .map(|(name, e)| (name.clone(), Normalized::new(e)))
                        .collect(),
                },
                Body::HistoryFormat { name, target } => UiStatement::HistoryFormat {
                    name: name.clone(),
                    target: target.clone(),
                    normalized: Box::new([Normalized::new(name), Normalized::new(target)]),
                },
                Body::Cabinet {
                    properties,
                    act,
                    targets,
                } => UiStatement::Cabinet {
                    properties: properties.clone(),
                    normalized_properties: properties
                        .iter()
                        .map(|(id, e)| (*id, Normalized::new(e)))
                        .collect(),
                    act: act.clone(),
                    normalized_act: Normalized::new(act),
                    targets: targets.clone(),
                    normalized_targets: targets.iter().map(Normalized::new).collect(),
                },
                Body::Flip {
                    parameters,
                    targets,
                } => UiStatement::Flip {
                    parameters: parameters.clone(),
                    normalized_parameters: parameters
                        .iter()
                        .map(|(name, e)| (name.clone(), Normalized::new(e)))
                        .collect(),
                    targets: targets.clone(),
                    normalized_targets: targets.iter().map(Normalized::new).collect(),
                },
                Body::Variable {
                    name,
                    value_type,
                    initial,
                    scope,
                } => UiStatement::Variable {
                    name: name.clone(),
                    value_type: *value_type,
                    scope: *scope,
                    initial: initial.clone(),
                    normalized: Normalized::new(initial),
                },
                Body::GetProperty {
                    target,
                    property,
                    destination,
                } => UiStatement::GetProperty {
                    target: target.clone(),
                    property: property.clone(),
                    destination: destination.clone(),
                    normalized: Box::new([Normalized::new(target), Normalized::new(property)]),
                },
                Body::Calc(expression) => UiStatement::Calculation {
                    expression: expression.clone(),
                    assignment: Assignment::new(expression),
                },
                Body::LoopUpdate { expression, target } => UiStatement::LoopUpdate {
                    expression: expression.clone(),
                    target: *target,
                    assignment: Assignment::new(expression),
                },
                Body::Object(properties) => UiStatement::Object {
                    properties: properties.clone(),
                    normalized: properties
                        .iter()
                        .map(|(id, e)| (*id, Normalized::new(e)))
                        .collect(),
                },
                Body::SetProperty {
                    target,
                    property,
                    value,
                } => UiStatement::SetProperty {
                    target: target.clone(),
                    property: property.clone(),
                    value: Box::new(value.clone()),
                    normalized: Box::new([
                        Normalized::new(target),
                        Normalized::new(property),
                        Normalized::new(value),
                    ]),
                },
                _ => continue,
            };
            ensure!(
                report.commands.len() + report.data_flow.len() < 4096,
                "E_IMPORT_UI_LIMIT: more than 4096 system UI statements"
            );
            let output = if matches!(
                statement,
                UiStatement::Object { .. } | UiStatement::SetProperty { .. }
            ) {
                &mut report.commands
            } else {
                &mut report.data_flow
            };
            output.push(UiCommand {
                location: SourceLocation {
                    source: relative.clone(),
                    index,
                    line: command.line,
                    byte: command.offset,
                    command: command.name().into(),
                },
                scope_depth: command.indent,
                muted: command.muted,
                not_update: command.not_update,
                statement,
                excluded_by_constant_branch: flow.guards().iter().any(Guard::excludes),
                guards: flow.guards(),
            });
        }
    }
    report.status = if !report.errors.is_empty() {
        "partially_parsed_not_lowered"
    } else if report.sources.is_empty() {
        "not_present"
    } else {
        "parsed_not_lowered"
    };
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::lsb::tests::{integer, object_script};
    #[test]
    fn history_analysis_keeps_source_units_guards_and_scope() {
        use crate::import::lsb::tests::{command, script};
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("ノベルシステム/シナリオ回想");
        std::fs::create_dir_all(&directory).unwrap();
        let mut args = vec![];
        for n in [7, -80, 600, 0, 0] {
            args.extend(integer(n));
        }
        let mut excluded = command(29, 11, &args);
        excluded[1..5].copy_from_slice(&1u32.to_le_bytes());
        let bytes = script(&[
            command(0, 10, &integer(0)),
            excluded,
            command(29, 12, &args),
            command(58, 13, &[integer(9), integer(7)].concat()),
        ]);
        std::fs::write(directory.join("fixture.lsb"), &bytes).unwrap();
        // A sibling with a matching name prefix is outside the audited scope.
        let outside = temp.path().join("ノベルシステム/シナリオ回想-other");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("bad.lsb"), [0]).unwrap();
        let report = analyze_system_menu(&mut Source::new(temp.path()).unwrap()).unwrap();
        assert_eq!(report.format, 3);
        assert_eq!(report.sources.len(), 1);
        assert!(report.errors.is_empty());
        assert_eq!(report.status, "parsed_not_lowered");
        assert_eq!(report.data_flow.len(), 3);
        assert!(report.data_flow[0].excluded_by_constant_branch);
        assert!(!report.data_flow[1].excluded_by_constant_branch);
        let UiStatement::HistoryCall { parameters, .. } = &report.data_flow[1].statement else {
            panic!()
        };
        assert!(matches!(
            parameters["index"].literal,
            Some(Literal::Int(-80))
        ));
        assert!(matches!(
            parameters["count"].literal,
            Some(Literal::Int(600))
        ));
        let diagnostics = report.diagnostics();
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics
            .iter()
            .all(|d| d.message.starts_with("E_IMPORT_UI_HISTORY_UNMAPPED")));
        assert_eq!(diagnostics[0].line, 12);
    }
    #[test]
    #[ignore = "requires NIR_IMPORT_SOURCE and NIR_IMPORT_UI_REPORT"]
    fn source_system_ui_analysis() {
        let source = std::env::var_os("NIR_IMPORT_SOURCE").expect("NIR_IMPORT_SOURCE");
        let out = std::env::var_os("NIR_IMPORT_UI_REPORT").expect("NIR_IMPORT_UI_REPORT");
        let mut source = Source::new(std::path::Path::new(&source)).unwrap();
        super::super::livenovel::verify_auto_timer(&mut source).unwrap();
        let items = super::super::ui_items::MenuItems::load(&mut source).unwrap();
        assert!(!items.items.is_empty());
        let report = analyze_system_menu(&mut source).unwrap();
        super::super::write_json(std::path::Path::new(&out), &report).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(!report.commands.is_empty());
        assert!(report
            .data_flow
            .iter()
            .any(|c| matches!(c.statement, UiStatement::Variable { .. })));
        assert!(report
            .data_flow
            .iter()
            .any(|c| matches!(c.statement, UiStatement::LoopUpdate { .. })));
        assert!(report.data_flow.iter().any(|c| matches!(
            c.statement,
            UiStatement::Calculation {
                assignment: Assignment::SingleWrite { .. },
                ..
            }
        )));
        assert!(report.data_flow.iter().any(|c| matches!(
            c.statement,
            UiStatement::Calculation {
                assignment: Assignment::Unresolved { .. },
                ..
            }
        )));
        assert!(report
            .commands
            .iter()
            .any(|c| c.excluded_by_constant_branch));
        assert!(report
            .commands
            .iter()
            .any(|c| c.guards.iter().any(|g| g.loop_target.is_some())));
        assert!(report
            .commands
            .iter()
            .any(|c| c.guards.iter().any(|g| g.normalized.truth().is_none())));
        let history: Vec<_> = report
            .data_flow
            .iter()
            .filter(|c| matches!(c.statement, UiStatement::HistoryCall { .. }))
            .collect();
        assert_eq!(history.len(), 4);
        for call in history {
            let UiStatement::HistoryCall {
                parameters,
                normalized_parameters,
            } = &call.statement
            else {
                unreachable!()
            };
            assert_eq!(parameters.len(), 5);
            assert!(parameters["format_name"].operations.is_empty());
            assert!(matches!(
                parameters["cut_break"].literal,
                Some(Literal::Int(0))
            ));
            assert!(matches!(
                normalized_parameters["index"],
                Normalized::Value { .. }
            ));
            assert!(matches!(
                normalized_parameters["count"],
                Normalized::Value { .. }
            ));
        }
        assert_eq!(
            report
                .diagnostics()
                .iter()
                .filter(|d| d.message.starts_with("E_IMPORT_UI_HISTORY_UNMAPPED"))
                .count(),
            4
        );
        for (relative, identity) in &report.sources {
            let bytes = super::super::read_binary(&source.path(relative).unwrap()).unwrap();
            assert_eq!(identity.sha256, nir_content::digest(&bytes));
        }
    }
    #[test]
    fn source_ui_report_keeps_original_units_and_does_not_claim_lowering() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("ノベルシステム/システムメニュー");
        std::fs::create_dir_all(&directory).unwrap();
        let bytes = object_script(
            51,
            &[(124, integer(0)), (125, integer(1000)), (128, integer(10))],
        );
        std::fs::write(directory.join("fixture.lsb"), &bytes).unwrap();
        let mut source = Source::new(temp.path()).unwrap();
        let report = analyze_system_menu(&mut source).unwrap();
        assert_eq!(report.status, "parsed_not_lowered");
        assert_eq!(report.commands.len(), 1);
        assert_eq!(
            report.sources.values().next().unwrap().sha256,
            nir_content::digest(&bytes)
        );
        assert_eq!(report.commands[0].location.line, 17);
        let diagnostics = report.diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("max=Some(1000)"));
        assert!(diagnostics[0].message.contains("step=Some(10)"));
        let inventory = UiInventory::from_script(&super::super::lsb::parse(&bytes).unwrap());
        assert_eq!(inventory.object_commands, 1);
        assert_eq!(inventory.literal_properties, 3);
        let output = serde_json::to_string(&inventory).unwrap();
        assert!(!output.contains("1000"));
        std::fs::write(directory.join("broken.lsb"), [0]).unwrap();
        let report = analyze_system_menu(&mut source).unwrap();
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.status, "partially_parsed_not_lowered");
        assert!(report
            .diagnostics()
            .iter()
            .any(|d| d.message.starts_with("E_IMPORT_UI_ANALYSIS")));
    }
}
