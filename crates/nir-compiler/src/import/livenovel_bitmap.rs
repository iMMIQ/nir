//! LiveNovel's fixed-cell CGCHAR events. Text is evaluated only at the source
//! create/change operation; later scenes read the explicit frozen capture slot.
use super::{route_support, Adapter, Node};
use anyhow::{bail, ensure, Context, Result};
use nir_format::{BitmapAnchor, BitmapText, ValueType};
use serde_json::{json, Value};

fn decode(text: &str) -> Result<String> {
    if !text.starts_with('#') {
        return Ok(text.to_owned());
    }
    let bytes = text[1..]
        .split('#')
        .map(|number| {
            number
                .parse::<u8>()
                .context("E_IMPORT_BITMAP: invalid escaped byte")
        })
        .collect::<Result<Vec<_>>>()?;
    // LiveNovel stores #decimal escapes as bytes of its CP932 source string,
    // including both bytes of Japanese variable names, rather than Unicode
    // scalar numbers. Use the same strict decoder as the LSB reader.
    encoding_rs::SHIFT_JIS
        .decode_without_bom_handling_and_without_replacement(&bytes)
        .map(|s| s.into_owned())
        .context("E_IMPORT_BITMAP: invalid escaped source encoding")
}

fn atlas_geometry(cell: [u32; 2], count: usize, size: [u32; 2]) -> bool {
    cell[0] > 0
        && cell[1] == size[1]
        && cell[0]
            .checked_mul(count as u32)
            .is_some_and(|used| used <= size[0] && size[0] - used < cell[0])
}

fn capture(
    adapter: &mut Adapter,
    slot: &str,
    text: &str,
    calc: &str,
    blocks: &mut Vec<Value>,
) -> Result<()> {
    let text = decode(text)?;
    let (value, initial) = match calc {
        "0" => (
            json!({"type":"const","value":{"type":"string","value":text}}),
            json!({"type":"string","value":""}),
        ),
        "1" => {
            let (mut value, mut ty) = route_support::expression(
                adapter,
                &super::super::ui_expr::Term::Read { name: text.clone() },
            )?;
            if ty == ValueType::F80 {
                // Some games declare integer HUD counters as source reals.
                // Admit them only after the complete route/replay graph proves
                // every possible value integral; never truncate fractions.
                adapter
                    .implicit_integer_assignments
                    .push((value.clone(), adapter.location.clone()));
                value = json!({"type":"to_i32","value":value});
                ty = ValueType::I32;
            }
            ensure!(
                matches!(ty, ValueType::I32 | ValueType::String),
                "E_IMPORT_BITMAP: unsupported capture type"
            );
            let initial = match ty {
                ValueType::I32 => json!({"type":"i32","value":0}),
                _ => json!({"type":"string","value":""}),
            };
            for op in adapter.profile_reads(&std::collections::BTreeSet::from([text])) {
                blocks.push(json!({"ops":[op],"terminator":{"type":"goto","target":"NEXT"}}));
            }
            (value, initial)
        }
        _ => bail!("E_IMPORT_BITMAP: unsupported expression flag"),
    };
    if let Some(previous) = adapter.variables.get(slot) {
        ensure!(
            adapter.bitmap_slots.contains(slot),
            "E_IMPORT_BITMAP: capture collides with source variable"
        );
        ensure!(
            previous["type"] == initial["type"],
            "E_IMPORT_BITMAP: capture slot changed type"
        );
    } else {
        adapter.bitmap_slots.insert(slot.to_owned());
        adapter.variables.insert(slot.to_owned(), initial);
    }
    adapter.op(blocks, json!({"type":"assign","target":slot,"value":value}));
    Ok(())
}

fn coordinate(value: &str, extent: f32, horizontal: bool) -> Result<(f32, BitmapAnchor)> {
    Ok(match value {
        "L" if horizontal => (0., BitmapAnchor::Start),
        "T" if !horizontal => (0., BitmapAnchor::Start),
        "C" => (extent, BitmapAnchor::Center),
        "R" if horizontal => (extent, BitmapAnchor::End),
        "B" if !horizontal => (extent, BitmapAnchor::End),
        _ => {
            let n = value
                .parse::<i32>()
                .context("E_IMPORT_BITMAP: dynamic coordinate")?;
            ensure!(n.abs_diff(0) <= 8192, "E_IMPORT_BITMAP: coordinate bounds");
            (n as f32, BitmapAnchor::Start)
        }
    })
}

