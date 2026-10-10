//! Certify lossless numeric assignments across the complete lowered graph.
//! A quantum q means every reachable value is a multiple of 2^q. Narrowing
//! a float to an integer is emitted only when q >= 0, so no source rounding
//! policy is guessed. Unknown writes and independently editable values weaken
//! the proof, including writes in choices, replays and other charts.
use super::*;
use nir_format::ValueType;

const UNKNOWN: i32 = -1_000_000;
const ZERO: i32 = 64;

pub(super) fn coerce(
    adapter: &mut Adapter,
    name: &str,
    value: Value,
    ty: ValueType,
) -> (Value, ValueType) {
    match (
        adapter.variables.get(name).and_then(|v| v["type"].as_str()),
        ty,
    ) {
        (Some("f80"), ValueType::I32) => (json!({"type":"to_f80","value":value}), ValueType::F80),
        (Some("i32"), ValueType::F80) => {
            adapter
                .implicit_integer_assignments
                .push((value.clone(), adapter.location.clone()));
            (json!({"type":"to_i32","value":value}), ValueType::I32)
        }
        _ => (value, ty),
    }
}

fn bounded(power: i32) -> i32 {
    if power < -64 {
        UNKNOWN
    } else {
        power.min(ZERO)
    }
}

fn literal(value: &Value) -> i32 {
    match value["type"].as_str() {
        Some("i32") => value["value"]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .map_or(UNKNOWN, |n| {
                if n == 0 {
                    ZERO
                } else {
                    n.unsigned_abs().trailing_zeros() as i32
                }
            }),
        Some("f80") => serde_json::from_value::<nir_format::Float80>(value["value"].clone())
            .map_or(UNKNOWN, |f| {
                if f.is_zero() {
                    return ZERO;
                }
                let bits = f.bits();
                let exponent = ((bits >> 64) & 0x7fff) as i32;
                bounded(exponent.max(1) - 16383 - 63 + (bits as u64).trailing_zeros() as i32)
            }),
        _ => UNKNOWN,
    }
}

fn kind<'a>(expression: &'a Value, variables: &'a BTreeMap<String, Value>) -> Option<&'a str> {
    match expression["type"].as_str()? {
        "const" => expression["value"]["type"].as_str(),
        "var" => variables
            .get(expression["name"].as_str()?)
            .and_then(|v| v["type"].as_str()),
        "to_i32" => Some("i32"),
        "to_f80" => Some("f80"),
        "binary" => kind(&expression["left"], variables),
        _ => None,
    }
}

fn power_of_two(expression: &Value) -> Option<i32> {
    if expression["type"] == "to_f80" {
        return power_of_two(&expression["value"]);
    }
    if expression["type"] != "const" {
        return None;
    }
    let value = &expression["value"];
    match value["type"].as_str()? {
        "i32" => {
            let n = i32::try_from(value["value"].as_i64()?).ok()?.unsigned_abs();
            n.is_power_of_two().then_some(n.trailing_zeros() as i32)
        }
        "f80" => {
            let f: nir_format::Float80 = serde_json::from_value(value["value"].clone()).ok()?;
            let bits = f.bits();
            (bits as u64).is_power_of_two().then(|| {
                (((bits >> 64) & 0x7fff) as i32).max(1) - 16383 - 63
                    + (bits as u64).trailing_zeros() as i32
            })
        }
        _ => None,
    }
}

fn quantum(
    expression: &Value,
    facts: &BTreeMap<String, i32>,
    variables: &BTreeMap<String, Value>,
) -> i32 {
    match expression["type"].as_str() {
        Some("const") => literal(&expression["value"]),
        Some("var") => expression["name"]
            .as_str()
            .and_then(|n| facts.get(n))
            .copied()
            .unwrap_or(UNKNOWN),
        Some("to_f80") => quantum(&expression["value"], facts, variables),
        Some("to_i32") => quantum(&expression["value"], facts, variables).clamp(0, 31),
        Some("binary") => {
            let a = quantum(&expression["left"], facts, variables);
            let b = quantum(&expression["right"], facts, variables);
            match expression["op"].as_str() {
                Some("add" | "sub") => a.min(b),
                Some("mul") if a != UNKNOWN && b != UNKNOWN => bounded(a + b),
                Some("div" | "rem") if kind(expression, variables) == Some("i32") => 0,
                Some("div") if a != UNKNOWN => {
                    power_of_two(&expression["right"]).map_or(UNKNOWN, |p| bounded(a - p))
                }
                Some("rem") if a != UNKNOWN && b != UNKNOWN => a.min(b),
                _ => UNKNOWN,
            }
        }
        _ => UNKNOWN,
    }
}

