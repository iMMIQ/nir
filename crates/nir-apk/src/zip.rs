//! Minimal ZIP writer and reader for APK assembly. Supports the STORE and
//! DEFLATE methods only, no data descriptors (sizes and CRC are patched into
//! the local header after the entry data via a seek), no ZIP64, no encryption.
#![forbid(unsafe_code)]

use anyhow::{bail, ensure, Context, Result};
use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression, Crc};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub(crate) const LOCAL_HEADER_SIG: u32 = 0x0403_4b50;
pub(crate) const CENTRAL_SIG: u32 = 0x0201_4b50;
pub(crate) const EOCD_SIG: u32 = 0x0605_4b50;
const VERSION_NEEDED: u16 = 20;
pub(crate) const METHOD_STORE: u16 = 0;
pub(crate) const METHOD_DEFLATE: u16 = 8;
/// Fixed DOS timestamp (1980-01-01 00:00) so builds are reproducible.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = 1 | (1 << 5);
/// EOCD limits: single disk, u16 entry counts, u32 sizes and offsets.
pub(crate) const MAX_ENTRIES: u32 = 65_534;
const MAX_U32: u64 = u32::MAX as u64;

/// Entry compression method.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Store,
    Deflate,
}

impl Method {
    fn code(self) -> u16 {
        match self {
            Method::Store => METHOD_STORE,
            Method::Deflate => METHOD_DEFLATE,
        }
    }
}

/// Central-directory record for one stored entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryRecord {
    pub name: String,
    pub method: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub local_offset: u32,
    /// Extra field bytes (alignment padding), mirrored in both headers.
    pub extra: Vec<u8>,
}

/// Streaming ZIP writer. Entries are appended in order; [`ZipWriter::finish`]
/// writes the central directory and returns its bytes together with the
/// contents length so the caller can insert an APK signing block between the
/// contents and the central directory.
pub struct ZipWriter<W> {
    out: W,
    offset: u64,
    entries: Vec<EntryRecord>,
}

