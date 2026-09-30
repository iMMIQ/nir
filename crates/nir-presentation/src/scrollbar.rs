use super::*;

#[derive(Debug, Clone)]
pub(crate) struct BarView {
    pub element: MenuElement,
    pub quad: Quad,
    pub enabled: bool,
    pub order: usize,
    pub paint_index: usize,
    pub semantic_index: usize,
    pub base_id: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryBarPart {
    Track,
    Thumb,
    Decrease,
    Increase,
}
#[derive(Debug, Clone, Serialize)]
pub struct HistoryBar {
    pub id: String,
    pub authority: Option<MenuScrollIdentity>,
    pub enabled: bool,
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub track: [f32; 4],
    pub thumb: [f32; 4],
    pub decrease: [f32; 4],
    pub increase: [f32; 4],
    pub offset: f32,
    pub max: f32,
    pub line_step: f32,
}
fn contains(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && y >= rect[1] && x <= rect[0] + rect[2] && y <= rect[1] + rect[3]
}
fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    [
        x,
        y,
        ((a[0] + a[2]).min(b[0] + b[2]) - x).max(0.),
        ((a[1] + a[3]).min(b[1] + b[3]) - y).max(0.),
    ]
}
impl HistoryBar {
    pub fn part_at(&self, x: f32, y: f32) -> Option<HistoryBarPart> {
        if !x.is_finite()
            || !y.is_finite()
            || !contains(self.clip, x, y)
            || !contains(self.rect, x, y)
        {
            return None;
        }
        if contains(self.decrease, x, y) {
            Some(HistoryBarPart::Decrease)
        } else if contains(self.increase, x, y) {
            Some(HistoryBarPart::Increase)
        } else if contains(self.thumb, x, y) {
            Some(HistoryBarPart::Thumb)
        } else {
            Some(HistoryBarPart::Track)
        }
    }
    fn part_enabled(&self, part: HistoryBarPart) -> bool {
        self.enabled
            && match part {
                HistoryBarPart::Decrease => self.offset > 0.,
                HistoryBarPart::Increase => self.offset < self.max,
                _ => true,
            }
    }
    pub fn action(&self, input: HistoryScrollInput) -> Option<UiAction> {
        if !self.enabled {
            return None;
        }
        let id = self.authority.as_ref()?;
        Some(UiAction::MenuHistoryScroll {
            control: Some(self.id.clone()),
            instance: id.instance,
            revision: id.revision,
            window: id.window.clone(),
            layout: id.layout,
            input,
        })
    }
}
#[derive(Debug)]
pub(crate) struct BarGesture {
    authority: MenuScrollIdentity,
    control: String,
    identity: (u32, u32),
    rect: [f32; 4],
    clip: [f32; 4],
    track: [f32; 4],
    grab: f32,
    dragging: bool,
}
impl BarGesture {
    fn matches(&self, bar: &HistoryBar) -> bool {
        bar.enabled
            && bar.authority.as_ref() == Some(&self.authority)
            && self.control == bar.id
            && self.rect == bar.rect
            && self.clip == bar.clip
            && self.track == bar.track
    }
}
impl ReadingState {
    pub fn hover_history_bar(&mut self, x: f32, y: f32) -> bool {
        let point = (x.is_finite() && y.is_finite() && x >= 0. && y >= 0.).then_some([x, y]);
        let changed = self.bar_pointer != point;
        self.bar_pointer = point;
        changed
    }
    pub(crate) fn project_history_bar(
        &mut self,
        p: &mut DrawPacket,
        m: &UiModel,
        flow_paints: usize,
    ) {
        let Some(view) = p.history_bar_view.clone() else {
            self.bar_gesture = None;
            self.bar_pressed = None;
            return;
        };
        let MenuContent::HistoryScrollbar {
            window,
            label,
            thumb_height,
            arrow_height,
            line_step,
            track,
            thumb,
            decrease,
            increase,
        } = &view.element.content
        else {
            return;
        };
        let scroll = p
            .scrolls
            .iter()
            .find(|v| v.menu.as_ref().is_some_and(|id| id.window == *window));
        let scale = view.quad.rect[2] / view.element.rect[2];
        let [x, y, w, h] = view.quad.rect;
        let ah = arrow_height * scale;
        let th = thumb_height * scale;
        let offset = scroll.map_or(0., |v| v.offset);
        let max = scroll.map_or(0., |v| v.max);
        let ratio = if max > 0. { offset / max } else { 0. };
        let mut clip = intersect(view.quad.rect, [0., 0., p.width, p.height]);
        if let Some(parent) = view.quad.clip {
            clip = intersect(clip, parent);
        }
        let bar = HistoryBar {
            id: view.element.id.clone(),
            authority: scroll.and_then(|v| v.menu.clone()),
            enabled: view.enabled && scroll.is_some() && !m.loading && clip[2] > 0. && clip[3] > 0.,
            rect: view.quad.rect,
            clip,
            track: [x, y + ah, w, h - 2. * ah],
            thumb: [x, y + ah + ratio * (h - 2. * ah - th), w, th],
            decrease: [x, y, w, ah],
            increase: [x, y + h - ah, w, ah],
            offset,
            max,
            line_step: line_step * scale * m.prefs.font_scale,
        };
        if self.bar_gesture.as_ref().is_some_and(|g| !g.matches(&bar)) {
            self.bar_gesture = None;
            self.bar_pressed = None;
        }
        let hover = self.bar_pointer.and_then(|[x, y]| bar.part_at(x, y));
        let pressed = self.bar_pressed.as_ref().and_then(|(id, control, part)| {
            (Some(id) == bar.authority.as_ref() && control == &bar.id).then_some(*part)
        });
        let parts = [
            (HistoryBarPart::Track, bar.track, track),
            (HistoryBarPart::Thumb, bar.thumb, thumb),
            (HistoryBarPart::Decrease, bar.decrease, decrease),
            (HistoryBarPart::Increase, bar.increase, increase),
        ];
        let quads: Vec<_> = parts
            .iter()
            .map(|(part, rect, images)| {
                let enabled = bar.part_enabled(*part);
                let asset = if !enabled {
                    images.disabled_asset.as_ref()
                } else if pressed == Some(*part) {
                    images
                        .pressed_asset
                        .as_ref()
                        .or(images.hover_asset.as_ref())
                } else if hover == Some(*part) {
                    images.hover_asset.as_ref()
                } else {
                    None
                };
                let tint = if !enabled && images.disabled_asset.is_none() {
                    0.35
                } else {
                    1.
                };
                Quad {
                    rect: *rect,
                    asset: Some(asset.unwrap_or(&images.asset).clone()),
                    color: [tint, tint, tint, view.quad.color[3]],
                    clip: Some(clip),
                }
            })
            .collect();
        let Some((start, end)) = p.menu_quad_range else {
            return;
        };
        for paint in &mut p.menu_paint {
            if let MenuPaint::Quad(i) = paint {
                if *i >= end {
                    *i += 4;
                }
            }
        }
        if let Some(i) = &mut p.dialogue_hint_quad {
            if *i >= end {
                *i += 4;
            }
        }
        p.quads.splice(end..end, quads);
        p.menu_quad_range = Some((start, end + 4));
        // The spliced bar quads belong to the authored page; a later page-root
        // divert must absorb them.
        if let Some((from, to)) = p.menu_page_range {
            p.menu_page_range = Some((from, to + 4));
        }
        let added = if p
            .history_flow
            .as_ref()
            .is_some_and(|f| f.order < view.order)
        {
            flow_paints
        } else {
            0
        };
        let paint_at = view.paint_index + added;
        p.menu_paint
            .splice(paint_at..paint_at, (end..end + 4).map(MenuPaint::Quad));
        let id = bar.authority.clone().unwrap_or(MenuScrollIdentity {
            instance: m.menu_instance,
            revision: m.menu_revision,
            window: window.clone(),
            layout: 0,
        });
        let action = |input| UiAction::MenuHistoryScroll {
            control: Some(bar.id.clone()),
            instance: id.instance,
            revision: id.revision,
            window: id.window.clone(),
            layout: id.layout,
            input,
        };
        let mut nodes = vec![];
        for (index, part, rect, name, input) in [
            (
                0,
                HistoryBarPart::Track,
                bar.track,
                label.clone(),
                HistoryScrollInput::Position { ratio },
            ),
            (
                1,
                HistoryBarPart::Decrease,
                bar.decrease,
                format!("{label} −"),
                HistoryScrollInput::Line { delta: -1 },
            ),
            (
                2,
                HistoryBarPart::Increase,
                bar.increase,
                format!("{label} +"),
                HistoryScrollInput::Line { delta: 1 },
            ),
        ] {
            let rect = intersect(rect, clip);
            if rect[2] <= 0. || rect[3] <= 0. {
                continue;
            }
            nodes.push(SemanticNode {
                id: view.base_id + index,
                label: name,
                action: action(input),
                enabled: bar.part_enabled(part),
                rect,
                locale: m.ui_locale.clone(),
                value: (part == HistoryBarPart::Track).then_some(SemanticValue::Scrollbar {
                    value: offset,
                    max,
                    step: bar.line_step,
                    from: bar.track[1] + th / 2.,
                    to: bar.track[1] + bar.track[3] - th / 2.,
                    thumb_top: bar.thumb[1],
                    thumb_bottom: bar.thumb[1] + th,
                }),
            });
        }
        p.semantics
            .splice(view.semantic_index..view.semantic_index, nodes);
        p.history_bar = Some(bar);
    }
    /// Shared pointer capture: 0 down, 1 move, 2 up, 3 cancel. Accepted gestures
    /// never fall through to Story, including after their page becomes stale.
    pub fn history_bar_gesture(
        &mut self,
        phase: u8,
        x: f32,
        y: f32,
        button: u8,
        identity: (u32, u32),
        packet: &DrawPacket,
    ) -> bool {
        if phase == 3 {
            let had = self.bar_gesture.take().is_some();
            self.bar_pressed = None;
            return had;
        }
        if button != 0 || phase > 2 || !x.is_finite() || !y.is_finite() {
            return false;
        }
        self.hover_history_bar(x, y);
        if phase == 0 {
            self.bar_gesture = None;
            self.bar_pressed = None;
            let Some(bar) = &packet.history_bar else {
                return false;
            };
            let Some(part) = bar.part_at(x, y).filter(|part| bar.part_enabled(*part)) else {
                return false;
            };
            // Obey the same paint order and disabled hit barriers as clicks.
            if !packet.hit_node(x, y).is_some_and(|node| {
                node.enabled
                    && matches!(&node.action,
                UiAction::MenuHistoryScroll { control:Some(control), .. } if control == &bar.id)
            }) {
                return false;
            }
            let mut authority = bar.authority.clone().unwrap();
            let input = match part {
                HistoryBarPart::Decrease => Some(HistoryScrollInput::Line { delta: -1 }),
                HistoryBarPart::Increase => Some(HistoryScrollInput::Line { delta: 1 }),
                HistoryBarPart::Track => Some(HistoryScrollInput::Page {
                    delta: if y < bar.thumb[1] { -1 } else { 1 },
                }),
                HistoryBarPart::Thumb => None,
            };
            if let Some(input) = input {
                if !self.scroll_menu_history(&bar.action(input).unwrap(), packet) {
                    return false;
                }
                authority.layout += 1; // service checked overflow before accepting
            }
            self.bar_pressed = Some((authority.clone(), bar.id.clone(), part));
            self.bar_gesture = Some(BarGesture {
                authority,
                control: bar.id.clone(),
                identity,
                rect: bar.rect,
                clip: bar.clip,
                track: bar.track,
                grab: y - (bar.thumb[1] + bar.thumb[3] / 2.),
                dragging: part == HistoryBarPart::Thumb,
            });
            return true;
        }
        let Some(mut gesture) = self.bar_gesture.take() else {
            return false;
        };
        let Some(bar) = packet
            .history_bar
            .as_ref()
            .filter(|bar| gesture.identity == identity && gesture.matches(bar))
        else {
            self.bar_pressed = None;
            return true;
        };
        if gesture.dragging {
            let from = bar.track[1] + bar.thumb[3] / 2.;
            let travel = bar.track[3] - bar.thumb[3];
            let ratio = ((y - gesture.grab - from) / travel).clamp(0., 1.);
            if ratio * bar.max != bar.offset {
                if !self.scroll_menu_history(
                    &bar.action(HistoryScrollInput::Position { ratio }).unwrap(),
                    packet,
                ) {
                    self.bar_pressed = None;
                    return true;
                }
                gesture.authority.layout += 1;
                self.bar_pressed = Some((
                    gesture.authority.clone(),
                    bar.id.clone(),
                    HistoryBarPart::Thumb,
                ));
            }
        }
        if phase == 2 {
            self.bar_pressed = None;
        } else {
            self.bar_gesture = Some(gesture);
        }
        true
    }
}
