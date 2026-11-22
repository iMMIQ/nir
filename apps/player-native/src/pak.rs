//! Zero-copy `.nirpak` container reader (mmap on desktop, direct buffer on Android).

use anyhow::{Context, Result};
use memmap2::Mmap;
use nir_ir::{decode_nirpak_header, nirpak_asset_slice, AssetId, NirProgram};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

enum Backing {
    #[allow(dead_code)]
    Mmap(Mmap),
    Bytes(Vec<u8>),
}

impl Backing {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Mmap(m) => m.as_ref(),
            Self::Bytes(b) => b.as_slice(),
        }
    }
}

/// Thread-safe handle to an opened `game.nirpak` archive.
pub struct PakArchive {
    source_path: Option<PathBuf>,
    last_modified: Option<SystemTime>,
    backing: Backing,
    media_base: usize,
    pub program: Arc<NirProgram>,
}

impl PakArchive {
    /// Opens a `.nirpak` file on disk via read-only memory mapping (zero-copy!).
    pub fn open_file(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("opening game package `{}`", path.display()))?;
        let last_modified = file.metadata().ok().and_then(|m| m.modified().ok());
        // SAFETY: The `.nirpak` file is read-only; if replaced during hot-reload, atomic rename
        // replaces the directory entry while the existing mmap keeps referencing the old inode.
        let mmap = unsafe { Mmap::map(&file) }
            .with_context(|| format!("memory-mapping `{}`", path.display()))?;
        let (program, media_base) = decode_nirpak_header(mmap.as_ref())
            .map_err(|e| anyhow::anyhow!("decoding nirpak header: {e}"))?;
        Ok(Self {
            source_path: Some(path.to_path_buf()),
            last_modified,
            backing: Backing::Mmap(mmap),
            media_base,
            program: Arc::new(program),
        })
    }

    /// Opens a `.nirpak` from an in-memory buffer (e.g. Android AssetManager).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let (program, media_base) = decode_nirpak_header(&bytes)
            .map_err(|e| anyhow::anyhow!("decoding nirpak header: {e}"))?;
        Ok(Self {
            source_path: None,
            last_modified: None,
            backing: Backing::Bytes(bytes),
            media_base,
            program: Arc::new(program),
        })
    }

    /// Zero-copy slice for a single asset inside `.nirpak`.
    pub fn asset_bytes(&self, id: AssetId) -> Option<&[u8]> {
        let meta = self.program.asset(id)?;
        nirpak_asset_slice(self.backing.as_slice(), self.media_base, meta).ok()
    }

    /// Returns `Some(new_archive)` if the backing file on disk has been modified (hot-reload).
    pub fn check_reloaded(&self) -> Option<Arc<Self>> {
        let path = self.source_path.as_ref()?;
        let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
        if Some(mtime) != self.last_modified {
            Self::open_file(path).ok().map(Arc::new)
        } else {
            None
        }
    }
}
