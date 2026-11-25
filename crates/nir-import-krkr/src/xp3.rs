//! Pure-Rust KiriKiri2 / KAG3 XP3 archive reader (`ArchiveSource`).
//!
//! Supports standard XP3 archives (v1 and v2/0x80 header flag, both raw and
//! zlib-compressed index tables and file segments, including XP3 archives
//! concatenated onto a Windows `.exe` launcher stub).

use anyhow::{bail, ensure, Context, Result};
use flate2::read::ZlibDecoder;
use nir_import_core::vfs::{normalize_vfs_path, ArchiveSource};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Standard 11-byte XP3 signature: `XP3\r\n \n\x1a\x8b\x67\x01`.
pub const XP3_MAGIC: &[u8; 11] = b"XP3\r\n \n\x1a\x8bg\x01";

#[derive(Clone, Debug)]
struct Xp3Segment {
    compressed: bool,
    offset: u64,
    orig_size: u64,
    packed_size: u64,
}

#[derive(Clone, Debug)]
struct Xp3Entry {
    protected: bool,
    segments: Vec<Xp3Segment>,
}

/// Read-only handle to a `.xp3` archive (or `.exe` with appended XP3 archive).
pub struct Xp3Archive {
    path: PathBuf,
    base_offset: u64,
    entries: BTreeMap<String, Xp3Entry>,
    pub protected_count: usize,
}

impl Xp3Archive {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)
            .with_context(|| format!("opening XP3 archive `{}`", path.display()))?;
        let file_len = file.metadata()?.len();
        let base_offset = find_xp3_magic(&mut file, file_len)?;

        file.seek(SeekFrom::Start(base_offset + XP3_MAGIC.len() as u64))?;
        let mut first_u64 = read_u64_le(&mut file)?;

        // XP3 v2 header check: if `first_u64 == 0x17`, followed by minor(u32), flag(u8),
        // index_size(u64), index_offset(u64).
        let index_offset = if first_u64 == 0x17 {
            let _minor = read_u32_le(&mut file)?;
            let flag = read_u8(&mut file)?;
            if (flag & 0x80) != 0 {
                let _table_size = read_u64_le(&mut file)?;
                first_u64 = read_u64_le(&mut file)?;
            } else {
                first_u64 = read_u64_le(&mut file)?;
            }
            first_u64
        } else {
            first_u64
        };

        let abs_index = base_offset
            .checked_add(index_offset)
            .context("XP3 index offset overflow")?;
        ensure!(abs_index < file_len, "XP3 index offset outside file");
        file.seek(SeekFrom::Start(abs_index))?;

        let index_flags = read_u8(&mut file)?;
        let index_bytes = if (index_flags & 1) != 0 {
            let packed_size = read_u64_le(&mut file)? as usize;
            let orig_size = read_u64_le(&mut file)? as usize;
            ensure!(orig_size <= 128 * 1024 * 1024, "XP3 index exceeds 128 MiB");
            let mut packed = vec![0u8; packed_size];
            file.read_exact(&mut packed)?;
            let mut dec = ZlibDecoder::new(&packed[..]);
            let mut out = Vec::with_capacity(orig_size);
            dec.read_to_end(&mut out)
                .context("decompressing XP3 zlib index")?;
            out
        } else {
            let size = read_u64_le(&mut file)? as usize;
            ensure!(size <= 128 * 1024 * 1024, "XP3 index exceeds 128 MiB");
            let mut out = vec![0u8; size];
            file.read_exact(&mut out)?;
            out
        };

        let mut entries = BTreeMap::new();
        let mut protected_count = 0usize;
        parse_index(&index_bytes, &mut entries, &mut protected_count)?;

        Ok(Self {
            path: path.to_path_buf(),
            base_offset,
            entries,
            protected_count,
        })
    }
}

