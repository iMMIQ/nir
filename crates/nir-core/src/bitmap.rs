use nir_format::{BitmapAnchor, Node, Value, MAX_NODES};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn tree_ids(nodes: &[Node], root: &str) -> BTreeSet<String> {
    let mut removed = BTreeSet::from([root.to_owned()]);
    loop {
        let before = removed.len();
        for node in nodes {
            if node
                .parent
                .as_ref()
                .is_some_and(|parent| removed.contains(parent))
            {
                removed.insert(node.id.clone());
            }
        }
        if removed.len() == before {
            return removed;
        }
    }
}
pub(crate) fn remove_tree(nodes: &mut Vec<Node>, root: &str) {
    let removed = tree_ids(nodes, root);
    nodes.retain(|node| !removed.contains(&node.id));
}

/// Recipes never enter a snapshot: committed geometry and atlas clips are
/// ordinary scene nodes, independent of later changes to the source variable.
pub(crate) fn materialize(
    nodes: &[Node],
    variables: &BTreeMap<String, Value>,
) -> Result<Vec<Node>, &'static str> {
    let mut out = Vec::new();
    let mut ids: BTreeSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
    for template in nodes {
        let Some(recipe) = &template.bitmap_text else {
            out.push(template.clone());
            continue;
        };
        let text = match variables.get(&recipe.slot) {
            Some(Value::I32(value)) => value.to_string(),
            Some(Value::String(value)) => value.replace("\r\n", "\n"),
            _ => return Err("bitmap text requires a global integer or string capture"),
        };
        if text.chars().count() > 128 {
            return Err("bitmap text exceeds 128 characters");
        }
        let atlas = template.asset.clone().ok_or("bitmap atlas missing")?;
        let alphabet: Vec<_> = recipe.alphabet.chars().collect();
        let lines: Vec<Vec<_>> = text
            .split('\n')
            .map(|line| line.chars().collect())
            .collect();
        let cw = recipe.cell[0] as f32;
        let ch = recipe.cell[1] as f32;
        let width = lines.iter().map(Vec::len).max().unwrap_or(0) as f32 * cw;
        let height = if text.is_empty() {
            0.
        } else {
            lines.len() as f32 * ch + (lines.len() - 1) as f32 * recipe.line_spacing as f32
        };
        if width > 8192. || height > 8192. {
            return Err("bitmap text geometry exceeds stage bounds");
        }
        let anchor = |reference: f32, extent: f32, alignment: BitmapAnchor| match alignment {
            BitmapAnchor::Start => reference,
            BitmapAnchor::Center => ((reference - extent) / 2.).trunc(),
            BitmapAnchor::End => reference - extent,
        };
        let mut group = template.clone();
        group.bitmap_text = None;
        group.asset = None;
        group.color = [0.; 4];
        group.x = anchor(group.x, width, recipe.x_anchor);
        group.y = anchor(group.y, height, recipe.y_anchor);
        group.width = width;
        group.height = height;
        out.push(group);
        let mut ordinal = 0;
        for (line_index, line) in lines.iter().enumerate() {
            let line_width = line.len() as f32 * cw;
            let aligned_x = match recipe.align {
                BitmapAnchor::Start => 0.,
                BitmapAnchor::Center => ((width - line_width) / 2.).trunc(),
                BitmapAnchor::End => width - line_width,
            };
            for (column, character) in line.iter().enumerate() {
                let index = alphabet
                    .iter()
                    .position(|c| c == character)
                    .ok_or("bitmap character is absent from the atlas")?;
                let id = format!("{}/@glyph/{ordinal}", template.id);
                ordinal += 1;
                if !ids.insert(id.clone()) {
                    return Err("bitmap generated node collision");
                }
                let offset = index as f32 * cw;
                out.push(Node {
                    id,
                    parent: Some(template.id.clone()),
                    asset: Some(atlas.clone()),
                    x: aligned_x + column as f32 * cw - offset,
                    y: line_index as f32 * (ch + recipe.line_spacing as f32),
                    width: alphabet.len() as f32 * cw,
                    height: ch,
                    scale: 1.,
                    opacity: 1.,
                    color: template.color,
                    order: column as i32,
                    clip: Some([offset, 0., cw, ch]),
                    timeline_binding: None,
                    inherit_existence: false,
                    sprite_transform: None,
                    bitmap_text: None,
                    preserve_pose: vec![],
                    offset: [0.; 2],
                });
            }
        }
        if out.len() > MAX_NODES {
            return Err("bitmap scene node limit");
        }
    }
    if out.len() > MAX_NODES {
        return Err("bitmap scene node limit");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nir_format::BitmapText;

    fn base_node(id: &str) -> Node {
        Node {
            id: id.into(),
            parent: None,
            asset: Some("atlas.digits".into()),
            x: 100.,
            y: 200.,
            width: 0.,
            height: 0.,
            offset: [0.; 2],
            sprite_transform: None,
            timeline_binding: None,
            inherit_existence: false,
            scale: 1.,
            opacity: 1.,
            color: [1., 1., 1., 1.],
            order: 0,
            clip: None,
            bitmap_text: None,
            preserve_pose: vec![],
        }
    }

    #[test]
    fn tree_ids_and_remove_tree_prune_entire_subtree() {
        let mut root = base_node("root");
        root.asset = None;
        let mut child = base_node("child");
        child.parent = Some("root".into());
        let mut grand = base_node("grand");
        grand.parent = Some("child".into());
        let sibling = base_node("sibling");
        let mut nodes = vec![root, child, grand, sibling];
        assert_eq!(
            tree_ids(&nodes, "root"),
            BTreeSet::from(["root".into(), "child".into(), "grand".into()])
        );
        remove_tree(&mut nodes, "root");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "sibling");
    }

    #[test]
    fn materialize_expands_multiline_aligned_glyphs_and_rejects_errors() {
        let mut tpl = base_node("score");
        tpl.bitmap_text = Some(BitmapText {
            slot: "label".into(),
            alphabet: "0123456789AB".into(),
            cell: [10, 20],
            line_spacing: 4,
            align: BitmapAnchor::Center,
            x_anchor: BitmapAnchor::End,
            y_anchor: BitmapAnchor::Start,
        });
        let vars = BTreeMap::from([("label".into(), Value::String("12\r\n3".into()))]);
        let out = materialize(&[tpl.clone()], &vars).unwrap();
        // 1 container group + 3 glyphs ('1', '2', '3')
        assert_eq!(out.len(), 4);
        assert!(out[0].bitmap_text.is_none());
        assert!(out[0].asset.is_none());
        // width = 20, height = 20*2 + 4 = 44, x_anchor=End => 100 - 20 = 80
        assert_eq!(out[0].x, 80.);
        assert_eq!(out[0].y, 200.);
        assert_eq!(out[0].width, 20.);
        assert_eq!(out[0].height, 44.);
        // First glyph '1' at alphabet index 1 => clip x=10
        assert_eq!(out[1].clip, Some([10., 0., 10., 20.]));
        // Second line '3' centered inside width 20 => aligned_x = (20-10)/2 = 5, offset = 30 => x = -25
        assert_eq!(out[3].x, -25.);
        assert_eq!(out[3].y, 24.);

        // Missing glyph in alphabet
        let bad_char = BTreeMap::from([("label".into(), Value::String("Z".into()))]);
        assert!(materialize(&[tpl.clone()], &bad_char).is_err());

        // Overlong > 128 chars
        let too_long = BTreeMap::from([("label".into(), Value::String("0".repeat(129)))]);
        assert!(materialize(&[tpl.clone()], &too_long).is_err());

        // Wrong variable type (Bool)
        let bad_ty = BTreeMap::from([("label".into(), Value::Bool(true))]);
        assert!(materialize(&[tpl], &bad_ty).is_err());
    }
}
