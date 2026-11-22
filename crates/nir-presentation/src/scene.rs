use super::*;

pub(super) fn scene_layout(
    packet: &DrawPacket,
    nodes: &[Node],
    stage: [f32; 2],
    alpha: f32,
) -> Vec<(String, Quad)> {
    // Each sibling list is ordered separately: a group subtree is contiguous.
    #[derive(Clone, Copy)]
    struct Transform {
        x: f32,
        y: f32,
        scale: f32,
        opacity: f32,
        clip: Option<[f32; 4]>,
    }
    fn intersect(a: Option<[f32; 4]>, b: Option<[f32; 4]>) -> Option<[f32; 4]> {
        match (a, b) {
            (Some(a), Some(b)) => {
                let x = a[0].max(b[0]);
                let y = a[1].max(b[1]);
                Some([
                    x,
                    y,
                    (a[0] + a[2]).min(b[0] + b[2]).max(x) - x,
                    (a[1] + a[3]).min(b[1] + b[3]).max(y) - y,
                ])
            }
            (a, b) => a.or(b),
        }
    }
    fn visit(out: &mut Vec<(String, Quad)>, nodes: &[Node], parent: Option<&str>, t: Transform) {
        let mut siblings: Vec<_> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent.as_deref() == parent)
            .collect();
        siblings.sort_by_key(|(i, n)| (n.order, *i));
        for (_, n) in siblings {
            let x = t.x + (n.x + n.offset[0]) * t.scale;
            let y = t.y + (n.y + n.offset[1]) * t.scale;
            let scale = t.scale * n.scale;
            let opacity = t.opacity * n.opacity;
            let clip = intersect(
                t.clip,
                n.clip.map(|r| {
                    [
                        x + r[0] * scale,
                        y + r[1] * scale,
                        r[2] * scale,
                        r[3] * scale,
                    ]
                }),
            );
            if n.width > 0. && n.height > 0. {
                let mut color = n.color;
                color[3] *= opacity;
                out.push((
                    n.id.clone(),
                    Quad {
                        rect: [x, y, n.width * scale, n.height * scale],
                        corners: n.sprite_transform.map(|transform| {
                            transform
                                .corners(n.width, n.height)
                                .map(|[cx, cy]| [x + cx * scale, y + cy * scale])
                        }),
                        color,
                        asset: n.asset.clone(),
                        clip,
                    },
                ));
            }
            visit(
                out,
                nodes,
                Some(&n.id),
                Transform {
                    x,
                    y,
                    scale,
                    opacity,
                    clip,
                },
            );
        }
    }
    let [x, y, width, height] =
        packet
            .stage_viewport
            .unwrap_or([0., 0., packet.width, packet.height]);
    let scale = (width / stage[0]).min(height / stage[1]);
    let mut out = vec![];
    visit(
        &mut out,
        nodes,
        None,
        Transform {
            x: x + (width - stage[0] * scale) / 2.,
            y: y + (height - stage[1] * scale) / 2.,
            scale,
            opacity: alpha,
            clip: None,
        },
    );
    out
}
pub(super) fn scene(packet: &mut DrawPacket, nodes: &[Node], stage: [f32; 2], alpha: f32) {
    let quads = scene_layout(packet, nodes, stage, alpha);
    packet.quads.extend(quads.into_iter().map(|(_, quad)| quad));
}