impl ArchiveSource for Xp3Archive {
    fn list_files(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    fn read_if_exists(&self, normalized_path: &str) -> Result<Option<Vec<u8>>> {
        let Some(entry) = self.entries.get(normalized_path) else {
            return Ok(None);
        };
        if entry.protected {
            bail!(
                "XP3 entry `{normalized_path}` uses custom DRM/encryption (not supported by standard XP3 reader)"
            );
        }
        let mut file = File::open(&self.path)?;
        let mut out = Vec::new();
        for seg in &entry.segments {
            let abs = self
                .base_offset
                .checked_add(seg.offset)
                .context("XP3 segment offset overflow")?;
            file.seek(SeekFrom::Start(abs))?;
            if seg.compressed {
                ensure!(
                    seg.packed_size <= 256 * 1024 * 1024,
                    "XP3 compressed segment too large"
                );
                let mut packed = vec![0u8; seg.packed_size as usize];
                file.read_exact(&mut packed)?;
                let mut dec = ZlibDecoder::new(&packed[..]);
                let mut plain = Vec::with_capacity(seg.orig_size as usize);
                dec.read_to_end(&mut plain)
                    .with_context(|| format!("decompressing XP3 entry `{normalized_path}`"))?;
                out.extend_from_slice(&plain);
            } else {
                ensure!(seg.orig_size <= 256 * 1024 * 1024, "XP3 segment too large");
                let mut plain = vec![0u8; seg.orig_size as usize];
                file.read_exact(&mut plain)?;
                out.extend_from_slice(&plain);
            }
        }
        Ok(Some(out))
    }
}

fn find_xp3_magic(file: &mut File, file_len: u64) -> Result<u64> {
    // Fast path: magic at offset 0
    if file_len >= XP3_MAGIC.len() as u64 {
        let mut head = [0u8; 11];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut head)?;
        if &head == XP3_MAGIC {
            return Ok(0);
        }
    }
    // Scan first 16 MiB (covers self-extracting .exe launchers with appended XP3)
    let scan_limit = file_len.min(16 * 1024 * 1024) as usize;
    let mut buf = vec![0u8; scan_limit];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut buf)?;
    if let Some(pos) = buf.windows(XP3_MAGIC.len()).position(|w| w == XP3_MAGIC) {
        return Ok(pos as u64);
    }
    bail!("XP3 magic header not found")
}

fn parse_index(
    data: &[u8],
    entries: &mut BTreeMap<String, Xp3Entry>,
    protected_count: &mut usize,
) -> Result<()> {
    let mut cur = 0usize;
    while cur + 12 <= data.len() {
        let tag = &data[cur..cur + 4];
        let chunk_size = u64::from_le_bytes(data[cur + 4..cur + 12].try_into()?) as usize;
        cur += 12;
        if cur + chunk_size > data.len() {
            break;
        }
        let chunk = &data[cur..cur + chunk_size];
        cur += chunk_size;

        if tag != b"File" {
            continue;
        }
        let mut sub = 0usize;
        let mut name: Option<String> = None;
        let mut protected = false;
        let mut segments = Vec::new();

        while sub + 12 <= chunk.len() {
            let stag = &chunk[sub..sub + 4];
            let ssize = u64::from_le_bytes(chunk[sub + 4..sub + 12].try_into()?) as usize;
            sub += 12;
            if sub + ssize > chunk.len() {
                break;
            }
            let body = &chunk[sub..sub + ssize];
            sub += ssize;

            match stag {
                b"info" if body.len() >= 22 => {
                    let flags = u32::from_le_bytes(body[0..4].try_into()?);
                    protected = (flags & 0x8000_0000) != 0;
                    // [4..12] orig_size, [12..20] packed_size
                    let name_len = u16::from_le_bytes(body[20..22].try_into()?) as usize;
                    let byte_len = name_len * 2;
                    if 22 + byte_len <= body.len() {
                        let units: Vec<u16> = body[22..22 + byte_len]
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|c| u16::from_le_bytes(*c))
                            .collect();
                        name = Some(String::from_utf16_lossy(&units));
                    }
                }
                b"segm" => {
                    let mut p = 0usize;
                    while p + 28 <= body.len() {
                        let flags = u32::from_le_bytes(body[p..p + 4].try_into()?);
                        let offset = u64::from_le_bytes(body[p + 4..p + 12].try_into()?);
                        let orig_size = u64::from_le_bytes(body[p + 12..p + 20].try_into()?);
                        let packed_size = u64::from_le_bytes(body[p + 20..p + 28].try_into()?);
                        segments.push(Xp3Segment {
                            compressed: (flags & 7) != 0,
                            offset,
                            orig_size,
                            packed_size,
                        });
                        p += 28;
                    }
                }
                _ => {}
            }
        }

        if let Some(raw_name) = name {
            if protected {
                *protected_count += 1;
            }
            entries.insert(
                normalize_vfs_path(&raw_name),
                Xp3Entry {
                    protected,
                    segments,
                },
            );
        }
    }
    Ok(())
}

