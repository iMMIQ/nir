//! Read-only presentation projection. UI actions do not mutate the story here.
#![forbid(unsafe_code)]
pub use cosmic_text;
use fluent_bundle::{FluentBundle, FluentResource};
use nir_format::*;
use serde::Serialize;
pub mod audio_recovery;
mod control_feedback;
pub mod history;
mod history_controls;
mod input;
mod reading;
mod scrollbar;
pub mod storage_recovery;
pub use control_feedback::{paint_control_feedback, ControlTarget};
pub use input::{
    control_value_action, pointer_action, primary_action, value_action, KeyboardFocus,
};
pub use reading::{MenuScrollIdentity, ReadingState, ScrollView};
pub use scrollbar::{HistoryBar, HistoryBarPart};

mod menu;
mod messages;
mod model;
mod scene;
#[cfg(test)]
mod scene_tests;
mod screens;
#[cfg(test)]
mod story_layout;
mod text_engine;

pub(crate) use menu::divert_menu_page;
use menu::{menu_elements, menu_node, menu_service_enabled};
pub use messages::Messages;
pub use model::*;
use scene::{scene, scene_layout};
pub use screens::project;
use screens::{apply_dialogue_offset, project_measured, LayoutMeasurements, PanelOffsets};
pub use text_engine::{TextCacheStats, TextEngine};
