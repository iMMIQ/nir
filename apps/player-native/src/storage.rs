//! Lightweight async atomic JSON local storage for saves, profile, preferences,
//! and a fast `slots_index.json` summary table (avoiding full snapshot parsing on cold start!).

use anyhow::{Context, Result};
use nir_ir::Preferences;
use nir_ui::SlotSummary;
use nir_vm::{ProfileData, SaveSnapshot};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};

enum WriteOp {
    WriteBytes { path: PathBuf, data: Vec<u8> },
    RemoveFile { path: PathBuf },
    Flush(mpsc::SyncSender<()>),
}

pub struct LocalStorage {
    dir: PathBuf,
    index: Mutex<BTreeMap<u32, SlotSummary>>,
    writer_tx: Option<mpsc::Sender<WriteOp>>,
    writer_handle: Option<std::thread::JoinHandle<()>>,
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

        let (tx, rx) = mpsc::channel::<WriteOp>();
        let handle = std::thread::Builder::new()
            .name("nir-storage-io".into())
            .spawn(move || {
                while let Ok(op) = rx.recv() {
                    match op {
                        WriteOp::WriteBytes { path, data } => {
                            let _ = write_bytes_atomic(&path, &data);
                        }
                        WriteOp::RemoveFile { path } => {
                            if path.exists() {
                                let _ = fs::remove_file(&path);
                            }
                        }
                        WriteOp::Flush(ack) => {
                            let _ = ack.send(());
                        }
                    }
                }
            })
            .ok();

        Ok(Self {
            dir,
            index: Mutex::new(BTreeMap::new()),
            writer_tx: handle.as_ref().map(|_| tx),
            writer_handle: handle,
        })
    }

    pub fn load_preferences(&self) -> Preferences {
        read_json(&self.dir.join("prefs.json")).unwrap_or_default()
    }

    pub fn save_preferences(&self, prefs: &Preferences) -> Result<()> {
        let data = serde_json::to_vec(prefs)?;
        self.enqueue_write(self.dir.join("prefs.json"), data)
    }

    pub fn load_profile(&self) -> ProfileData {
        read_json(&self.dir.join("profile.json")).unwrap_or_default()
    }

    pub fn save_profile(&self, profile: &ProfileData) -> Result<()> {
        let data = serde_json::to_vec(profile)?;
        self.enqueue_write(self.dir.join("profile.json"), data)
    }

    pub fn save_slot(&self, slot: u32, snap: &SaveSnapshot) -> Result<()> {
        let data = serde_json::to_vec(snap)?;
        self.enqueue_write(self.slot_path(slot), data)?;
        let summary = SlotSummary {
            slot,
            exists: true,
            saved_at_unix: snap.saved_at_unix,
            summary: snap.summary.clone(),
        };
        let index_bytes = {
            let mut map = self.index.lock().unwrap_or_else(|e| e.into_inner());
            map.insert(slot, summary);
            serde_json::to_vec(&*map)?
        };
        self.enqueue_write(self.index_path(), index_bytes)
    }

    pub fn load_slot(&self, slot: u32) -> Result<SaveSnapshot> {
        // Ensure any in-flight write to this slot has completed before reading back!
        self.flush();
        let path = self.slot_path(slot);
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading save slot `{}`", path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn delete_slot(&self, slot: u32) -> Result<()> {
        self.enqueue_remove(self.slot_path(slot))?;
        let index_bytes = {
            let mut map = self.index.lock().unwrap_or_else(|e| e.into_inner());
            map.remove(&slot);
            serde_json::to_vec(&*map)?
        };
        self.enqueue_write(self.index_path(), index_bytes)
    }

    /// Reads lightweight `slots_index.json` in a single I/O operation (O(1) cold start!).
    /// Falls back once to scanning existing `slot_XX.json` files if index is absent.
    pub fn list_slot_summaries(&self, slot_count: u32) -> Vec<SlotSummary> {
        let idx_path = self.index_path();
        let indexed: BTreeMap<u32, SlotSummary> = if let Some(map) = read_json(&idx_path) {
            map
        } else {
            let mut scanned = BTreeMap::new();
            for slot in 0..slot_count {
                let path = self.slot_path(slot);
                if path.is_file() {
                    if let Ok(snap) = self.load_slot(slot) {
                        scanned.insert(
                            slot,
                            SlotSummary {
                                slot,
                                exists: true,
                                saved_at_unix: snap.saved_at_unix,
                                summary: snap.summary,
                            },
                        );
                    }
                }
            }
            if !scanned.is_empty() {
                if let Ok(bytes) = serde_json::to_vec(&scanned) {
                    let _ = self.enqueue_write(idx_path, bytes);
                }
            }
            scanned
        };

        {
            let mut guard = self.index.lock().unwrap_or_else(|e| e.into_inner());
            *guard = indexed.clone();
        }

        let mut out = Vec::with_capacity(slot_count as usize);
        for slot in 0..slot_count {
            if let Some(summary) = indexed.get(&slot) {
                out.push(summary.clone());
            } else {
                out.push(SlotSummary {
                    slot,
                    ..Default::default()
                });
            }
        }
        out
    }

    /// Blocks until all queued background writes have safely completed on disk.
    pub fn flush(&self) {
        if let Some(tx) = &self.writer_tx {
            let (ack_tx, ack_rx) = mpsc::sync_channel(1);
            if tx.send(WriteOp::Flush(ack_tx)).is_ok() {
                let _ = ack_rx.recv();
            }
        }
    }

    fn enqueue_write(&self, path: PathBuf, data: Vec<u8>) -> Result<()> {
        if let Some(tx) = &self.writer_tx {
            if tx.send(WriteOp::WriteBytes { path: path.clone(), data }).is_ok() {
                return Ok(());
            }
        }
        Err(anyhow::anyhow!("storage writer closed for {}", path.display()))
    }

    fn enqueue_remove(&self, path: PathBuf) -> Result<()> {
        if let Some(tx) = &self.writer_tx {
            if tx.send(WriteOp::RemoveFile { path }).is_ok() {
                return Ok(());
            }
        }
        Ok(())
    }

    fn slot_path(&self, slot: u32) -> PathBuf {
        self.dir.join(format!("slot_{slot:02}.json"))
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("slots_index.json")
    }
}

