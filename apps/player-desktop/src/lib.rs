//! Native package and persistence contracts, independent of window/GPU setup.
#![forbid(unsafe_code)]
pub mod audio_envelope;
pub mod audio_source;
#[cfg(any(windows, target_os = "linux"))]
pub mod desktop;
#[cfg(any(windows, target_os = "linux"))]
pub mod dialog;
#[cfg(any(windows, target_os = "linux"))]
pub mod io_worker;
#[cfg(any(windows, target_os = "linux"))]
pub mod loader;
use anyhow::{bail, ensure, Context, Result};
use nir_format::{NativeRelease, Preferences, RuntimeExecutable};
use nir_player::SaveEnvelope;
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub struct Bundle {
    pub root: PathBuf,
    pub release: String,
    pub manifest: NativeRelease,
}
pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
impl Bundle {
    pub fn open(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root)?;
        let release = fs::read_to_string(root.join("release.txt"))?
            .trim()
            .to_owned();
        ensure!(valid_digest(&release), "E_NATIVE_RELEASE: invalid digest");
        let bytes = fs::read(root.join(format!("releases/{release}.json")))?;
        nir_content::verify(&bytes, &release)?;
        let manifest: NativeRelease = nir_content::parse(&bytes, "native release")?;
        ensure!(
            manifest.format == 1 && matches!(manifest.profile.as_str(), "release" | "dev"),
            "E_NATIVE_VERSION"
        );
        ensure!(valid_digest(&manifest.player), "E_NATIVE_PLAYER");
        for (hash, object) in &manifest.objects {
            ensure!(valid_digest(hash), "E_OBJECT_REFERENCE");
            ensure!(
                object.path.starts_with(&format!("objects/{hash}."))
                    && !object.path.contains(['\\', ':', '%'])
                    && !object.path.split('/').any(|s| s == "..")
                    && object.path.matches('/').count() == 1,
                "E_OBJECT_REFERENCE"
            );
        }
        ensure!(
            manifest.objects.contains_key(&manifest.program),
            "E_PROGRAM_REFERENCE"
        );
        Ok(Self {
            root,
            release,
            manifest,
        })
    }
    pub fn object(&self, hash: &str) -> Result<Vec<u8>> {
        let descriptor = self
            .manifest
            .objects
            .get(hash)
            .context("E_OBJECT_REFERENCE")?;
        let path = fs::canonicalize(self.root.join(&descriptor.path))?;
        ensure!(
            path.starts_with(&self.root),
            "E_OBJECT_PATH: escaped package"
        );
        ensure!(
            fs::metadata(&path)?.len() == descriptor.bytes,
            "E_OBJECT_SIZE"
        );
        let bytes = fs::read(path)?;
        nir_content::verify(&bytes, hash)?;
        Ok(bytes)
    }
    pub fn executable(&self) -> Result<RuntimeExecutable> {
        let executable: RuntimeExecutable =
            nir_content::parse(&self.object(&self.manifest.program)?, "native program")?;
        ensure!(
            executable.format == 2 && executable.program.game_id == self.manifest.game_id,
            "E_RUNTIME_IDENTITY"
        );
        Ok(executable)
    }
    pub fn verify_all(&self) -> Result<()> {
        for hash in self.manifest.objects.keys() {
            self.object(hash)?;
        }
        self.executable()?;
        Ok(())
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("E_STORAGE_PATH")?;
    fs::create_dir_all(parent)?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = fs::File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path)?;
    Ok(())
}

