//! Case-insensitive layered Virtual File System and automatic CJK/Unicode text decoder.

use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Normalize a virtual path to lowercase forward-slash form without leading `./` or `/`.
pub fn normalize_vfs_path(raw: &str) -> String {
    let replaced = raw.replace('\\', "/");
    let mut parts = Vec::new();
    for seg in replaced.split('/') {
        let s = seg.trim();
        if s.is_empty() || s == "." {
            continue;
        }
        if s == ".." {
            parts.pop();
            continue;
        }
        parts.push(s.to_ascii_lowercase());
    }
    parts.join("/")
}

/// Abstract read-only archive or directory source.
pub trait ArchiveSource: Send + Sync {
    /// Returns all normalized virtual file paths in deterministic order.
    fn list_files(&self) -> Vec<String>;
    /// Reads the raw bytes of a normalized virtual path, or `None` if absent.
    fn read_if_exists(&self, normalized_path: &str) -> Result<Option<Vec<u8>>>;
}

/// Filesystem directory mounted as a case-insensitive archive.
pub struct DirArchive {
    files: BTreeMap<String, PathBuf>,
}

impl DirArchive {
    pub fn open(root: &Path) -> Result<Self> {
        let mut files = BTreeMap::new();
        if root.is_dir() {
            scan_dir(root, root, &mut files)?;
        }
        Ok(Self { files })
    }

    pub fn real_path(&self, raw_path: &str) -> Option<&Path> {
        let key = normalize_vfs_path(raw_path);
        self.files.get(&key).map(PathBuf::as_path)
    }
}

fn scan_dir(root: &Path, dir: &Path, out: &mut BTreeMap<String, PathBuf>) -> Result<()> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading directory `{}`", dir.display()))?
        .filter_map(|e| e.ok().map(|x| x.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            scan_dir(root, &path, out)?;
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let norm = normalize_vfs_path(&rel.to_string_lossy());
                out.insert(norm, path);
            }
        }
    }
    Ok(())
}

impl ArchiveSource for DirArchive {
    fn list_files(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    fn read_if_exists(&self, normalized_path: &str) -> Result<Option<Vec<u8>>> {
        let Some(real) = self.files.get(normalized_path) else {
            return Ok(None);
        };
        let bytes =
            fs::read(real).with_context(|| format!("reading file `{}`", real.display()))?;
        Ok(Some(bytes))
    }
}

/// In-memory virtual archive (useful for extracted XP3/VF packs or tests).
#[derive(Default)]
pub struct MemoryArchive {
    files: BTreeMap<String, Vec<u8>>,
}

impl MemoryArchive {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: impl AsRef<str>, bytes: Vec<u8>) {
        self.files
            .insert(normalize_vfs_path(path.as_ref()), bytes);
    }
}

impl ArchiveSource for MemoryArchive {
    fn list_files(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    fn read_if_exists(&self, normalized_path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.files.get(normalized_path).cloned())
    }
}

/// Layered VFS where later-mounted layers override earlier ones (e.g. `patch.xp3` over `data.xp3`).
#[derive(Default)]
pub struct CompositeVfs {
    layers: Vec<Box<dyn ArchiveSource>>,
}

