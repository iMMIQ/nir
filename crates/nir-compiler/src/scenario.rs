use crate::{load_project, relative};
use anyhow::{bail, Result};
use nir_core::{Core, CoreInput, TaskState, ValidatedProgram};
use nir_format::{Effect, Value};
use serde::Deserialize;
use std::path::Path;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    format: u32,
    id: String,
    entry: String,
    text_locale: String,
    steps: Vec<Step>,
    expect: Expect,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    AwaitChoice { id: String },
    Choose { option_id: String },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    outcome: String,
    #[serde(default)]
    affection: Option<i32>,
    #[serde(default)]
    variables: std::collections::BTreeMap<String, Value>,
}
pub fn test_project(root: &Path) -> Result<Vec<String>> {
    let p = load_project(root)?;
    let mut passed = vec![];
    for file in &p.manifest.inputs.scenarios {
        let path = relative(root, root, file)?;
        let s: Scenario = toml::from_str(&std::fs::read_to_string(path)?)?;
        if s.format != 1 || s.entry != p.program.entry {
            bail!("E_SCENARIO: invalid entry/format");
        }
        let mut core = Core::new(
            ValidatedProgram::new(p.program.clone())?,
            "scenario".into(),
            s.text_locale,
        )?;
        let mut cursor = 0;
        for sequence in 1..=10_000 {
            let input = if let Some(p) = &core.state().pending {
                CoreInput::Prepared { activation: p.id }
            } else if let Some(c) = &core.state().choice {
                if matches!(s.steps.get(cursor),Some(Step::AwaitChoice{id}) if id==&c.id) {
                    cursor += 1;
                }
                let Some(Step::Choose { option_id }) = s.steps.get(cursor) else {
                    bail!("E_SCENARIO: unexpected choice {} in {}", c.id, s.id);
                };
                let input = CoreInput::Choose {
                    interaction: c.interaction,
                    option: option_id.clone(),
                    sequence,
                };
                cursor += 1;
                input
            } else if let Some(t) = core.state().tasks.values().find(|t| {
                t.state == TaskState::Running
                    && matches!(t.effect, Effect::Audio { looped: false, .. })
            }) {
                CoreInput::AudioEnded { task: t.id }
            } else if let Some((_, d)) = core.dialogue() {
                if d.at_gate {
                    CoreInput::Time {
                        delta_us: 1_000_000,
                    }
                } else {
                    CoreInput::Advance {
                        interaction: d.interaction,
                        sequence,
                    }
                }
            } else {
                CoreInput::Time {
                    delta_us: 1_000_000,
                }
            };
            core.step(input, 10_000);
            if let Some(f) = &core.state().fault {
                bail!("E_SCENARIO: {}: {}", s.id, f);
            }
            if core.state().outcome.is_some() {
                break;
            }
        }
        if core.state().outcome.as_deref() != Some(&s.expect.outcome)
            || s.expect
                .affection
                .is_some_and(|v| core.state().variables.get("affection") != Some(&Value::I32(v)))
            || s.expect
                .variables
                .iter()
                .any(|(id, value)| core.state().variables.get(id) != Some(value))
            || cursor != s.steps.len()
        {
            bail!("E_SCENARIO: {} expectation failed", s.id);
        }
        passed.push(s.id);
    }
    Ok(passed)
}