pub(super) fn certify(adapter: &Adapter) -> Result<()> {
    if adapter.implicit_integer_assignments.is_empty() {
        return Ok(());
    }
    let mut facts: BTreeMap<_, _> = adapter
        .variables
        .iter()
        .map(|(name, value)| {
            let q = if adapter.status_values.contains_key(name)
                || adapter.status_flags.contains_key(name)
            {
                if value["type"] == "i32" {
                    0
                } else {
                    UNKNOWN
                }
            } else {
                literal(value)
            };
            (name.clone(), q)
        })
        .collect();
    let mut writes = vec![];
    for function in adapter.functions.values() {
        for block in function["blocks"]
            .as_object()
            .context("E_IMPORT_NUMERIC: missing blocks")?
            .values()
        {
            for op in block["ops"]
                .as_array()
                .context("E_IMPORT_NUMERIC: missing operations")?
            {
                let op = &op["operation"];
                let Some(target) = op["target"].as_str() else {
                    continue;
                };
                let value = match op["type"].as_str() {
                    Some("assign" | "profile_value_assign") => op["value"].clone(),
                    // Independent progress recovery and random draws may
                    // produce any value of the slot's declared type.
                    Some("profile_read" | "profile_value_read" | "random") => {
                        if adapter
                            .variables
                            .get(target)
                            .is_some_and(|v| v["type"] == "i32")
                        {
                            json!({"type":"const","value":{"type":"i32","value":1}})
                        } else {
                            Value::Null
                        }
                    }
                    _ => continue,
                };
                writes.push((target.to_owned(), value));
            }
            let term = &block["terminator"];
            if let Some(target) = term["result"].as_str() {
                if term["type"] == "interact" {
                    if let Some(choice) = term["choice"]
                        .as_str()
                        .and_then(|id| adapter.choices.get(id))
                    {
                        for option in choice["options"]
                            .as_array()
                            .context("E_IMPORT_NUMERIC: choice options")?
                        {
                            writes.push((
                                target.to_owned(),
                                json!({"type":"const","value":option["value"]}),
                            ));
                        }
                    } else {
                        writes.push((target.to_owned(), Value::Null));
                    }
                } else if term["type"] == "call" {
                    writes.push((target.to_owned(), Value::Null));
                }
            }
        }
    }
    // Monotone weakening over a finite lattice; cyclic arithmetic can only
    // lose precision. Exotic slow cycles are conservatively rejected.
    let mut stable = false;
    for _ in 0..(facts.len() + 1) * 160 {
        let mut changed = false;
        for (target, expression) in &writes {
            let value = quantum(expression, &facts, &adapter.variables);
            if let Some(old) = facts.get_mut(target) {
                let next = (*old).min(value);
                changed |= next != *old;
                *old = next;
            }
        }
        if !changed {
            stable = true;
            break;
        }
    }
    ensure!(
        stable,
        "E_IMPORT_NUMERIC: assignment analysis did not converge"
    );
    for (value, source) in &adapter.implicit_integer_assignments {
        ensure!(
            quantum(value, &facts, &adapter.variables) >= 0,
            "E_IMPORT_NUMERIC_COERCION: {}:{}: fractional integer assignment is not certified",
            source.source,
            source.line
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn integer(n: i32) -> Value {
        json!({"type":"const","value":{"type":"i32","value":n}})
    }
    fn variable(name: &str) -> Value {
        json!({"type":"var","name":name})
    }
    fn binary(op: &str, left: Value, right: Value) -> Value {
        json!({"type":"binary","op":op,"left":left,"right":right})
    }
    fn widened(value: Value) -> Value {
        json!({"type":"to_f80","value":value})
    }
    #[test]
    fn integral_income_proof_includes_all_routes_and_independent_values() {
        let temp = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
        adapter
            .variables
            .insert("income".into(), json!({"type":"i32","value":0}));
        adapter
            .variables
            .insert("balance".into(), json!({"type":"i32","value":0}));
        adapter.variables.insert(
            "rate".into(),
            json!({"type":"f80","value":"3ffd8000000000000000"}),
        );
        let income = binary("mul", widened(variable("income")), variable("rate"));
        let total = binary("add", widened(variable("balance")), income);
        let (narrowed, ty) = coerce(&mut adapter, "balance", total, ValueType::F80);
        assert_eq!(ty, ValueType::I32);
        adapter.functions.insert("main".into(), json!({"blocks":{"body":{"ops":[
            {"operation":{"type":"assign","target":"income","value":integer(1_005_504)}},
            {"operation":{"type":"assign","target":"income","value":binary("add",variable("income"),integer(1000))}},
            {"operation":{"type":"assign","target":"balance","value":narrowed}}
        ],"terminator":{"type":"end"}}}}));
        certify(&adapter).unwrap();
        // A write in another chart or replay invalidates a global invariant.
        adapter.functions.insert(
            "alternate".into(),
            json!({"blocks":{"body":{"ops":[
            {"operation":{"type":"assign","target":"income","value":integer(1)}}
        ],"terminator":{"type":"end"}}}}),
        );
        assert!(certify(&adapter)
            .unwrap_err()
            .to_string()
            .contains("E_IMPORT_NUMERIC_COERCION"));
        adapter.functions.remove("alternate");
        adapter
            .status_values
            .insert("income".into(), "independent-income".into());
        assert!(certify(&adapter).is_err());
    }
    #[test]
    fn fractional_values_are_rejected_and_widening_preserves_persistent_types() {
        let temp = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(temp.path()).unwrap());
        adapter
            .variables
            .insert("count".into(), json!({"type":"i32","value":0}));
        adapter.variables.insert(
            "rate".into(),
            json!({"type":"f80","value":"3ffd8000000000000000"}),
        );
        let (value, ty) = coerce(&mut adapter, "rate", integer(1), ValueType::I32);
        assert_eq!(ty, ValueType::F80);
        assert_eq!(value["type"], "to_f80");
        coerce(
            &mut adapter,
            "count",
            json!({"type":"const","value":{"type":"f80","value":"3fffc000000000000000"}}),
            ValueType::F80,
        );
        assert!(certify(&adapter).is_err());
        let vars = BTreeMap::new();
        let facts = BTreeMap::new();
        assert_eq!(
            quantum(
                &binary("div", widened(integer(4)), widened(integer(2))),
                &facts,
                &vars
            ),
            1
        );
        assert_eq!(
            quantum(
                &binary("div", widened(integer(4)), widened(integer(3))),
                &facts,
                &vars
            ),
            UNKNOWN
        );
        assert_eq!(
            quantum(
                &binary(
                    "mul",
                    widened(integer(-12)),
                    json!({"type":"const","value":{"type":"f80","value":"3ffd8000000000000000"}})
                ),
                &facts,
                &vars
            ),
            0
        );
    }
}
