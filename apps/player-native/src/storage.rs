//! Lightweight async atomic JSON local storage for saves, profile, preferences,
//! and a fast `slots_index.json` summary table (avoiding full snapshot parsing on cold start!).

use anyhow::{Context, Result};
use nir_ir::Preferences;
use nir_ui::SlotSummary;
use nir_vm::{ProfileData, SaveBackupBundle, SaveSnapshot};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};

enum WriteOp {
    WriteBytes { path: PathBuf, data: Vec<u8> },
    RemoveFile { path: PathBuf },
    Flush(mpsc::SyncSender<()>),
}

pub struct LocalStorage {
    nir_root: PathBuf,
    dir: PathBuf,
    index: Mutex<BTreeMap<u32, SlotSummary>>,
    thumbs: Mutex<HashMap<String, String>>,
    _lock_file: Option<File>,
    concurrent_instance: bool,
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
        let nir_root = base.join("nir");
        let dir = nir_root.join(safe_id);
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating save directory `{}`", dir.display()))?;

        let (lock_file, concurrent_instance) = try_acquire_instance_lock(&dir);

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
            nir_root,
            dir,
            index: Mutex::new(BTreeMap::new()),
            thumbs: Mutex::new(HashMap::new()),
            _lock_file: lock_file,
            concurrent_instance,
            writer_tx: handle.as_ref().map(|_| tx),
            writer_handle: handle,
        })
    }

    pub fn is_concurrent_instance(&self) -> bool {
        self.concurrent_instance
    }

    pub fn patches_dir(&self) -> PathBuf {
        self.dir.join("patches")
    }

    pub fn screenshots_dir(&self) -> PathBuf {
        #[cfg(not(target_os = "android"))]
        {
            if let Some(pic) = std::env::var_os("XDG_PICTURES_DIR") {
                let p = PathBuf::from(pic);
                if p.is_absolute() {
                    return p.join("NIR");
                }
            }
            if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
            {
                let pics = PathBuf::from(home).join("Pictures");
                if pics.is_dir() {
                    return pics.join("NIR");
                }
            }
        }
        self.dir.join("screenshots")
    }

    /// Loads game-specific preferences, automatically inheriting cross-game `global_prefs.json`
    /// on first launch (matching Web player global user preferences).
    pub fn load_preferences(&self) -> Preferences {
        if let Some(local) = read_json(&self.dir.join("prefs.json")) {
            return local;
        }
        read_json(&self.nir_root.join("global_prefs.json")).unwrap_or_default()
    }

    pub fn save_preferences(&self, prefs: &Preferences) -> Result<()> {
        let data = serde_json::to_vec(prefs)?;
        let _ = self.enqueue_write(self.nir_root.join("global_prefs.json"), data.clone());
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
        let has_thumb = snap.thumbnail.is_some();
        if let Some(t) = &snap.thumbnail {
            if let Ok(mut guard) = self.thumbs.lock() {
                guard.insert(format!("slot_{slot}"), t.clone());
            }
        }
        let summary = SlotSummary {
            slot,
            exists: true,
            saved_at_unix: snap.saved_at_unix,
            summary: snap.summary.clone(),
            has_thumb,
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
        if let Ok(mut guard) = self.thumbs.lock() {
            guard.remove(&format!("slot_{slot}"));
        }
        let index_bytes = {
            let mut map = self.index.lock().unwrap_or_else(|e| e.into_inner());
            map.remove(&slot);
            serde_json::to_vec(&*map)?
        };
        self.enqueue_write(self.index_path(), index_bytes)
    }

    pub fn save_named(&self, name: &str, snap: &SaveSnapshot) -> Result<()> {
        let data = serde_json::to_vec(snap)?;
        if let Some(t) = &snap.thumbnail {
            if let Ok(mut guard) = self.thumbs.lock() {
                guard.insert(name.to_owned(), t.clone());
            }
        }
        self.enqueue_write(self.dir.join(format!("{name}.json")), data)
    }

    pub fn load_named(&self, name: &str) -> Result<SaveSnapshot> {
        self.flush();
        let path = self.dir.join(format!("{name}.json"));
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading named save `{}`", path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn named_summary(&self, name: &str, virtual_slot: u32) -> Option<SlotSummary> {
        let snap = self.load_named(name).ok()?;
        let has_thumb = snap.thumbnail.is_some();
        if let Some(t) = snap.thumbnail {
            if let Ok(mut guard) = self.thumbs.lock() {
                guard.insert(name.to_owned(), t);
            }
        }
        Some(SlotSummary {
            slot: virtual_slot,
            exists: true,
            saved_at_unix: snap.saved_at_unix,
            summary: snap.summary,
            has_thumb,
        })
    }

    /// Lazily retrieves all stored save thumbnails (`(key, data_url)` where key is
    /// `"autosave"`, `"quicksave"`, or `"slot_<n>"`) when the player opens the Saves screen.
    pub fn collect_all_thumbnails(&self, slot_count: u32) -> Vec<(String, String)> {
        self.flush();
        let mut out = Vec::new();
        let mut cache = self.thumbs.lock().unwrap_or_else(|e| e.into_inner());

        for name in ["autosave", "quicksave"] {
            if let Some(url) = cache.get(name) {
                out.push((name.to_owned(), url.clone()));
            } else if let Some(snap) =
                read_json::<SaveSnapshot>(&self.dir.join(format!("{name}.json")))
            {
                if let Some(url) = snap.thumbnail {
                    cache.insert(name.to_owned(), url.clone());
                    out.push((name.to_owned(), url));
                }
            }
        }

        let existing_slots: Vec<u32> = {
            let idx = self.index.lock().unwrap_or_else(|e| e.into_inner());
            (0..slot_count)
                .filter(|s| idx.get(s).is_some_and(|e| e.exists))
                .collect()
        };
        for slot in existing_slots {
            let key = format!("slot_{slot}");
            if let Some(url) = cache.get(&key) {
                out.push((key, url.clone()));
            } else if let Some(snap) = read_json::<SaveSnapshot>(&self.slot_path(slot)) {
                if let Some(url) = snap.thumbnail {
                    cache.insert(key.clone(), url.clone());
                    out.push((key, url));
                }
            }
        }
        out
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
                        let has_thumb = snap.thumbnail.is_some();
                        if let Some(t) = snap.thumbnail {
                            if let Ok(mut guard) = self.thumbs.lock() {
                                guard.insert(format!("slot_{slot}"), t);
                            }
                        }
                        scanned.insert(
                            slot,
                            SlotSummary {
                                slot,
                                exists: true,
                                saved_at_unix: snap.saved_at_unix,
                                summary: snap.summary,
                                has_thumb,
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

    /// Exports all saves (slots + autosave + quicksave), profile, and preferences into a single
    /// portable `nir-backup-v1` JSON file (100% interoperable across Desktop, Android, and Web!).
    pub fn export_backup_json(&self, game_id: &str, slot_count: u32, dest: &Path) -> Result<usize> {
        self.flush();
        let mut saves = BTreeMap::<String, SaveSnapshot>::new();
        for name in ["autosave", "quicksave"] {
            if let Ok(snap) = self.load_named(name) {
                saves.insert(name.to_owned(), snap);
            }
        }
        for slot in 0..slot_count {
            if let Ok(snap) = self.load_slot(slot) {
                saves.insert(format!("slot_{slot:02}"), snap);
            }
        }
        let count = saves.len();
        let bundle = SaveBackupBundle::new(
            game_id,
            Some(self.load_preferences()),
            Some(self.load_profile()),
            saves,
        );
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_vec_pretty(&bundle)?;
        write_bytes_atomic(dest, &data)?;
        Ok(count)
    }

    /// Imports saves/profile/preferences from a portable `nir-backup-v1` JSON file with smart
    /// non-destructive profile union merging and timestamp-aware slot updates (keeps newer local saves
    /// unless the backup is at least as recent).
    pub fn import_backup_json(&self, expected_game_id: &str, src: &Path) -> Result<usize> {
        let text = fs::read_to_string(src)
            .with_context(|| format!("reading save backup `{}`", src.display()))?;
        let bundle: SaveBackupBundle = serde_json::from_str(&text)
            .with_context(|| format!("parsing save backup `{}`", src.display()))?;
        bundle
            .validate_for(expected_game_id)
            .map_err(|e| anyhow::anyhow!(e))?;
        if let Some(prefs) = &bundle.preferences {
            let _ = self.save_preferences(prefs);
        }
        if let Some(incoming_profile) = &bundle.profile {
            let mut merged = self.load_profile();
            merged.merge_from(incoming_profile);
            let _ = self.save_profile(&merged);
        }
        let mut imported = 0usize;
        for (k, snap) in &bundle.saves {
            if k == "autosave" || k == "quicksave" {
                let existing_ts = self
                    .load_named(k)
                    .map(|s| s.saved_at_unix)
                    .unwrap_or(0);
                if snap.saved_at_unix >= existing_ts && self.save_named(k, snap).is_ok() {
                    imported += 1;
                }
            } else if let Some(rest) = k.strip_prefix("slot_") {
                if let Ok(slot) = rest.parse::<u32>() {
                    let existing_ts = self
                        .load_slot(slot)
                        .map(|s| s.saved_at_unix)
                        .unwrap_or(0);
                    if snap.saved_at_unix >= existing_ts && self.save_slot(slot, snap).is_ok() {
                        imported += 1;
                    }
                }
            }
        }
        self.flush();
        Ok(imported)
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
            if tx
                .send(WriteOp::WriteBytes {
                    path: path.clone(),
                    data,
                })
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(anyhow::anyhow!(
            "storage writer closed for {}",
            path.display()
        ))
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

pub(crate) fn default_data_dir() -> PathBuf {
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

/// Acquires a non-blocking advisory lock on `<game_dir>/.lock` to detect concurrent instances
/// playing the same game (matching Web player's Web Locks warning).
fn try_acquire_instance_lock(dir: &Path) -> (Option<File>, bool) {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let lock_path = dir.join(".lock");
        if let Ok(file) = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
        {
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                return (Some(file), false);
            }
            return (None, true);
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    (None, false)
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
            thumbnail: None,
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

    #[test]
    fn storage_inherits_global_prefs_tracks_thumbnails_and_detects_concurrent_lock() {
        let dir = std::env::temp_dir().join(format!(
            "nir-storage-global-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let st_a = LocalStorage::open("org.nir.game_a", Some(dir.clone())).unwrap();
        assert!(!st_a.is_concurrent_instance());
        // Opening the same game while st_a holds the flock reports concurrent instance on Unix.
        let st_a2 = LocalStorage::open("org.nir.game_a", Some(dir.clone())).unwrap();
        #[cfg(unix)]
        assert!(st_a2.is_concurrent_instance());
        drop(st_a2);

        let prefs = Preferences {
            master_volume: 0.42,
            eco_mode: true,
            ..Default::default()
        };
        st_a.save_preferences(&prefs).unwrap();
        st_a.flush();

        // A brand-new game inherits global preferences automatically.
        let st_b = LocalStorage::open("org.nir.game_b", Some(dir.clone())).unwrap();
        let inherited = st_b.load_preferences();
        assert!((inherited.master_volume - 0.42).abs() < 1e-4);
        assert!(inherited.eco_mode);

        // Save with embedded thumbnail sets has_thumb=true and is retrievable via collect_all_thumbnails.
        let mut snap = SaveSnapshot {
            format: nir_vm::SAVE_FORMAT_VERSION,
            game_id: "org.nir.game_b".into(),
            version: "1.0".into(),
            saved_at_unix: 1_700_000_001,
            summary: "Thumb test".into(),
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
            thumbnail: Some("data:image/webp;base64,AAAA".into()),
        };
        st_b.save_slot(1, &snap).unwrap();
        snap.summary = "Auto thumb".into();
        st_b.save_named("autosave", &snap).unwrap();
        st_b.flush();

        let summaries = st_b.list_slot_summaries(4);
        assert!(summaries[1].has_thumb);
        let thumbs = st_b.collect_all_thumbnails(4);
        assert!(thumbs
            .iter()
            .any(|(k, v)| k == "slot_1" && v == "data:image/webp;base64,AAAA"));
        assert!(thumbs
            .iter()
            .any(|(k, v)| k == "autosave" && v == "data:image/webp;base64,AAAA"));

        drop(st_a);
        drop(st_b);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_export_and_import_roundtrips_saves_and_preferences() {
        let dir = std::env::temp_dir().join(format!(
            "nir-storage-backup-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let src_dir = dir.join("src");
        let dst_dir = dir.join("dst");
        let backup_path = dir.join("saves-backup.json");

        let st_src = LocalStorage::open("org.nir.backup_demo", Some(src_dir)).unwrap();
        let prefs = Preferences {
            master_volume: 0.77,
            eco_mode: true,
            ..Default::default()
        };
        st_src.save_preferences(&prefs).unwrap();
        let snap = SaveSnapshot {
            format: nir_vm::SAVE_FORMAT_VERSION,
            game_id: "org.nir.backup_demo".into(),
            version: "1.0".into(),
            saved_at_unix: 1_700_000_050,
            summary: "Portable Save".into(),
            pc: 7,
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
            locale: "zh-Hans".into(),
            rng_word_pos: 0,
            outcome: None,
            thumbnail: None,
        };
        st_src.save_slot(3, &snap).unwrap();
        st_src.save_named("autosave", &snap).unwrap();

        let exported = st_src
            .export_backup_json("org.nir.backup_demo", 8, &backup_path)
            .unwrap();
        assert_eq!(exported, 2);

        let st_dst = LocalStorage::open("org.nir.backup_demo", Some(dst_dir)).unwrap();
        // Reject mismatched game_id backups cleanly.
        assert!(st_dst
            .import_backup_json("org.nir.other_game", &backup_path)
            .is_err());
        let imported = st_dst
            .import_backup_json("org.nir.backup_demo", &backup_path)
            .unwrap();
        assert_eq!(imported, 2);
        assert_eq!(st_dst.load_slot(3).unwrap().summary, "Portable Save");
        assert_eq!(st_dst.load_named("autosave").unwrap().pc, 7);
        assert!(st_dst.load_preferences().eco_mode);

        drop(st_src);
        drop(st_dst);
        let _ = fs::remove_dir_all(&dir);
    }
}
