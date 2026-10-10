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
