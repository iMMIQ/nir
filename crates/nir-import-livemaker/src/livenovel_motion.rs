//! Proven stock MOTION subset: literal local X/Y destination, linear movement,
//! explicit wait/pass and replacement of the named motion component.
use super::*;
use nir_format::Property;

pub(super) fn task_name(name: &str) -> String {
    let name = if name.is_empty() {
        "__モーション"
    } else {
        name
    };
    format!("lm_motion_{}", nir_content::digest(name.as_bytes()))
}

pub(super) fn event(adapter: &mut Adapter, args: &[String], blocks: &mut Vec<Value>) -> Result<()> {
    if args.get(2).is_some_and(|property| property == "11")
        || args
            .get(5)
            .is_some_and(|curve| matches!(curve.as_str(), "INC" | "DEC"))
    {
        let (_, events) = adapter
            .source
            .read("ノベルシステム/メッセージボックス/イベント.lsb")?;
        ensure!(
            events.version == 117
                && matches!(
                    events.source_sha256.as_str(),
                    "177cdcd83fb945b0ed94d7e5f9a1d129dcb55bc7f9c0e38cfcf8bca1dd330c7c"
                        | "3e7df74bdfcbbca60e46863ab59f1b66e2f9c9ad3f99403cb3ab450ed39734fe"
                ),
            "E_IMPORT_MOTION: unverified stock curve helper"
        );
    }
    lower(adapter, args, blocks)
}
fn lower(adapter: &mut Adapter, args: &[String], blocks: &mut Vec<Value>) -> Result<()> {
    ensure!(args.len() == 7, "E_IMPORT_MOTION: argument count");
    let [name, target, property, value, duration, easing, wait] = args else {
        unreachable!()
    };
    ensure!(
        !name.starts_with('?')
            && !target.starts_with('?')
            && matches!(easing.as_str(), "NORMAL" | "INC" | "DEC")
            && matches!(wait.as_str(), "PASS" | "WAIT"),
        "E_IMPORT_MOTION: dynamic name or unsupported motion policy"
    );
    let property = match property.as_str() {
        "3" => Property::X,
        "4" => Property::Y,
        "11" => Property::Opacity,
        _ => bail!("E_IMPORT_MOTION: unsupported source property"),
    };
    let to = value
        .parse::<i32>()
        .context("E_IMPORT_MOTION: dynamic destination")?;
    let duration = duration
        .parse::<u64>()
        .context("E_IMPORT_MOTION: dynamic duration")?;
    ensure!(
        to.abs_diff(0) <= 8192 && duration <= 60_000,
        "E_IMPORT_MOTION: bounds"
    );
    ensure!(
        property != Property::Opacity || ((0..=255).contains(&to) && easing == "NORMAL"),
        "E_IMPORT_MOTION: only bounded linear byte opacity is supported"
    );
    let task = task_name(name);
    let node = adapter
        .nodes
        .get_mut(target)
        .context("E_IMPORT_MOTION: missing target")?;
    if !node.preserve_pose.contains(&property) {
        node.preserve_pose.push(property);
    }
    node.set(
        property,
        if property == Property::Opacity {
            to as f32 / 255.
        } else {
            to as f32
        },
    );
    if let Some(anchor) = adapter.anchors.get_mut(target) {
        match property {
            Property::X => anchor.0 = to.to_string(),
            Property::Y => anchor.1 = to.to_string(),
            Property::Opacity => {}
            _ => unreachable!(),
        }
    }
    if matches!(property, Property::X | Property::Y) {
        adapter.centered.remove(target);
    }
    if adapter.motion_tasks.contains_key(&task) {
        adapter.op(
            blocks,
            json!({"type":"task_control","task":task,"action":"cancel"}),
        );
    }
    let effect = if property == Property::Opacity {
        json!({"type":"source_motion","node":target,"property":property,"to":to,"duration_us":(duration*1000).to_string(),"curve":"opacity_linear"})
    } else if easing == "NORMAL" {
        json!({"type":"clip","node":target,"property":property,"to":to,"duration_us":(duration*1000).to_string(),"replace":true})
    } else {
        json!({"type":"source_motion","node":target,"property":property,"to":to,"duration_us":(duration*1000).to_string(),"curve":easing.to_ascii_lowercase()})
    };
    adapter.effect(blocks, &task, "session", effect, wait == "WAIT");
    adapter.motion_tasks.insert(task, target.to_owned());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waitplay_resolves_the_named_motion_handle_and_preserves_click_policy() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Adapter::new(Source::new(dir.path()).unwrap());
        a.nodes.insert(
            "actor".into(),
            serde_json::from_value(json!({"id":"actor","x":0,"y":0,"width":32,"height":24}))
                .unwrap(),
        );
        let mut blocks = vec![];
        lower(
            &mut a,
            &["move", "actor", "3", "250", "200", "NORMAL", "PASS"].map(str::to_owned),
            &mut blocks,
        )
        .unwrap();
        let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
        let handle = a.cues[cue]["effects"][0]["id"].as_str().unwrap().to_owned();
        for mode in ["NORMAL", "CLICK"] {
            a.event(&["WAITPLAY", "move", mode].map(str::to_owned), &mut blocks)
                .unwrap();
            let wait = &blocks.last().unwrap()["terminator"];
            assert_eq!(wait["conditions"][0]["task"], handle);
            assert_eq!(wait["conditions"][0]["milestone"]["type"], "finished");
            assert_eq!(
                wait.get("on_advance").and_then(Value::as_str),
                (mode == "CLICK").then_some("NEXT")
            );
        }
        assert!(a
            .event(
                &["WAITPLAY", "unknown", "NORMAL"].map(str::to_owned),
                &mut blocks
            )
            .is_err());
    }
    #[test]
    fn byte_opacity_keeps_center_anchor_and_rejects_unverified_shapes_or_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Adapter::new(Source::new(dir.path()).unwrap());
        a.nodes.insert(
            "actor".into(),
            serde_json::from_value(json!({"id":"actor","x":10,"y":20,"width":32,"height":24}))
                .unwrap(),
        );
        a.centered.insert("actor".into());
        a.anchors.insert("actor".into(), ("C".into(), "B".into()));
        let mut blocks = vec![];
        let args = ["fade", "actor", "11", "220", "800", "NORMAL", "PASS"].map(str::to_owned);
        lower(&mut a, &args, &mut blocks).unwrap();
        assert!(a.centered.contains("actor"));
        assert_eq!(a.nodes["actor"].opacity, 220. / 255.);
        assert_eq!(a.nodes["actor"].preserve_pose, [Property::Opacity]);
        let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
        assert_eq!(
            a.cues[cue]["effects"][0]["effect"]["curve"],
            "opacity_linear"
        );
        for (index, value) in [(3, "256"), (3, "-1"), (5, "INC")] {
            let mut bad = args.clone();
            bad[index] = value.into();
            assert!(lower(&mut a, &bad, &mut blocks).is_err());
        }
    }
    #[test]
    fn literal_stock_curves_keep_named_replacement_and_authored_wait() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Adapter::new(Source::new(dir.path()).unwrap());
        a.nodes.insert(
            "actor".into(),
            serde_json::from_value(json!({"id":"actor","x":0,"y":0,"width":32,"height":24}))
                .unwrap(),
        );
        let mut blocks = vec![];
        for curve in ["INC", "DEC"] {
            lower(
                &mut a,
                &["move", "actor", "3", "250", "200", curve, "WAIT"].map(str::to_owned),
                &mut blocks,
            )
            .unwrap();
            let effect = &a.cues[blocks
                .iter()
                .rev()
                .find_map(|b| b["terminator"]["cue"].as_str())
                .unwrap()]["effects"][0]["effect"];
            assert_eq!(effect["type"], "source_motion");
            assert_eq!(effect["curve"], curve.to_ascii_lowercase());
            assert_eq!(effect["duration_us"], "200000");
            assert_eq!(blocks.last().unwrap()["terminator"]["type"], "await");
        }
        assert!(blocks
            .iter()
            .any(|b| b["ops"][0]["operation"]["action"] == "cancel"));
        assert!(event(
            &mut a,
            &["move", "actor", "3", "250", "200", "INC", "WAIT"].map(str::to_owned),
            &mut blocks
        )
        .is_err());
    }

    #[test]
    fn literal_motion_keeps_final_anchor_and_source_name_replacement_and_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Adapter::new(Source::new(dir.path()).unwrap());
        a.nodes.insert(
            "background".into(),
            serde_json::from_value(json!({"id":"background","x":0,"y":0,"width":100,"height":100}))
                .unwrap(),
        );
        a.anchors
            .insert("background".into(), ("C".into(), "T".into()));
        let mut blocks = vec![];
        let args = ["", "background", "4", "-180", "3500", "NORMAL", "PASS"].map(str::to_owned);
        event(&mut a, &args, &mut blocks).unwrap();
        let (task, target) = a.motion_tasks.first_key_value().unwrap();
        assert_eq!(target, "background");
        let task = task.clone();
        assert_eq!(a.nodes["background"].y, -180.);
        assert_eq!(a.nodes["background"].preserve_pose, vec![Property::Y]);
        assert_eq!(a.anchors["background"].1, "-180");
        let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
        assert_eq!(
            a.cues[cue]["effects"][0]["effect"]["duration_us"],
            "3500000"
        );
        assert_eq!(a.cues[cue]["effects"][0]["scope"], "session");
        let mut next = args.clone();
        next[3] = "-200".into();
        next[6] = "WAIT".into();
        event(&mut a, &next, &mut blocks).unwrap();
        assert_eq!(
            blocks[1]["ops"][0]["operation"],
            json!({"type":"task_control","task":task,"action":"cancel"})
        );
        assert_eq!(blocks[3]["terminator"]["type"], "await");
        let mut unsupported = args;
        unsupported[5] = "CUSTOM".into();
        assert!(event(&mut a, &unsupported, &mut blocks).is_err());
        a.event(
            &[
                "DELETECG".into(),
                "background".into(),
                "0".into(),
                "0".into(),
            ],
            &mut blocks,
        )
        .unwrap();
        assert!(a.motion_tasks.is_empty());
    }
}