pub(super) fn event(
    adapter: &mut Adapter,
    name: &str,
    args: &[String],
    blocks: &mut Vec<Value>,
) -> Result<()> {
    if name == "CGCHARCHG" {
        ensure!(args.len() == 3, "E_IMPORT_BITMAP: change argument count");
        let slot = adapter
            .nodes
            .get(&args[0])
            .and_then(|node| node.bitmap_text.as_ref())
            .context("E_IMPORT_BITMAP: change target is not bitmap text")?
            .slot
            .clone();
        capture(adapter, &slot, &args[1], &args[2], blocks)?;
        let parent = adapter.nodes[&args[0]].parent.clone();
        let extent = if let Some(parent) = parent {
            adapter.geometry_extent(&parent)?
        } else {
            adapter.stage.map(|n| n as f32)
        };
        let node = adapter.nodes.get_mut(&args[0]).unwrap();
        let recipe = node.bitmap_text.as_ref().unwrap();
        if recipe.x_anchor != BitmapAnchor::Start {
            node.x = extent[0];
        }
        if recipe.y_anchor != BitmapAnchor::Start {
            node.y = extent[1];
        }
        adapter.scene(blocks, 0);
        return Ok(());
    }
    ensure!(args.len() == 13, "E_IMPORT_BITMAP: create argument count");
    let [id, parent, x, y, priority, alignment, spacing, text, calc, path, cw, ch, alphabet] = args
    else {
        unreachable!()
    };
    ensure!(
        !id.is_empty() && !id.starts_with('?') && !parent.starts_with('?'),
        "E_IMPORT_BITMAP: dynamic node name"
    );
    let extent = if parent.is_empty() {
        adapter.stage.map(|n| n as f32)
    } else {
        let node = adapter
            .nodes
            .get(parent)
            .context("E_IMPORT_BITMAP: missing parent")?;
        ensure!(
            node.bitmap_text.is_none(),
            "E_IMPORT_BITMAP: bitmap parent needs dynamic layout"
        );
        adapter.geometry_extent(parent)?
    };
    let (asset, size) = adapter.image(&format!("グラフィック/{path}"))?;
    let cell = [cw.parse::<u32>()?, ch.parse::<u32>()?];
    let alphabet = decode(alphabet)?;
    ensure!(
        atlas_geometry(cell, alphabet.chars().count(), size),
        "E_IMPORT_BITMAP: atlas cell geometry differs from image"
    );
    let (x, x_anchor) = coordinate(x, extent[0], true)?;
    let (y, y_anchor) = coordinate(y, extent[1], false)?;
    let align = match alignment.as_str() {
        "L" => BitmapAnchor::Start,
        "C" => BitmapAnchor::Center,
        "R" => BitmapAnchor::End,
        _ => bail!("E_IMPORT_BITMAP: unsupported line alignment"),
    };
    let slot = format!("__nir_bitmap_{}", nir_content::digest(id.as_bytes()));
    capture(adapter, &slot, text, calc, blocks)?;
    let recipe = BitmapText {
        slot,
        alphabet,
        cell,
        line_spacing: spacing.parse()?,
        align,
        x_anchor,
        y_anchor,
    };
    let variables = adapter
        .variables
        .iter()
        .map(|(k, v)| Ok((k.clone(), serde_json::from_value(v.clone())?)))
        .collect::<Result<std::collections::BTreeMap<_, nir_format::Value>>>()?;
    ensure!(recipe.valid(&variables), "E_IMPORT_BITMAP: invalid recipe");
    adapter.nodes.insert(
        id.clone(),
        Node {
            id: id.clone(),
            parent: (!parent.is_empty()).then(|| parent.clone()),
            asset: Some(asset),
            x,
            y,
            width: 0.,
            height: 0.,
            scale: 1.,
            opacity: 1.,
            color: [1.; 4],
            order: priority.parse()?,
            clip: None,
            timeline_binding: None,
            inherit_existence: false,
            sprite_transform: None,
            bitmap_text: Some(recipe),
            preserve_pose: vec![],
            offset: [0.; 2],
        },
    );
    adapter.scene(blocks, 0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::Source;
    use super::*;

    #[test]
    fn escaped_bitmap_names_decode_source_bytes_and_atlas_keeps_partial_cell_padding() {
        assert_eq!(decode("#076#118").unwrap(), "Lv");
        assert_eq!(decode("#142#120#142#157#146#108").unwrap(), "支持値");
        assert_eq!(decode("支持値").unwrap(), "支持値");
        for value in ["#", "#142", "#256", "#1#", "#abc", "#-1"] {
            assert!(decode(value).is_err(), "{value}");
        }
        assert!(atlas_geometry([9, 15], 13, [117, 15]));
        assert!(atlas_geometry([9, 15], 13, [120, 15]));
        assert!(!atlas_geometry([9, 15], 13, [116, 15]));
        assert!(!atlas_geometry([9, 15], 13, [126, 15]));
        assert!(!atlas_geometry([9, 15], 13, [120, 30]));
        assert!(!atlas_geometry([0, 15], 13, [120, 15]));
    }

    #[test]
    fn real_bitmap_counters_require_whole_graph_integrality_proof() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Adapter::new(Source::new(dir.path()).unwrap());
        a.variables.insert(
            "counter".into(),
            json!({"type":"f80","value":nir_format::Float80::from_i32(3)}),
        );
        let mut blocks = vec![];
        capture(&mut a, "capture", "counter", "1", &mut blocks).unwrap();
        assert_eq!(blocks[0]["ops"][0]["operation"]["value"]["type"], "to_i32");
        a.finish_function("hud", blocks, json!({"type":"return"}));
        super::super::numeric::certify(&a).unwrap();
        let fraction = nir_format::Float80::from_i32(3)
            .checked_binary(nir_format::BinaryOp::Div, nir_format::Float80::from_i32(2))
            .unwrap();
        a.variables
            .insert("counter".into(), json!({"type":"f80","value":fraction}));
        assert!(super::super::numeric::certify(&a).is_err());
    }

    #[test]
    fn bitmap_events_bind_original_atlas_and_capture_only_explicit_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("グラフィック")).unwrap();
        let mut header = vec![0; 47];
        header[..7].copy_from_slice(b"Gale106");
        header[15..19].copy_from_slice(&100u32.to_le_bytes());
        header[19..23].copy_from_slice(&20u32.to_le_bytes());
        std::fs::write(dir.path().join("グラフィック/digits.gal"), header).unwrap();
        let mut a = Adapter::new(Source::new(dir.path()).unwrap());
        a.variables
            .insert("turn".into(), json!({"type":"i32","value":12}));
        let mut blocks = vec![];
        let args = [
            "digits",
            "",
            "R",
            "C",
            "100",
            "L",
            "2",
            "#116#117#114#110",
            "1",
            "digits.gal",
            "10",
            "20",
            "0123456789",
        ]
        .map(str::to_owned);
        event(&mut a, "CGCHARNEW", &args, &mut blocks).unwrap();
        let node = &a.nodes["digits"];
        let recipe = node.bitmap_text.clone().unwrap();
        assert_eq!(recipe.cell, [10, 20]);
        assert_eq!(recipe.line_spacing, 2);
        assert_eq!(recipe.x_anchor, BitmapAnchor::End);
        assert_eq!(
            blocks[0]["ops"][0]["operation"]["value"],
            json!({"type":"var","name":"turn"})
        );
        assert_eq!(blocks[0]["ops"][0]["operation"]["target"], recipe.slot);
        a.variables
            .insert("turn".into(), json!({"type":"i32","value":34}));
        assert_eq!(
            a.nodes["digits"].bitmap_text.as_ref().unwrap().slot,
            recipe.slot
        );
        event(
            &mut a,
            "CGCHARCHG",
            &["digits".into(), "turn".into(), "1".into()],
            &mut blocks,
        )
        .unwrap();
        assert_eq!(blocks[2]["ops"][0]["operation"]["target"], recipe.slot);
        assert!(event(
            &mut a,
            "CGCHARCHG",
            &["digits".into(), "turn+1".into(), "1".into()],
            &mut blocks
        )
        .is_err());
        let mut bad = args.clone();
        bad[10] = "11".into();
        assert!(event(&mut a, "CGCHARNEW", &bad, &mut blocks).is_err());
        // A generated slot must never overwrite a source variable with the same name.
        a.bitmap_slots.clear();
        assert!(event(&mut a, "CGCHARNEW", &args, &mut blocks).is_err());
        a.event(
            &["DELETECG".into(), "digits".into(), "0".into(), "0".into()],
            &mut blocks,
        )
        .unwrap();
        assert!(!a.nodes.contains_key("digits"));
    }
}