fn read_u8(r: &mut impl Read) -> Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}
fn read_u32_le(r: &mut impl Read) -> Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64_le(r: &mut impl Read) -> Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;

    /// Builds a valid standard XP3 archive in bytes for deterministic tests.
    pub fn build_xp3(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(XP3_MAGIC);
        let idx_offset_pos = out.len();
        out.extend_from_slice(&0u64.to_le_bytes()); // placeholder for index offset

        let mut index_plain = Vec::new();
        for (name, content, compress) in files {
            let seg_offset = out.len() as u64;
            let payload = if *compress {
                let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
                enc.write_all(content).unwrap();
                enc.finish().unwrap()
            } else {
                content.to_vec()
            };
            out.extend_from_slice(&payload);

            // Build `File` chunk
            let mut file_body = Vec::new();
            // `info` subchunk
            let utf16: Vec<u16> = name.encode_utf16().collect();
            let mut info = Vec::new();
            info.extend_from_slice(&0u32.to_le_bytes()); // flags (unprotected)
            info.extend_from_slice(&(content.len() as u64).to_le_bytes());
            info.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            info.extend_from_slice(&(utf16.len() as u16).to_le_bytes());
            for u in utf16 {
                info.extend_from_slice(&u.to_le_bytes());
            }
            file_body.extend_from_slice(b"info");
            file_body.extend_from_slice(&(info.len() as u64).to_le_bytes());
            file_body.extend_from_slice(&info);

            // `segm` subchunk
            let mut segm = Vec::new();
            let seg_flags: u32 = if *compress { 1 } else { 0 };
            segm.extend_from_slice(&seg_flags.to_le_bytes());
            segm.extend_from_slice(&seg_offset.to_le_bytes());
            segm.extend_from_slice(&(content.len() as u64).to_le_bytes());
            segm.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            file_body.extend_from_slice(b"segm");
            file_body.extend_from_slice(&(segm.len() as u64).to_le_bytes());
            file_body.extend_from_slice(&segm);

            index_plain.extend_from_slice(b"File");
            index_plain.extend_from_slice(&(file_body.len() as u64).to_le_bytes());
            index_plain.extend_from_slice(&file_body);
        }

        let index_offset = out.len() as u64;
        out[idx_offset_pos..idx_offset_pos + 8].copy_from_slice(&index_offset.to_le_bytes());

        // Compressed index
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&index_plain).unwrap();
        let index_packed = enc.finish().unwrap();
        out.push(1); // compressed flag
        out.extend_from_slice(&(index_packed.len() as u64).to_le_bytes());
        out.extend_from_slice(&(index_plain.len() as u64).to_le_bytes());
        out.extend_from_slice(&index_packed);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xp3_roundtrip_compressed_and_raw() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.xp3");
        let bytes = test_util::build_xp3(&[
            ("scenario/first.ks", b"*start\nHello KAG![p]\n", true),
            ("bgimage/sky.txt", b"raw-payload", false),
        ]);
        std::fs::write(&path, bytes).unwrap();

        let xp3 = Xp3Archive::open(&path).unwrap();
        assert_eq!(xp3.list_files().len(), 2);
        assert_eq!(
            xp3.read_if_exists("scenario/first.ks").unwrap().unwrap(),
            b"*start\nHello KAG![p]\n"
        );
        assert_eq!(
            xp3.read_if_exists("bgimage/sky.txt").unwrap().unwrap(),
            b"raw-payload"
        );
    }
}