impl CompositeVfs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mount<A: ArchiveSource + 'static>(&mut self, layer: A) {
        self.layers.push(Box::new(layer));
    }

    pub fn list_files(&self) -> Vec<String> {
        let mut all = BTreeMap::new();
        for layer in &self.layers {
            for p in layer.list_files() {
                all.insert(p, ());
            }
        }
        all.into_keys().collect()
    }

    pub fn exists(&self, raw_path: &str) -> bool {
        let key = normalize_vfs_path(raw_path);
        self.layers
            .iter()
            .rev()
            .any(|l| matches!(l.read_if_exists(&key), Ok(Some(_))))
    }

    pub fn read(&self, raw_path: &str) -> Result<Vec<u8>> {
        let key = normalize_vfs_path(raw_path);
        for layer in self.layers.iter().rev() {
            if let Some(bytes) = layer.read_if_exists(&key)? {
                return Ok(bytes);
            }
        }
        Err(anyhow!("file not found in VFS: `{raw_path}`"))
    }

    /// Resolves a filename across the VFS either by exact normalized path, basename match,
    /// or stem match with one of the candidate extensions (common in KAG3 and LiveMaker).
    pub fn find_by_stem(&self, raw_name: &str, exts: &[&str]) -> Option<String> {
        let norm = normalize_vfs_path(raw_name);
        let all = self.list_files();
        if all.iter().any(|p| p == &norm) {
            return Some(norm);
        }
        for ext in exts {
            let with_ext = format!("{norm}.{ext}");
            if all.iter().any(|p| p == &with_ext) {
                return Some(with_ext);
            }
        }
        // Basename or basename+ext match
        let needle_base = norm.rsplit('/').next().unwrap_or(&norm);
        if let Some(hit) = all
            .iter()
            .find(|p| p.rsplit('/').next() == Some(needle_base))
        {
            return Some(hit.clone());
        }
        for ext in exts {
            let target = format!("{needle_base}.{ext}");
            if let Some(hit) = all.iter().find(|p| p.rsplit('/').next() == Some(target.as_str())) {
                return Some(hit.clone());
            }
        }
        None
    }
}

/// Decodes text files from UTF-8, UTF-16LE/BE (with BOM), Shift-JIS (CP932), or GBK.
/// Returns `(decoded_string, detected_encoding_label)`.
pub fn decode_text_bytes(bytes: &[u8]) -> (String, &'static str) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        if let Ok(s) = std::str::from_utf8(rest) {
            return (s.to_owned(), "utf-8-bom");
        }
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (cow, _, _) = encoding_rs::UTF_16LE.decode(rest);
        return (cow.into_owned(), "utf-16le");
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (cow, _, _) = encoding_rs::UTF_16BE.decode(rest);
        return (cow.into_owned(), "utf-16be");
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_owned(), "utf-8");
    }
    let (sjis, _, sjis_err) = encoding_rs::SHIFT_JIS.decode(bytes);
    if !sjis_err {
        return (sjis.into_owned(), "shift_jis");
    }
    let (gbk, _, gbk_err) = encoding_rs::GBK.decode(bytes);
    if !gbk_err {
        return (gbk.into_owned(), "gbk");
    }
    (sjis.into_owned(), "shift_jis-lossy")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vfs_layering_and_stem_lookup() {
        let mut base = MemoryArchive::new();
        base.insert("Data/BG/Station.PNG", b"base".to_vec());
        let mut patch = MemoryArchive::new();
        patch.insert("data\\bg\\station.png", b"patch".to_vec());

        let mut vfs = CompositeVfs::new();
        vfs.mount(base);
        vfs.mount(patch);

        assert_eq!(vfs.read("DATA/BG/STATION.PNG").unwrap(), b"patch");
        assert_eq!(
            vfs.find_by_stem("station", &["png", "jpg"]).as_deref(),
            Some("data/bg/station.png")
        );
    }

    #[test]
    fn auto_detects_utf16_and_shift_jis() {
        let mut utf16 = vec![0xFF, 0xFE];
        for ch in "こんにちは".encode_utf16() {
            utf16.extend_from_slice(&ch.to_le_bytes());
        }
        let (text, enc) = decode_text_bytes(&utf16);
        assert_eq!(enc, "utf-16le");
        assert_eq!(text, "こんにちは");

        let (sjis_bytes, _, _) = encoding_rs::SHIFT_JIS.encode("雨の駅");
        let (text2, enc2) = decode_text_bytes(&sjis_bytes);
        assert_eq!(enc2, "shift_jis");
        assert_eq!(text2, "雨の駅");
    }
}