impl<W: Write + Seek> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            offset: 0,
            entries: Vec::new(),
        }
    }

    /// Appends one entry, streaming `input` through the chosen method.
    pub fn add(&mut self, name: &str, method: Method, input: &mut dyn Read) -> Result<()> {
        self.add_aligned(name, method, input, 0)
    }

    /// Appends one entry, aligning the data of STORED entries to `align`
    /// bytes via a zipalign-style padding extra field (Android loaders want
    /// uncompressed `lib/**` entries page-aligned). `align` must be 0 or a
    /// power of two; it is ignored for DEFLATE.
    pub fn add_aligned(
        &mut self,
        name: &str,
        method: Method,
        input: &mut dyn Read,
        align: u16,
    ) -> Result<()> {
        ensure!(
            self.entries.len() < MAX_ENTRIES as usize,
            "E_ZIP_ENTRY_COUNT: too many entries"
        );
        ensure!(name.len() <= u16::MAX as usize, "E_ZIP_NAME: too long");
        ensure!(
            name.bytes()
                .all(|b| b.is_ascii() && b != b'\\' && (0x20..0x7f).contains(&b)),
            "E_ZIP_NAME: {name:?} must be printable ASCII with forward slashes"
        );
        let extra = alignment_extra(self.offset, name, method, align)?;
        let local_offset = self.offset;
        let header = local_header(name, method, &extra, 0, 0, 0);
        self.out.write_all(&header).context("E_ZIP_WRITE")?;
        self.offset += header.len() as u64;
        let mut crc = Crc::new();
        let mut uncompressed: u64 = 0;
        let mut buffer = [0u8; 64 * 1024];
        let compressed = {
            let mut written = CountingWriter {
                out: &mut self.out,
                count: 0,
            };
            match method {
                Method::Store => loop {
                    let n = input.read(&mut buffer).context("E_ZIP_READ")?;
                    if n == 0 {
                        break;
                    }
                    crc.update(&buffer[..n]);
                    uncompressed += n as u64;
                    written.write_all(&buffer[..n]).context("E_ZIP_WRITE")?;
                },
                Method::Deflate => {
                    let mut encoder = DeflateEncoder::new(&mut written, Compression::new(6));
                    loop {
                        let n = input.read(&mut buffer).context("E_ZIP_READ")?;
                        if n == 0 {
                            break;
                        }
                        crc.update(&buffer[..n]);
                        uncompressed += n as u64;
                        encoder.write_all(&buffer[..n]).context("E_ZIP_WRITE")?;
                    }
                    encoder.finish().context("E_ZIP_WRITE")?;
                }
            }
            written.count
        };
        self.offset += compressed;
        ensure!(
            compressed <= MAX_U32 && uncompressed <= MAX_U32,
            "E_ZIP_TOO_LARGE: entry {name:?} exceeds ZIP limits"
        );
        let header = local_header(name, method, &extra, crc.sum(), compressed, uncompressed);
        self.out
            .seek(SeekFrom::Start(local_offset))
            .context("E_ZIP_SEEK")?;
        self.out.write_all(&header).context("E_ZIP_WRITE")?;
        self.out
            .seek(SeekFrom::Start(self.offset))
            .context("E_ZIP_SEEK")?;
        self.entries.push(EntryRecord {
            name: name.to_owned(),
            method: method.code(),
            crc32: crc.sum(),
            compressed_size: compressed as u32,
            uncompressed_size: uncompressed as u32,
            local_offset: local_offset as u32,
            extra,
        });
        Ok(())
    }

    /// Appends one in-memory entry.
    pub fn add_bytes(&mut self, name: &str, method: Method, data: &[u8]) -> Result<()> {
        self.add_bytes_aligned(name, method, data, 0)
    }

    /// Appends one in-memory entry, aligning it like `add_aligned`.
    pub fn add_bytes_aligned(
        &mut self,
        name: &str,
        method: Method,
        data: &[u8],
        align: u16,
    ) -> Result<()> {
        let mut cursor = std::io::Cursor::new(data);
        self.add_aligned(name, method, &mut cursor, align)
    }

    /// Builds the central directory without writing it, returning its bytes
    /// and the contents length (the offset at which an APK signing block will
    /// be inserted before the directory is appended).
    pub fn finish(&mut self) -> Result<(Vec<u8>, u64)> {
        let central = central_directory(&self.entries)?;
        ensure!(
            self.offset + central.len() as u64 <= MAX_U32,
            "E_ZIP_TOO_LARGE: archive exceeds ZIP limits"
        );
        Ok((central, self.offset))
    }

    /// Number of entries written so far.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the underlying writer.
    pub fn into_inner(self) -> W {
        self.out
    }
}

struct CountingWriter<'a, W> {
    out: &'a mut W,
    count: u64,
}

impl<W: Write> Write for CountingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.out.write(buf)?;
        self.count += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// Computes the zipalign padding extra field (ID 0xd935, as emitted by
/// Android's zipalign and aapt2) so the entry data lands on an `align`-byte
/// boundary.
fn alignment_extra(offset: u64, name: &str, method: Method, align: u16) -> Result<Vec<u8>> {
    ensure!(
        align == 0 || align.is_power_of_two(),
        "E_ZIP_ALIGN: alignment must be a power of two"
    );
    if align == 0 || method != Method::Store {
        return Ok(Vec::new());
    }
    let align = align as usize;
    let header = offset as usize + 30 + name.len();
    let mut padding = (align - header % align) % align;
    if padding > 0 && padding < 4 {
        padding += align;
    }
    if padding == 0 {
        return Ok(Vec::new());
    }
    let mut extra = Vec::with_capacity(padding);
    extra.extend_from_slice(&0xd935u16.to_le_bytes());
    extra.extend_from_slice(&((padding - 4) as u16).to_le_bytes());
    extra.resize(padding, 0);
    ensure!(
        (header + extra.len()).is_multiple_of(align),
        "E_ZIP_ALIGN: padding computation"
    );
    Ok(extra)
}

