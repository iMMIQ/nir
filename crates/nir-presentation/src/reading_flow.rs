use super::*;
use crate::history::{HistoryLayout, HistoryStyle};
use std::result::Result;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct HistoryFlowView {
    pub id: String,
    pub order: usize,
    pub rect: [f32; 4],
    pub clip: Option<[f32; 4]>,
    pub color: [f32; 4],
    pub style: HistoryStyle,
    pub wheel_step: f32,
    pub page_step: f32,
    pub max_visible: usize,
    pub paint_index: usize,
    pub enabled: bool,
}
#[derive(Debug)]
pub(super) struct State {
    key: (u32, u32, String),
    rows: Arc<[MenuHistoryRow]>,
    layout: Result<HistoryLayout, String>,
    epoch: u32,
    page_step: f32,
}
fn next(epoch: u32) -> Result<u32, String> {
    epoch
        .checked_add(1)
        .ok_or_else(|| "E_VIEW_LIMIT: history layout identity exhausted".into())
}
impl ReadingState {
    pub fn history_pending(&self) -> bool {
        self.flow_pending
    }
    pub fn history_error(&self) -> Option<&str> {
        self.flow
            .as_ref()
            .and_then(|state| state.layout.as_ref().err().map(String::as_str))
    }
    pub(super) fn project_history_flow(
        &mut self,
        p: &mut DrawPacket,
        m: &UiModel,
        session: u32,
        messages: &Messages,
        text: &mut TextEngine,
    ) {
        self.flow_pending = false;
        if self
            .flow
            .as_ref()
            .is_some_and(|s| s.key.0 != session || s.key.1 != m.menu_instance)
        {
            self.flow = None;
        }
        let (Some(view), Some(rows)) = (p.history_flow.clone(), m.menu_history_flow.as_ref())
        else {
            return;
        };
        let mut rect = view.rect;
        for clip in [Some([0., 0., p.width, p.height]), view.clip]
            .into_iter()
            .flatten()
        {
            let right = (rect[0] + rect[2]).min(clip[0] + clip[2]);
            let bottom = (rect[1] + rect[3]).min(clip[1] + clip[3]);
            rect[0] = rect[0].max(clip[0]);
            rect[1] = rect[1].max(clip[1]);
            rect[2] = (right - rect[0]).max(0.);
            rect[3] = (bottom - rect[1]).max(0.);
        }
        if rect[2] <= 0. || rect[3] <= 0. {
            return;
        }
        let key = (session, m.menu_instance, view.id.clone());
        if self
            .flow
            .as_ref()
            .is_none_or(|s| s.key != key || !Arc::ptr_eq(&s.rows, rows))
        {
            self.flow = Some(State {
                key,
                rows: rows.clone(),
                layout: HistoryLayout::new(rows.clone(), view.style),
                epoch: 1,
                page_step: view.page_step,
            });
        }
        let state = self.flow.as_mut().unwrap();
        state.page_step = view.page_step;
        let result = (|| -> Result<Option<Vec<TextRun>>, String> {
            let layout = state.layout.as_mut().map_err(|error| error.clone())?;
            if layout.reflow(view.style, text)? {
                state.epoch = next(state.epoch)?;
            }
            if !layout.ready() {
                layout.measure_next(text)?;
            }
            if !layout.ready() {
                return Ok(None);
            }
            let runs = layout.visible_runs([view.rect[0], view.rect[1]], Some(rect), view.color)?;
            if runs.len() > view.max_visible {
                return Err("E_HISTORY_LAYOUT: declared visible budget exceeded".into());
            }
            if view.enabled
                && !m.loading
                && layout.max_offset().unwrap_or(0.) > 0.
                && rect[2] > 0.
                && rect[3] > 0.
            {
                p.scrolls.push(ScrollView {
                    menu: Some(MenuScrollIdentity {
                        instance: m.menu_instance,
                        revision: m.menu_revision,
                        window: view.id.clone(),
                        layout: state.epoch,
                    }),
                    region: ScrollRegion::History,
                    rect,
                    offset: layout.offset().unwrap(),
                    max: layout.max_offset().unwrap(),
                    step: view.wheel_step,
                });
            }
            Ok(Some(runs))
        })();
        let start = p.texts.len();
        match result {
            Ok(Some(runs)) => p.texts.extend(runs),
            other => {
                let id = match other {
                    Err(error) => {
                        state.layout = Err(error);
                        "history-unavailable"
                    }
                    _ => {
                        self.flow_pending = true;
                        "loading"
                    }
                };
                p.text(
                    messages.text(&m.ui_locale, id),
                    view.rect[0],
                    view.rect[1],
                    view.rect[2],
                    view.style.size,
                    view.color,
                );
                let run = p.texts.last_mut().unwrap();
                run.height = view.rect[3];
                run.clip = Some(rect);
            }
        }
        p.menu_paint.splice(
            view.paint_index..view.paint_index,
            (start..p.texts.len()).map(MenuPaint::Text),
        );
    }
    pub fn scroll_menu_history(&mut self, action: &UiAction, packet: &DrawPacket) -> bool {
        let UiAction::MenuHistoryScroll {
            control,
            instance,
            revision,
            window,
            layout: epoch,
            input,
        } = action
        else {
            return false;
        };
        let Some(view) = packet.scrolls.iter().find(|view| {
            view.menu.as_ref().is_some_and(|id| {
                id.instance == *instance
                    && id.revision == *revision
                    && id.window == *window
                    && id.layout == *epoch
            })
        }) else {
            return false;
        };
        let bar = if let Some(control) = control {
            let Some(bar) = packet.history_bar.as_ref().filter(|bar| {
                bar.id == *control && bar.enabled && bar.authority.as_ref() == view.menu.as_ref()
            }) else {
                return false;
            };
            Some(bar)
        } else {
            None
        };
        let Some(state) = &mut self.flow else {
            return false;
        };
        if state.key.1 != *instance || state.key.2 != *window || state.epoch != *epoch {
            return false;
        }
        let Ok(layout) = &mut state.layout else {
            return false;
        };
        let offset = match input {
            HistoryScrollInput::Line { delta } if matches!(*delta, -1 | 1) => {
                let Some(bar) = bar else {
                    return false;
                };
                if (*delta < 0 && view.offset <= 0.) || (*delta > 0 && view.offset >= view.max) {
                    return false;
                }
                view.offset + *delta as f32 * bar.line_step
            }

            HistoryScrollInput::Step { delta } if control.is_none() && matches!(*delta, -1 | 1) => {
                view.offset + *delta as f32 * view.step
            }
            HistoryScrollInput::Page { delta } if matches!(*delta, -1 | 1) => {
                view.offset + *delta as f32 * state.page_step
            }
            HistoryScrollInput::Position { ratio }
                if ratio.is_finite() && (0. ..=1.).contains(ratio) =>
            {
                view.max * ratio
            }
            _ => return false,
        };
        let Ok(epoch) = next(state.epoch) else {
            return false;
        };
        if !layout.scroll_to(offset) {
            return false;
        }
        state.epoch = epoch;
        true
    }
}
