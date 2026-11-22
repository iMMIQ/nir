//! Lightweight atomic JSON local storage for saves, profile, and user preferences.

use anyhow::{Context, Result};
use nir_ir::Preferences;
use nir_ui::SlotSummary;
use nir_vm::{ProfileData, SaveSnapshot};
use std::fs;
use std::path::{Path, PathBuf};

pub struct LocalStorage {
    dir: PathBuf,
}

impl LocalStorage {
    pub fn open(game_id: &str, base_override: Option<PathBuf>) -> Result<Self> {
        let safe_id: String = game_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let base = base_override.unwrap_or_else(default_data_dir);
        let dir = base.join("nir").join(safe_id);
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating save directory `{}`", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn load_preferences(&self) -> Preferences {
        read_json(&self.dir.join("prefs.json")).unwrap_or_default()
    }

    pub fn save_preferences(&self, prefs: &Preferences) -> Result<()> {
        write_json_atomic(&self.dir.join("prefs.json"), prefs)
    }

    pub fn load_profile(&self) -> ProfileData {
        read_json(&self.dir.join("profile.json")).unwrap_or_default()
    }

    pub fn save_profile(&self, profile: &ProfileData) -> Result<()> {
        write_json_atomic(&self.dir.join("profile.json"), profile)
    }

    pub fn save_slot(&self, slot: u32, snap: &SaveSnapshot) -> Result<()> {
        write_json_atomic(&self.slot_path(slot), snap)
    }

    pub fn load_slot(&self, slot: u32) -> Result<SaveSnapshot> {
        let path = self.slot_path(slot);
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading save slot `{}`", path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn delete_slot(&self, slot: u32) -> Result<()> {
        let path = self.slot_path(slot);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    pub fn list_slot_summaries(&self, slot_count: u32) -> Vec<SlotSummary> {
        let mut out = Vec::with_capacity(slot_count as usize);
        for slot in 0..slot_count {
            if let Ok(snap) = self.load_slot(slot) {
                out.push(SlotSummary {
                    slot,
                    exists: true,
                    saved_at_unix: snap.saved_at_unix,
                    summary: snap.summary.clone(),
                });
            } else {
                out.push(SlotSummary {
                    slot,
                    ..Default::default()
                });
            }
        }
        out
    }

    fn slot_path(&self, slot: u32) -> PathBuf {
        self.dir.join(format!("slot_{slot:02}.json"))
    }
}

fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    if let Some(p) = std::env::var_os("APPDATA") {
        return PathBuf::from(p);
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Application Support");
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share");
    }
    PathBuf::from(".")
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_json_atomic<T: serde::Serialize>(path: &Path, val: &T) -> Result<()> {
    let json = serde_json::to_vec_pretty(val)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}