fn local_header(
    name: &str,
    method: Method,
    extra: &[u8],
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(30 + name.len() + extra.len());
    out.extend_from_slice(&LOCAL_HEADER_SIG.to_le_bytes());
    out.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // flags
    out.extend_from_slice(&method.code().to_le_bytes());
    out.extend_from_slice(&DOS_TIME.to_le_bytes());
    out.extend_from_slice(&DOS_DATE.to_le_bytes());
    out.extend_from_slice(&crc32.to_le_bytes());
    out.extend_from_slice(&(compressed_size as u32).to_le_bytes());
    out.extend_from_slice(&(uncompressed_size as u32).to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&(extra.len() as u16).to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(extra);
    out
}

/// Builds the central directory bytes for the given records.
pub(crate) fn central_directory(entries: &[EntryRecord]) -> Result<Vec<u8>> {
    ensure!(
        entries.len() <= MAX_ENTRIES as usize,
        "E_ZIP_ENTRY_COUNT: too many entries"
    );
    let mut out = Vec::new();
    for entry in entries {
        out.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
        out.extend_from_slice(&VERSION_NEEDED.to_le_bytes()); // made by
        out.extend_from_slice(&VERSION_NEEDED.to_le_bytes()); // needed to extract
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&entry.method.to_le_bytes());
        out.extend_from_slice(&DOS_TIME.to_le_bytes());
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&entry.crc32.to_le_bytes());
        out.extend_from_slice(&entry.compressed_size.to_le_bytes());
        out.extend_from_slice(&entry.uncompressed_size.to_le_bytes());
        out.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entry.extra.len() as u16).to_le_bytes()); // extra
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        out.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        out.extend_from_slice(&entry.local_offset.to_le_bytes());
        out.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(&entry.extra);
    }
    Ok(out)
}

