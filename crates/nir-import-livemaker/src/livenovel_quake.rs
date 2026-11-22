//! Finite stock text-content quake. Paired system-program fingerprints pin
//! the inspected callback/motion convention; customized helpers remain errors.
//! No original script body is embedded or interpreted by the generated game.
use super::*;
use nir_format::{DialogueShake, Micros};

pub(super) fn index_cg_names(adapter: &mut Adapter, page: &str, script: &Script) -> Result<()> {
    if !adapter.indexed_cg_scripts.insert(page.to_owned()) {
        return Ok(());
    }
    for command in script.commands.iter().filter(|command| !command.muted) {
        let Body::Text { text, .. } = &command.body else {
            continue;
        };
        for glyph in &text.glyphs {
            let Glyph::Event(fields) = glyph else {
                continue;
            };
            if fields.len() >= 8
                && fields[0].trim_start_matches('\u{1}') == "CREATECG"
                && matches!(fields[3].as_str(), "NORMAL" | "WAIT")
                && !fields[1].is_empty()
                && fields[1].len() <= 256
                && !fields[1].starts_with(['?', '@'])
            {
                adapter.declared_cg_names.insert(fields[1].clone());
            }
        }
    }
    ensure!(
        adapter.declared_cg_names.len() <= 10_000,
        "E_IMPORT_LIMIT: declared CG identities"
    );
    Ok(())
}

fn convention(adapter: &mut Adapter) -> Result<()> {
    let (_, events) = adapter
        .source
        .read("ノベルシステム/メッセージボックス/イベント.lsb")?;
    let (_, quake) = adapter
        .source
        .read("ノベルシステム/メッセージボックス/画面揺らし.lsb")?;
    // The event branches and complete quake callback AST are equivalent in
    // these variants after rebasing loop targets and removing file offsets.
    // Pin whole-file pairs so changes elsewhere never widen this convention.
    const VERIFIED: [(&str, &str); 3] = [
        (
            "177cdcd83fb945b0ed94d7e5f9a1d129dcb55bc7f9c0e38cfcf8bca1dd330c7c",
            "464b274bcecd06a4ca457af8c598a06428a734165a5451343eaad9ced2beae64",
        ),
        (
            "3e7df74bdfcbbca60e46863ab59f1b66e2f9c9ad3f99403cb3ab450ed39734fe",
            "464b274bcecd06a4ca457af8c598a06428a734165a5451343eaad9ced2beae64",
        ),
        (
            "522cf5722a0a2ce17561d0988e02e00663bae0dadcf5b3673f402b5fea90152e",
            "a663fcea169cde1059045ec7f03cc7cd5d112b55ba97efacdbd0f050bf3b2829",
        ),
    ];
    ensure!(
        events.version == 117
            && quake.version == 117
            && VERIFIED.contains(&(events.source_sha256.as_str(), quake.source_sha256.as_str())),
        "E_IMPORT_QUAKE: unverified stock helper pair"
    );
    Ok(())
}

pub(super) fn event(
    adapter: &mut Adapter,
    name: &str,
    args: &[String],
    blocks: &mut Vec<Value>,
) -> Result<()> {
    convention(adapter)?;
    lower(adapter, name, args, blocks)
}

