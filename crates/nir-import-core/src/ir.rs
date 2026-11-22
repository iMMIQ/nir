//! High-level engine-agnostic Visual Novel Intermediate Representation (VN-IR).
//!
//! Every importer lowers its engine's native format into `VnProject`, which is then
//! emitted as a clean, human-editable `.nirs` project (and optionally compiled directly
//! to `.nirpak`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A converted visual novel project ready to emit as `.nirs` + `game.toml` + `assets/`.
#[derive(Clone, Debug)]
pub struct VnProject {
    pub id: String,
    pub title: String,
    pub version: String,
    pub source_locale: String,
    pub stage_width: u32,
    pub stage_height: u32,
    pub entry_script: String,
    pub characters: BTreeMap<String, VnCharacter>,
    pub vars: BTreeMap<String, VnConst>,
    /// Script file stem (e.g. `"main"`, `"ch01"`) -> ordered statements.
    pub scripts: BTreeMap<String, Vec<VnStmt>>,
    /// Relative asset path under `assets/` (e.g. `"bg/station.png"`) -> raw bytes.
    pub assets: BTreeMap<String, Vec<u8>>,
}

impl Default for VnProject {
    fn default() -> Self {
        Self {
            id: "org.nir.imported".into(),
            title: "Imported Game".into(),
            version: "0.1.0".into(),
            source_locale: "ja".into(),
            stage_width: 1280,
            stage_height: 720,
            entry_script: "main".into(),
            characters: BTreeMap::new(),
            vars: BTreeMap::new(),
            scripts: BTreeMap::new(),
            assets: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VnCharacter {
    pub display_name: String,
    pub color: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum VnConst {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VnChoice {
    pub text: String,
    pub target_label: String,
    pub condition: Option<String>,
}

/// High-level VN statement corresponding cleanly to `.nirs` syntax and NIR v2 Persistent Scene Graph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum VnStmt {
    Comment(String),
    Label(String),
    Jump(String),
    Call(String),
    Return,
    SetVar {
        name: String,
        expr: String,
    },
    If {
        cond: String,
        then_body: Vec<VnStmt>,
        else_body: Vec<VnStmt>,
    },
    Bg {
        asset: String,
        transition: Option<String>,
        duration_ms: Option<u32>,
    },
    Show {
        slot: String,
        asset: String,
        x: Option<f32>,
        y: Option<f32>,
        z: Option<i32>,
        opacity: Option<f32>,
        scale: Option<f32>,
    },
    Hide {
        slot: String,
    },
    Clear,
    Commit {
        transition: Option<String>,
        duration_ms: Option<u32>,
    },
    Tween {
        slot: String,
        prop: String,
        to: f32,
        duration_ms: u32,
        ease: Option<String>,
    },
    Shake {
        target: String,
        amp: f32,
        duration_ms: u32,
    },
    Say {
        speaker: Option<String>,
        text: String,
        voice: Option<String>,
    },
    Choice(Vec<VnChoice>),
    Bgm {
        asset: String,
        fade_ms: Option<u32>,
        volume: Option<f32>,
    },
    StopBgm {
        fade_ms: Option<u32>,
    },
    Sfx {
        asset: String,
        volume: Option<f32>,
    },
    Voice {
        asset: String,
    },
    Wait {
        ms: u32,
    },
    Window {
        visible: bool,
    },
    Video {
        asset: String,
    },
    Unlock {
        kind: String,
        id: String,
    },
    End {
        outcome: String,
    },
}