/// Builds the 22-byte EOCD record for a central directory at `cd_offset`.
pub(crate) fn eocd(entry_count: u32, cd_size: u64, cd_offset: u64) -> Result<Vec<u8>> {
    ensure!(entry_count <= MAX_ENTRIES, "E_ZIP_ENTRY_COUNT");
    let mut out = Vec::with_capacity(22);
    out.extend_from_slice(&EOCD_SIG.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // central directory disk
    out.extend_from_slice(&(entry_count as u16).to_le_bytes());
    out.extend_from_slice(&(entry_count as u16).to_le_bytes());
    out.extend_from_slice(&(cd_size as u32).to_le_bytes());
    out.extend_from_slice(&(cd_offset as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    Ok(out)
}

/// Locates the EOCD record in `tail` (the trailing bytes of an archive) and
/// returns its offset within `tail`. Fails when data follows the EOCD.
pub(crate) fn find_eocd(tail: &[u8]) -> Result<usize> {
    ensure!(tail.len() >= 22, "E_ZIP_EOCD: archive too short");
    let scan = tail.len().min(22 + u16::MAX as usize);
    for offset in (tail.len() - scan..=tail.len() - 22).rev() {
        let signature = u32::from_le_bytes(tail[offset..offset + 4].try_into().unwrap());
        if signature != EOCD_SIG {
            continue;
        }
        let comment = u16::from_le_bytes(tail[offset + 20..offset + 22].try_into().unwrap());
        ensure!(
            offset + 22 + comment as usize == tail.len(),
            "E_ZIP_EOCD: trailing data after EOCD"
        );
        return Ok(offset);
    }
    bail!("E_ZIP_EOCD: record not found")
}

/// A parsed ZIP entry, used by tests and the verifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    pub method: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub local_offset: u32,
}

/// Reads the central directory of a (possibly signing-block-carrying) ZIP.
pub fn read_entries(data: &[u8]) -> Result<Vec<ZipEntry>> {
    let eocd = find_eocd(data)?;
    let cd_offset = u32::from_le_bytes(data[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
    let cd_size = u32::from_le_bytes(data[eocd + 12..eocd + 16].try_into().unwrap()) as usize;
    ensure!(
        cd_offset.checked_add(cd_size) == Some(eocd),
        "E_ZIP_CENTRAL: central directory does not abut the EOCD"
    );
    let mut entries = Vec::new();
    let mut offset = cd_offset;
    while offset < eocd {
        let record = data
            .get(offset..)
            .context("E_ZIP_CENTRAL: truncated record")?;
        ensure!(record.len() >= 46, "E_ZIP_CENTRAL: truncated record header");
        let signature = u32::from_le_bytes(record[..4].try_into().unwrap());
        ensure!(signature == CENTRAL_SIG, "E_ZIP_CENTRAL: bad signature");
        let name_len = u16::from_le_bytes(record[28..30].try_into().unwrap()) as usize;
        let extra_len = u16::from_le_bytes(record[30..32].try_into().unwrap()) as usize;
        let comment_len = u16::from_le_bytes(record[32..34].try_into().unwrap()) as usize;
        let total = 46 + name_len + extra_len + comment_len;
        let name = String::from_utf8(
            record
                .get(46..46 + name_len)
                .context("E_ZIP_CENTRAL: truncated name")?
                .to_vec(),
        )
        .context("E_ZIP_NAME: not UTF-8")?;
        entries.push(ZipEntry {
            name,
            method: u16::from_le_bytes(record[10..12].try_into().unwrap()),
            crc32: u32::from_le_bytes(record[16..20].try_into().unwrap()),
            compressed_size: u32::from_le_bytes(record[20..24].try_into().unwrap()),
            uncompressed_size: u32::from_le_bytes(record[24..28].try_into().unwrap()),
            local_offset: u32::from_le_bytes(record[42..46].try_into().unwrap()),
        });
        offset += total;
    }
    Ok(entries)
}

/// Extracts and decompresses one entry from an in-memory archive.
pub fn extract_entry(data: &[u8], entry: &ZipEntry) -> Result<Vec<u8>> {
    let local = data
        .get(entry.local_offset as usize..)
        .context("E_ZIP_LOCAL: offset out of bounds")?;
    ensure!(local.len() >= 30, "E_ZIP_LOCAL: truncated header");
    let signature = u32::from_le_bytes(local[..4].try_into().unwrap());
    ensure!(signature == LOCAL_HEADER_SIG, "E_ZIP_LOCAL: bad signature");
    let name_len = u16::from_le_bytes(local[26..28].try_into().unwrap()) as usize;
    let extra_len = u16::from_le_bytes(local[28..30].try_into().unwrap()) as usize;
    let start = 30 + name_len + extra_len;
    let payload = local
        .get(start..start + entry.compressed_size as usize)
        .context("E_ZIP_LOCAL: truncated data")?;
    let output = match entry.method {
        METHOD_STORE => payload.to_vec(),
        METHOD_DEFLATE => {
            let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
            DeflateDecoder::new(payload)
                .read_to_end(&mut out)
                .context("E_ZIP_DEFLATE")?;
            out
        }
        other => bail!("E_ZIP_METHOD: unsupported method {other}"),
    };
    ensure!(
        output.len() as u32 == entry.uncompressed_size,
        "E_ZIP_SIZE: {} decompressed size mismatch",
        entry.name
    );
    ensure!(
        {
            let mut crc = Crc::new();
            crc.update(&output);
            crc.sum()
        } == entry.crc32,
        "E_ZIP_CRC: {} CRC mismatch",
        entry.name
    );
    Ok(output)
}

/// Streams a file from disk into the writer, aligning STORED entries.
pub(crate) fn add_file_aligned<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    name: &str,
    method: Method,
    path: &Path,
    align: u16,
) -> Result<()> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("E_ZIP_OPEN: {}", path.display()))?;
    writer.add_aligned(name, method, &mut file, align)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn crc32_known_vectors() {
        // ISO/IEC 8802-3 check values.
        let cases: &[(&[u8], u32)] = &[
            (b"", 0x0000_0000),
            (b"a", 0xe8b7_be43),
            (b"abc", 0x3524_41c2),
            (b"123456789", 0xcbf4_3926),
            (b"The quick brown fox jumps over the lazy dog", 0x414f_a339),
            (&[0xff; 32], 0xff6c_ab0b),
        ];
        for (input, expected) in cases {
            let mut crc = Crc::new();
            crc.update(input);
            assert_eq!(crc.sum(), *expected, "{input:?}");
        }
    }

    #[test]
    fn store_and_deflate_round_trip() {
        for method in [Method::Store, Method::Deflate] {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            let lorem: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
            writer.add_bytes("dir/lorem.bin", method, &lorem).unwrap();
            writer.add_bytes("empty.txt", method, b"").unwrap();
            writer
                .add_bytes("ascii.txt", method, "contents".as_bytes())
                .unwrap();
            let (central, contents_len) = writer.finish().unwrap();
            let contents = writer.into_inner().into_inner();
            assert_eq!(contents_len, contents.len() as u64);
            let mut archive = contents;
            archive.extend_from_slice(&central);
            archive.extend_from_slice(&eocd(3, central.len() as u64, contents_len).unwrap());
            let entries = read_entries(&archive).unwrap();
            assert_eq!(entries.len(), 3);
            assert_eq!(entries[0].name, "dir/lorem.bin");
            assert_eq!(entries[0].method, method.code());
            if method == Method::Deflate {
                assert!(entries[0].compressed_size < entries[0].uncompressed_size);
            }
            for (name, expected) in [
                ("dir/lorem.bin", lorem.clone()),
                ("empty.txt", Vec::new()),
                ("ascii.txt", b"contents".to_vec()),
            ] {
                let entry = entries.iter().find(|e| e.name == name).unwrap();
                assert_eq!(extract_entry(&archive, entry).unwrap(), expected);
            }
        }
    }

    #[test]
    fn rejects_corrupt_archives() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_bytes("a.txt", Method::Deflate, b"payload")
            .unwrap();
        let (central, contents_len) = writer.finish().unwrap();
        let contents = writer.into_inner().into_inner();
        let mut archive = contents.clone();
        archive.extend_from_slice(&central);
        archive.extend_from_slice(&eocd(1, central.len() as u64, contents_len).unwrap());
        let entries = read_entries(&archive).unwrap();
        assert_eq!(entries.len(), 1);
        // Corrupt the payload: CRC and deflate both fail.
        let mut corrupted = archive.clone();
        let data_start = entries[0].local_offset as usize + 30 + entries[0].name.len();
        corrupted[data_start + 2] ^= 0xff;
        assert!(extract_entry(&corrupted, &entries[0]).is_err());
        let mut truncated = archive.clone();
        truncated.truncate(10);
        assert!(read_entries(&truncated).is_err());
        let mut trailing = archive;
        trailing.extend_from_slice(b"junk"); // data after EOCD
        assert!(read_entries(&trailing).is_err());
    }

    #[test]
    fn rejects_bad_names() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        assert!(writer.add_bytes("back\\slash", Method::Store, b"").is_err());
        assert!(writer.add_bytes("nul\0byte", Method::Store, b"").is_err());
        assert!(writer.add_bytes("hi\u{e9}", Method::Store, b"").is_err());
        writer
            .add_bytes("ok/name.bin", Method::Store, b"x")
            .unwrap();
        assert_eq!(writer.len(), 1);
        assert!(!writer.is_empty());
    }

    #[test]
    fn stored_entries_align_to_page() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_bytes("first.txt", Method::Deflate, &[7u8; 100])
            .unwrap();
        let mut library = &[0u8; 32][..];
        writer
            .add_aligned(
                "lib/arm64-v8a/libplayer.so",
                Method::Store,
                &mut library,
                4096,
            )
            .unwrap();
        let (central, contents_len) = writer.finish().unwrap();
        let contents = writer.into_inner().into_inner();
        let mut archive = contents.clone();
        archive.extend_from_slice(&central);
        archive.extend_from_slice(&eocd(2, central.len() as u64, contents_len).unwrap());
        let entries = read_entries(&archive).unwrap();
        let lib = &entries[1];
        // Data start = local offset + 30 + name + extra; must be 4096-aligned.
        let extra_len = u16::from_le_bytes(
            contents[lib.local_offset as usize + 28..lib.local_offset as usize + 30]
                .try_into()
                .unwrap(),
        ) as usize;
        let data_start = lib.local_offset as usize + 30 + lib.name.len() + extra_len;
        assert_eq!(data_start % 4096, 0);
        // The extra field ID is the zipalign padding ID.
        let extra = &archive[lib.local_offset as usize + 30 + lib.name.len()..data_start];
        assert_eq!(&extra[..2], &0xd935u16.to_le_bytes());
        assert_eq!(extract_entry(&archive, lib).unwrap(), vec![0u8; 32]);
    }
}