pub struct Storage {
    pub root: PathBuf,
    release: String,
    _lock: fs::File,
}
impl Storage {
    pub fn open(base: &Path, game: &str, profile: &str, release: &str) -> Result<Self> {
        ensure!(
            valid_digest(release) && matches!(profile, "dev" | "release"),
            "E_STORAGE_IDENTITY"
        );
        let root = base
            .join(nir_content::digest(game.as_bytes()))
            .join(profile);
        fs::create_dir_all(&root)?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("player.lock"))?;
        lock.try_lock()
            .context("E_ALREADY_RUNNING: this game is already open")?;
        Ok(Self {
            root,
            release: release.into(),
            _lock: lock,
        })
    }
    fn slot(&self, slot: u32) -> Result<PathBuf> {
        ensure!(slot < 3, "E_SAVE_SLOT");
        Ok(self
            .root
            .join("releases")
            .join(&self.release)
            .join(format!("slot-{slot}.json")))
    }
    pub fn load(&self, slot: u32) -> Result<Option<SaveEnvelope>> {
        let path = self.slot(slot)?;
        match fs::read(path) {
            Ok(bytes) => {
                let envelope: SaveEnvelope = nir_content::parse(&bytes, "save")?;
                self.validate_save(slot, &envelope)?;
                Ok(Some(envelope))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, slot: u32, expected: u32, envelope: &SaveEnvelope) -> Result<()> {
        self.validate_save(slot, envelope)?;
        let current = self.load(slot)?.map_or(0, |s| s.revision);
        if current != expected {
            bail!("E_SAVE_CONFLICT: save revision changed");
        }
        ensure!(
            envelope.slot == slot && expected.checked_add(1) == Some(envelope.revision),
            "E_SAVE_REVISION"
        );
        atomic_write(&self.slot(slot)?, &serde_json::to_vec(envelope)?)
    }
    fn validate_save(&self, slot: u32, value: &SaveEnvelope) -> Result<()> {
        ensure!(
            value.format == 1 && value.slot == slot && value.snapshot.release == self.release,
            "E_SAVE_IDENTITY"
        );
        nir_content::verify(&serde_json::to_vec(&value.snapshot)?, &value.digest)?;
        Ok(())
    }
    pub fn preferences(&self) -> Result<Option<Preferences>> {
        match fs::read(self.root.join("preferences.json")) {
            Ok(bytes) => Ok(Some(nir_content::parse(&bytes, "preferences")?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn profile(&self) -> Result<BTreeSet<String>> {
        match fs::read(self.root.join("profile.json")) {
            Ok(bytes) => Ok(nir_content::parse(&bytes, "profile")?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn write_preferences(&self, value: &Preferences) -> Result<()> {
        atomic_write(
            &self.root.join("preferences.json"),
            &serde_json::to_vec(value)?,
        )
    }
    pub fn merge_profile(&self, keys: BTreeSet<String>) -> Result<()> {
        let mut all = self.profile()?;
        all.extend(keys);
        atomic_write(&self.root.join("profile.json"), &serde_json::to_vec(&all)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    fn envelope(release: &str, revision: u32) -> SaveEnvelope {
        let program = serde_json::from_str(include_str!("../../../fixtures/rain.json")).unwrap();
        let player = nir_player::Player::new(program, release.into(), "Test".into()).unwrap();
        let snapshot = player.core().snapshot();
        SaveEnvelope {
            format: 1,
            slot: 0,
            revision,
            digest: nir_content::digest(&serde_json::to_vec(&snapshot).unwrap()),
            snapshot,
        }
    }
    #[test]
    fn storage_persists_and_rejects_stale_writes_and_other_releases() {
        let temp = tempfile::tempdir().unwrap();
        let release = "a".repeat(64);
        let store = Storage::open(temp.path(), "game", "release", &release).unwrap();
        assert!(Storage::open(temp.path(), "game", "release", &release).is_err());
        store.save(0, 0, &envelope(&release, 1)).unwrap();
        assert!(store.save(0, 0, &envelope(&release, 1)).is_err());
        assert!(store.save(0, 1, &envelope(&"b".repeat(64), 2)).is_err());
        store.save(0, 1, &envelope(&release, 2)).unwrap();
        drop(store);
        let store = Storage::open(temp.path(), "game", "release", &release).unwrap();
        assert_eq!(store.load(0).unwrap().unwrap().revision, 2);
        drop(store);
        let newer = Storage::open(temp.path(), "game", "release", &"b".repeat(64)).unwrap();
        assert!(newer.load(0).unwrap().is_none());
    }
    #[test]
    fn native_graph_rejects_tampering_and_path_escape() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("objects")).unwrap();
        fs::create_dir(temp.path().join("releases")).unwrap();
        let hash = nir_content::digest(b"content");
        let mut manifest = NativeRelease {
            format: 1,
            game_id: "game".into(),
            title: "title".into(),
            version: "1".into(),
            profile: "release".into(),
            engine_build: "a".repeat(64),
            player: "b".repeat(64),
            program: hash.clone(),
            objects: BTreeMap::from([(
                hash.clone(),
                nir_format::Object {
                    path: format!("objects/{hash}.json"),
                    bytes: 7,
                    media_type: "application/json".into(),
                },
            )]),
        };
        let publish = |manifest: &NativeRelease| {
            let bytes = serde_json::to_vec(manifest).unwrap();
            let id = nir_content::digest(&bytes);
            fs::write(temp.path().join(format!("releases/{id}.json")), bytes).unwrap();
            fs::write(temp.path().join("release.txt"), id).unwrap();
        };
        publish(&manifest);
        fs::write(temp.path().join(&manifest.objects[&hash].path), b"content").unwrap();
        let bundle = Bundle::open(temp.path()).unwrap();
        assert_eq!(bundle.object(&hash).unwrap(), b"content");
        fs::write(temp.path().join(&manifest.objects[&hash].path), b"changed").unwrap();
        assert!(bundle.object(&hash).is_err());
        manifest.objects.get_mut(&hash).unwrap().path = format!("objects/{hash}.json/../../secret");
        publish(&manifest);
        assert!(Bundle::open(temp.path()).is_err());
    }
    #[test]
    fn profile_is_monotonic_and_preferences_survive_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let release = "a".repeat(64);
        let store = Storage::open(temp.path(), "game", "dev", &release).unwrap();
        store.merge_profile(BTreeSet::from(["one".into()])).unwrap();
        store.merge_profile(BTreeSet::from(["two".into()])).unwrap();
        let preferences = Preferences {
            font_scale: 1.2,
            ..Default::default()
        };
        store.write_preferences(&preferences).unwrap();
        drop(store);
        let store = Storage::open(temp.path(), "game", "dev", &release).unwrap();
        assert_eq!(store.profile().unwrap().len(), 2);
        assert_eq!(store.preferences().unwrap().unwrap().font_scale, 1.2);
    }
}