fn lower(
    adapter: &mut Adapter,
    name: &str,
    args: &[String],
    blocks: &mut Vec<Value>,
) -> Result<()> {
    if name == "QUAKESTOP" {
        ensure!(
            args.len() == 1 && args[0].parse::<u64>().is_ok_and(|n| n <= 60000),
            "E_IMPORT_QUAKE: literal bounded stop required"
        );
        // All admitted quakes are finite and awaited. The verified stoptimer
        // callback checks MotionX0 first; when it is absent it resets offsets
        // and deletes the stop timer immediately, irrespective of requested
        // decay duration. Indefinite/custom motions remain unsupported.
        return Ok(());
    }
    let [target, mode, random, x, y, duration, period] = args else {
        bail!("E_IMPORT_QUAKE: arguments");
    };
    ensure!(
        matches!(mode.as_str(), "QUAKE" | "WAVE" | "BOUND") && matches!(random.as_str(), "0" | "1"),
        "E_IMPORT_QUAKE: unsupported target or policy"
    );
    let duration = duration
        .parse::<u64>()
        .context("E_IMPORT_QUAKE: literal duration")?;
    let period = period
        .parse::<u64>()
        .context("E_IMPORT_QUAKE: literal period")?;
    let spec = DialogueShake {
        amplitude: [
            x.parse().context("E_IMPORT_QUAKE: literal X")?,
            y.parse().context("E_IMPORT_QUAKE: literal Y")?,
        ],
        step_us: Micros(
            period
                .checked_div(2)
                .and_then(|n| n.checked_mul(1000))
                .context("E_IMPORT_QUAKE: period overflow")?,
        ),
        duration_us: Micros(
            duration
                .checked_mul(1000)
                .context("E_IMPORT_QUAKE: duration overflow")?,
        ),
        randomize: random == "1",
    };
    ensure!(spec.valid(), "E_IMPORT_QUAKE: finite bounds");
    if target != "メッセージボックス" || mode != "QUAKE" {
        let nodes: Vec<_> = target.split(',').map(str::to_owned).collect();
        ensure!(
            !nodes.is_empty()
                && nodes.len() <= 32
                && nodes.iter().all(|name| !name.is_empty()
                    && name.len() <= 256
                    && !name.starts_with(['?', '@'])
                    && (adapter.declared_cg_names.contains(name)
                        || adapter.nodes.contains_key(name)
                        || adapter
                            .scenes
                            .values()
                            .flatten()
                            .any(|node| node.id == *name && node.parent.is_none())))
                && nodes.iter().collect::<BTreeSet<_>>().len() == nodes.len(),
            "E_IMPORT_QUAKE: sprite wave needs known unique literal CG targets"
        );
        // Certified PropMotion finishes immediately for absent visual names.
        // The callback's separate lifetime timer still keeps its finite wait.
        // Admit only literal source CG identities, including later declarations
        // and branches where the image is absent; system/custom targets
        // and dynamic names remain errors instead of disappearing silently.
        let nodes: Vec<_> = nodes
            .into_iter()
            .filter(|name| adapter.nodes.contains_key(name))
            .collect();
        if nodes.is_empty() {
            adapter.effect(
                blocks,
                "lm_quake",
                "session",
                json!({"type":"delay","duration_us":spec.duration_us}),
                true,
            );
            return Ok(());
        }
        let effect = if mode == "WAVE" && !spec.randomize {
            json!({"type":"sprite_wave","nodes":nodes,"spec":spec})
        } else {
            json!({"type":"sprite_shake","nodes":nodes,"mode":mode.to_ascii_lowercase(),"spec":spec})
        };
        adapter.effect(blocks, "lm_quake", "scene", effect, true);
        return Ok(());
    }
    adapter.effect(
        blocks,
        "lm_quake",
        "session",
        json!({"type":"dialogue_shake","spec":spec}),
        true,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_absent_sprites_keep_finite_wait_and_mixed_lists_keep_present_targets() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Adapter::new(Source::new(dir.path()).unwrap());
        let node: Node =
            serde_json::from_value(json!({"id":"gone","x":0,"y":0,"width":32,"height":24}))
                .unwrap();
        a.scenes.insert("previous".into(), vec![node]);
        let mut blocks = vec![];
        let args = ["gone", "BOUND", "1", "0", "30", "500", "25"].map(str::to_owned);
        lower(&mut a, "QUAKE", &args, &mut blocks).unwrap();
        let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
        assert_eq!(
            a.cues[cue]["effects"][0]["effect"],
            json!({"type":"delay","duration_us":"500000"})
        );
        assert_eq!(blocks[1]["terminator"]["type"], "await");
        a.nodes.insert(
            "present".into(),
            serde_json::from_value(json!({"id":"present","x":0,"y":0,"width":32,"height":24}))
                .unwrap(),
        );
        let mut mixed = args.clone();
        mixed[0] = "gone,present".into();
        lower(&mut a, "QUAKE", &mixed, &mut blocks).unwrap();
        let cue = blocks[2]["terminator"]["cue"].as_str().unwrap();
        assert_eq!(
            a.cues[cue]["effects"][0]["effect"]["nodes"],
            json!(["present"])
        );
        for target in ["unknown", "?dynamic", "@Sender", "gone,gone"] {
            let mut bad = args.clone();
            bad[0] = target.into();
            assert!(lower(&mut a, "QUAKE", &bad, &mut blocks).is_err());
        }
    }
    #[test]
    fn finite_text_quake_waits_and_rejects_other_targets_or_indefinite_timing() {
        let dir = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(dir.path()).unwrap());
        let mut blocks = vec![];
        let args = ["メッセージボックス", "QUAKE", "1", "15", "15", "500", "30"].map(str::to_owned);
        lower(&mut adapter, "QUAKE", &args, &mut blocks).unwrap();
        let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
        let effect = &adapter.cues[cue]["effects"][0];
        assert_eq!(effect["scope"], "session");
        assert_eq!(effect["effect"]["spec"]["step_us"], "15000");
        assert_eq!(effect["effect"]["spec"]["duration_us"], "500000");
        assert_eq!(blocks[1]["terminator"]["conditions"][0]["task"], "lm_quake");
        assert_eq!(
            blocks[1]["terminator"]["conditions"][0]["milestone"]["type"],
            "finished"
        );
        let mut other = args.clone();
        other[0] = "background".into();
        assert!(lower(&mut adapter, "QUAKE", &other, &mut blocks).is_err());
        other = args.clone();
        other[5] = "0".into();
        assert!(lower(&mut adapter, "QUAKE", &other, &mut blocks).is_err());
        other = args;
        other[1] = "WAVE".into();
        assert!(lower(&mut adapter, "QUAKE", &other, &mut blocks).is_err());
        let count = blocks.len();
        lower(&mut adapter, "QUAKESTOP", &["0".into()], &mut blocks).unwrap();
        assert_eq!(blocks.len(), count);
        lower(&mut adapter, "QUAKESTOP", &["500".into()], &mut blocks).unwrap();
        assert_eq!(blocks.len(), count);
        assert!(lower(&mut adapter, "QUAKESTOP", &["60001".into()], &mut blocks).is_err());
    }
    #[test]
    fn random_sprite_bound_and_wave_use_independent_frozen_trajectories() {
        let dir = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(dir.path()).unwrap());
        for name in ["left", "right"] {
            adapter.nodes.insert(
                name.into(),
                serde_json::from_value(json!({"id":name,"x":0,"y":0,"width":32,"height":24}))
                    .unwrap(),
            );
        }
        for mode in ["WAVE", "BOUND", "QUAKE"] {
            let mut blocks = vec![];
            let args = ["left,right", mode, "1", "0", "30", "500", "25"].map(str::to_owned);
            lower(&mut adapter, "QUAKE", &args, &mut blocks).unwrap();
            let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
            let effect = &adapter.cues[cue]["effects"][0]["effect"];
            assert_eq!(effect["type"], "sprite_shake");
            assert_eq!(effect["mode"], mode.to_ascii_lowercase());
            assert_eq!(effect["nodes"], json!(["left", "right"]));
            assert_eq!(effect["spec"]["step_us"], "12000");
            assert_eq!(blocks[1]["terminator"]["type"], "await");
            let n = blocks.len();
            lower(&mut adapter, "QUAKESTOP", &["100".into()], &mut blocks).unwrap();
            assert_eq!(n, blocks.len());
        }
    }

    #[test]
    fn finite_sprite_wave_requires_existing_targets_and_keeps_scene_wait() {
        let dir = tempfile::tempdir().unwrap();
        let mut adapter = Adapter::new(Source::new(dir.path()).unwrap());
        for name in ["face", "body"] {
            adapter.nodes.insert(
                name.into(),
                serde_json::from_value(json!({
                "id":name,"x":0,"y":0,"width":20,"height":20}))
                .unwrap(),
            );
        }
        let mut blocks = vec![];
        let args = ["face,body", "WAVE", "0", "0", "10", "750", "300"].map(str::to_owned);
        lower(&mut adapter, "QUAKE", &args, &mut blocks).unwrap();
        let cue = blocks[0]["terminator"]["cue"].as_str().unwrap();
        assert_eq!(
            adapter.cues[cue]["effects"][0]["effect"]["type"],
            "sprite_wave"
        );
        assert_eq!(
            adapter.cues[cue]["effects"][0]["effect"]["nodes"],
            json!(["face", "body"])
        );
        assert_eq!(adapter.cues[cue]["effects"][0]["scope"], "scene");
        assert_eq!(blocks[1]["terminator"]["type"], "await");
        for (index, value) in [(0, "face,missing"), (0, "face,face"), (2, "2")] {
            let mut invalid = args.clone();
            invalid[index] = value.into();
            assert!(lower(&mut adapter, "QUAKE", &invalid, &mut blocks).is_err());
        }
    }
}