impl Drop for LocalStorage {
    fn drop(&mut self) {
        self.flush();
        self.writer_tx = None;
        if let Some(handle) = self.writer_handle.take() {
            let _ = handle.join();
        }
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

fn write_bytes_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn async_storage_maintains_fast_slot_index() {
        let dir = std::env::temp_dir().join(format!(
            "nir-storage-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let st = LocalStorage::open("org.nir.storetest", Some(dir.clone())).unwrap();
        let snap = SaveSnapshot {
            format: nir_vm::SAVE_FORMAT_VERSION,
            game_id: "org.nir.storetest".into(),
            version: "1.0".into(),
            saved_at_unix: 1_700_000_000,
            summary: "Hello index".into(),
            pc: 0,
            last_label: None,
            vars: BTreeMap::new(),
            frames: vec![],
            layers: vec![],
            dialogue_visible: true,
            dialogue: None,
            dialogue_portrait: None,
            dialogue_style: 0,
            choice: None,
            wait: nir_vm::WaitReason::None,
            audio: vec![],
            audio_paused: [false; 3],
            timelines: vec![],
            history: vec![],
            locale: "en".into(),
            rng_word_pos: 0,
            outcome: None,
        };
        st.save_slot(2, &snap).unwrap();
        st.flush();
        assert!(st.index_path().is_file());

        // Re-open and verify slot summaries load from `slots_index.json`
        let st2 = LocalStorage::open("org.nir.storetest", Some(dir.clone())).unwrap();
        let summaries = st2.list_slot_summaries(4);
        assert!(!summaries[0].exists);
        assert!(summaries[2].exists);
        assert_eq!(summaries[2].summary, "Hello index");
        assert_eq!(st2.load_slot(2).unwrap().summary, "Hello index");

        st2.delete_slot(2).unwrap();
        st2.flush();
        let st3 = LocalStorage::open("org.nir.storetest", Some(dir.clone())).unwrap();
        assert!(!st3.list_slot_summaries(4)[2].exists);
        let _ = fs::remove_dir_all(&dir);
    }
}
