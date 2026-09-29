//! APK Signature Scheme v2 signer and verifier
//! (https://source.android.com/docs/security/features/apksigning/v2).
//!
//! The APK is viewed as four sections: ZIP entry contents, the APK signing
//! block, the central directory, and the EOCD. v2 protects sections 1, 3 and
//! 4: each section is split into consecutive 1 MiB chunks; a chunk digest is
//! SHA-256 over `0xa5 || u32-le chunk length || chunk`; the content digest is
//! SHA-256 over `0x5a || u32-le total chunk count || all chunk digests in
//! file order` (one combined digest across the three sections, matching
//! apksigner). While section 4 is digested, the EOCD's central-directory
//! offset field is treated as pointing at the start of the signing block.
//!
//! The signing block carries one signer: ECDSA with SHA-256 (algorithm
//! 0x0201), the certificate, and the SubjectPublicKeyInfo.
#![forbid(unsafe_code)]

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use crate::key::SigningIdentity;

/// ID of the APK Signature Scheme v2 block inside the APK signing block.
pub const V2_BLOCK_ID: u32 = 0x7109_871a;
pub const APK_SIG_BLOCK_MAGIC: &[u8; 16] = b"APK Sig Block 42";
/// Maximum size of a digest chunk.
pub const CHUNK_SIZE: usize = 1024 * 1024;
/// ECDSA with SHA-256.
pub const ALGORITHM_ECDSA_SHA256: u32 = 0x0201;

/// Streaming digester implementing the chunked SHA-256 topology. Sections are
/// fed in file order; each new section starts a fresh chunk boundary.
#[derive(Clone)]
pub struct ChunkedDigester {
    pending: Vec<u8>,
    digests: Vec<u8>,
    chunks: u32,
}

impl ChunkedDigester {
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(CHUNK_SIZE),
            digests: Vec::new(),
            chunks: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut offset = 0;
        while offset < data.len() {
            let room = CHUNK_SIZE - self.pending.len();
            let take = room.min(data.len() - offset);
            self.pending.extend_from_slice(&data[offset..offset + take]);
            offset += take;
            if self.pending.len() == CHUNK_SIZE {
                self.emit();
            }
        }
    }

    /// Streams exactly `length` bytes from `input` into the digester.
    pub fn update_from(&mut self, mut input: impl Read, length: u64) -> Result<()> {
        let mut buffer = vec![0u8; 64 * 1024];
        let mut remaining = length;
        while remaining > 0 {
            let want = buffer.len().min(remaining as usize);
            let read = input.read(&mut buffer[..want]).context("E_SIGN_READ")?;
            ensure!(read > 0, "E_SIGN_READ: unexpected end of input");
            self.update(&buffer[..read]);
            remaining -= read as u64;
        }
        Ok(())
    }

    /// Ends a section: a trailing partial chunk is flushed so the next
    /// section starts on a fresh chunk boundary, as the scheme requires.
    pub fn end_section(&mut self) {
        if !self.pending.is_empty() {
            self.emit();
        }
    }

    fn emit(&mut self) {
        let mut hasher = Sha256::new();
        hasher.update([0xa5]);
        hasher.update((self.pending.len() as u32).to_le_bytes());
        hasher.update(&self.pending);
        self.digests.extend_from_slice(&hasher.finalize());
        self.chunks += 1;
        self.pending.clear();
    }

    /// Finishes the combined digest across all fed sections.
    pub fn finish(mut self) -> [u8; 32] {
        self.end_section();
        let mut hasher = Sha256::new();
        hasher.update([0x5a]);
        hasher.update(self.chunks.to_le_bytes());
        hasher.update(&self.digests);
        hasher.finalize().into()
    }
}

impl Default for ChunkedDigester {
    fn default() -> Self {
        Self::new()
    }
}

/// Computes the v2 content digest over the three protected sections:
/// `contents` (from the start of the file to the signing block), the central
/// directory, and the EOCD with its central-directory offset replaced by
/// `block_offset` (the patched view required by the scheme).
pub fn content_digest(
    mut contents: impl Read,
    contents_length: u64,
    central_directory: &[u8],
    eocd_patched: &[u8],
) -> Result<[u8; 32]> {
    let mut digester = ChunkedDigester::new();
    digester.update_from(&mut contents, contents_length)?;
    digester.end_section();
    digester.update(central_directory);
    digester.end_section();
    digester.update(eocd_patched);
    Ok(digester.finish())
}

/// Length-prefixed (u32 little-endian) byte sequence helper.
fn len_prefixed(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

/// apksig's `encodeAsSequenceOfLengthPrefixedElements`: the concatenation of
/// u32-length-prefixed elements with NO outer prefix (the caller wraps when
/// the format asks for a "length-prefixed sequence").
fn concat_prefixed(elements: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for element in elements {
        out.extend_from_slice(&len_prefixed(element));
    }
    out
}

/// Builds a `pair of u32 algorithm ID + length-prefixed payload`, itself
/// length-prefixed — the element form used in the digest and signature lists.
fn algorithm_entry(algorithm: u32, payload: &[u8]) -> Vec<u8> {
    let mut pair = Vec::with_capacity(8 + payload.len());
    pair.extend_from_slice(&algorithm.to_le_bytes());
    pair.extend_from_slice(&len_prefixed(payload));
    len_prefixed(&pair)
}

/// Builds the complete APK signing block (size fields and magic included)
/// holding one v2 signer.
pub fn signing_block(signer: &SigningIdentity, digest: &[u8; 32]) -> Vec<u8> {
    let digests = algorithm_entry(ALGORITHM_ECDSA_SHA256, digest);
    let certificates = concat_prefixed(&[signer.certificate_der()]);
    let signed_data = concat_prefixed(&[&digests, &certificates, &[]]);
    let signature = signer.sign_der(&signed_data);
    let signatures = algorithm_entry(ALGORITHM_ECDSA_SHA256, &signature);
    let signer_block = concat_prefixed(&[&signed_data, &signatures, &signer.public_key_der()]);
    let v2_block = len_prefixed(&concat_prefixed(&[&signer_block]));
    let mut out = Vec::with_capacity(v2_block.len() + 32);
    let block_size = (8 + 4 + v2_block.len() + 8 + 16) as u64;
    out.extend_from_slice(&block_size.to_le_bytes());
    out.extend_from_slice(&(4 + v2_block.len() as u64).to_le_bytes());
    out.extend_from_slice(&V2_BLOCK_ID.to_le_bytes());
    out.extend_from_slice(&v2_block);
    out.extend_from_slice(&block_size.to_le_bytes());
    out.extend_from_slice(APK_SIG_BLOCK_MAGIC);
    out
}

/// Successful verification result.
#[derive(Clone, Debug)]
pub struct Verified {
    /// Number of v2 signers found (this crate always emits exactly one).
    pub signers: usize,
    /// Recomputed SHA-256 content digest of the APK.
    pub content_digest: [u8; 32],
    /// DER certificate of the first signer.
    pub certificate_der: Vec<u8>,
}

/// Verifies the APK Signature Scheme v2 block of the APK at `path`:
/// locates the signing block, checks its framing, verifies the ECDSA
/// signature over the signed data, recomputes the chunked content digest,
/// and checks the certificate matches the signing key.
pub fn verify_apk(path: &Path) -> Result<Verified> {
    let mut file = File::open(path).context("E_VERIFY_OPEN")?;
    let file_length = file.metadata().context("E_VERIFY_META")?.len();
    // Locate the EOCD: read the tail, scanning backwards.
    let tail_length = (22 + u16::MAX as u64).min(file_length);
    file.seek(SeekFrom::Start(file_length - tail_length))
        .context("E_VERIFY_SEEK")?;
    let mut tail = vec![0u8; tail_length as usize];
    file.read_exact(&mut tail).context("E_VERIFY_READ")?;
    let eocd_in_tail = crate::zip::find_eocd(&tail).context("E_VERIFY_EOCD")?;
    let eocd_offset = file_length - tail_length + eocd_in_tail as u64;
    let eocd = &tail[eocd_in_tail..];
    let cd_offset = u64::from(u32::from_le_bytes(eocd[16..20].try_into().unwrap()));
    let cd_size = u64::from(u32::from_le_bytes(eocd[12..16].try_into().unwrap()));
    ensure!(
        cd_offset + cd_size == eocd_offset,
        "E_VERIFY_LAYOUT: EOCD does not immediately follow the central directory"
    );
    // Locate the signing block ending at the central directory.
    ensure!(
        cd_offset >= 32,
        "E_VERIFY_BLOCK: no room for a signing block"
    );
    let mut trailer = [0u8; 24];
    file.seek(SeekFrom::Start(cd_offset - 24))
        .context("E_VERIFY_SEEK")?;
    file.read_exact(&mut trailer).context("E_VERIFY_READ")?;
    ensure!(
        &trailer[8..] == APK_SIG_BLOCK_MAGIC,
        "E_VERIFY_BLOCK: magic not found"
    );
    let block_size = u64::from_le_bytes(trailer[..8].try_into().unwrap());
    ensure!(
        block_size >= 24 && block_size + 8 <= cd_offset,
        "E_VERIFY_BLOCK: bad size"
    );
    let block_start = cd_offset - block_size - 8;
    let mut header = [0u8; 8];
    file.seek(SeekFrom::Start(block_start))
        .context("E_VERIFY_SEEK")?;
    file.read_exact(&mut header).context("E_VERIFY_READ")?;
    ensure!(
        u64::from_le_bytes(header) == block_size,
        "E_VERIFY_BLOCK: size fields disagree"
    );
    // Walk the ID-value pairs.
    let pairs_end = block_start + 8 + block_size - 24;
    let mut position = block_start + 8;
    let mut v2_block: Option<Vec<u8>> = None;
    while position < pairs_end {
        let mut size_buf = [0u8; 8];
        file.seek(SeekFrom::Start(position))
            .context("E_VERIFY_SEEK")?;
        file.read_exact(&mut size_buf).context("E_VERIFY_READ")?;
        let pair_size = u64::from_le_bytes(size_buf);
        ensure!(
            pair_size >= 4 && position + 8 + pair_size <= pairs_end,
            "E_VERIFY_BLOCK: malformed ID-value pair"
        );
        let mut id_buf = [0u8; 4];
        file.read_exact(&mut id_buf).context("E_VERIFY_READ")?;
        let id = u32::from_le_bytes(id_buf);
        let value_length = (pair_size - 4) as usize;
        let mut value = vec![0u8; value_length];
        file.read_exact(&mut value).context("E_VERIFY_READ")?;
        if id == V2_BLOCK_ID && v2_block.is_none() {
            v2_block = Some(value);
        }
        position += 8 + pair_size;
    }
    let v2_block = v2_block.context("E_VERIFY_BLOCK: no v2 block")?;

    // Parse the v2 block: a length-prefixed sequence of length-prefixed
    // signer blocks.
    let signers = parse_elements(strip_prefix(&v2_block).context("E_VERIFY_BLOCK: v2 prefix")?)
        .context("E_VERIFY_BLOCK: signers")?;
    ensure!(!signers.is_empty(), "E_VERIFY_BLOCK: no signers");
    let mut certificate = None;
    let mut digest_out = None;
    for signer_bytes in &signers {
        let fields = parse_elements(signer_bytes).context("E_VERIFY_BLOCK: signer fields")?;
        ensure!(fields.len() == 3, "E_VERIFY_BLOCK: signer field count");
        let (signed_data, signatures, public_key) = (&fields[0], &fields[1], &fields[2]);
        let signed_fields =
            parse_elements(signed_data).context("E_VERIFY_BLOCK: signed data fields")?;
        ensure!(
            signed_fields.len() == 3,
            "E_VERIFY_BLOCK: signed data shape"
        );
        let digest_entries =
            parse_algorithm_entries(signed_fields[0]).context("E_VERIFY_BLOCK: digests")?;
        let certificates =
            parse_elements(signed_fields[1]).context("E_VERIFY_BLOCK: certificates")?;
        let sig_entries = parse_algorithm_entries(signatures).context("E_VERIFY_BLOCK: sigs")?;
        ensure!(
            sig_entries.len() == digest_entries.len()
                && sig_entries
                    .iter()
                    .zip(&digest_entries)
                    .all(|(s, d)| s.0 == d.0),
            "E_VERIFY_BLOCK: digest and signature algorithm lists differ"
        );
        let signature = sig_entries
            .iter()
            .find(|(algorithm, _)| *algorithm == ALGORITHM_ECDSA_SHA256)
            .map(|(_, payload)| payload.clone())
            .context("E_VERIFY_BLOCK: no ECDSA/SHA-256 signature")?;
        ensure!(
            SigningIdentity::verify_der(public_key, signed_data, &signature),
            "E_VERIFY_SIGNATURE"
        );
        // Recompute the content digest over the three protected sections.
        let mut eocd_patched = eocd.to_vec();
        eocd_patched[16..20].copy_from_slice(&(block_start as u32).to_le_bytes());
        let mut central_directory = vec![0u8; cd_size as usize];
        file.seek(SeekFrom::Start(cd_offset))
            .context("E_VERIFY_SEEK")?;
        file.read_exact(&mut central_directory)
            .context("E_VERIFY_READ")?;
        file.seek(SeekFrom::Start(0)).context("E_VERIFY_SEEK")?;
        let recomputed = content_digest(&mut file, block_start, &central_directory, &eocd_patched)?;
        let stored = digest_entries
            .iter()
            .find(|(algorithm, _)| *algorithm == ALGORITHM_ECDSA_SHA256)
            .map(|(_, digest)| digest.clone())
            .context("E_VERIFY_BLOCK: no stored digest")?;
        ensure!(recomputed == stored[..], "E_VERIFY_DIGEST: content changed");
        // The first certificate must carry the signing key.
        ensure!(!certificates.is_empty(), "E_VERIFY_BLOCK: no certificates");
        ensure!(
            crate::key::certificate_spki(certificates[0])? == *public_key,
            "E_VERIFY_CERTIFICATE: does not match signing key"
        );
        certificate = Some(certificates[0].to_vec());
        digest_out = Some(recomputed);
    }
    Ok(Verified {
        signers: signers.len(),
        content_digest: digest_out.unwrap(),
        certificate_der: certificate.unwrap(),
    })
}

/// Strips one u32 length prefix, validating that it covers the rest.
fn strip_prefix(data: &[u8]) -> Option<&[u8]> {
    let length = u32::from_le_bytes(data.get(..4)?.try_into().ok()?) as usize;
    let body = data.get(4..4 + length)?;
    (4 + length == data.len()).then_some(body)
}

/// Parses a concatenation of u32-length-prefixed elements.
fn parse_elements(data: &[u8]) -> Option<Vec<&[u8]>> {
    let mut elements = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let length = u32::from_le_bytes(data.get(offset..offset + 4)?.try_into().ok()?) as usize;
        offset += 4;
        elements.push(data.get(offset..offset + length)?);
        offset += length;
    }
    Some(elements)
}

/// Parses pairs of `(u32 algorithm ID, length-prefixed payload)` where each
/// pair is itself length-prefixed.
fn parse_algorithm_entries(data: &[u8]) -> Option<Vec<(u32, Vec<u8>)>> {
    parse_elements(data)?
        .iter()
        .map(|entry| {
            let algorithm = u32::from_le_bytes(entry.get(..4)?.try_into().ok()?);
            let payload = strip_prefix(entry.get(4..)?)?;
            Some((algorithm, payload.to_vec()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_digest_topology() {
        // One small section: 0x5a || count=1 || digest(0xa5||len||data).
        let mut digester = ChunkedDigester::new();
        digester.update(b"nir");
        let digest = digester.finish();
        let mut expected = Sha256::new();
        expected.update([0x5a]);
        expected.update(1u32.to_le_bytes());
        let mut chunk = Sha256::new();
        chunk.update([0xa5]);
        chunk.update(3u32.to_le_bytes());
        chunk.update(b"nir");
        expected.update(chunk.finalize());
        let expected_digest: [u8; 32] = expected.finalize().into();
        assert_eq!(digest, expected_digest);
    }

    #[test]
    fn chunk_boundaries_span_sections() {
        // A 1.5 MiB section followed by a 1 MiB section: three chunks total,
        // and section boundaries reset chunk accumulation.
        let big = vec![0x11u8; CHUNK_SIZE + CHUNK_SIZE / 2];
        let mut digester = ChunkedDigester::new();
        digester.update(&big);
        digester.end_section();
        digester.update(&vec![0x22u8; CHUNK_SIZE]);
        let digest = digester.finish();
        let mut expected = Sha256::new();
        expected.update([0x5a]);
        expected.update(3u32.to_le_bytes());
        for (fill, len) in [
            (0x11u8, CHUNK_SIZE),
            (0x11, CHUNK_SIZE / 2),
            (0x22, CHUNK_SIZE),
        ] {
            let mut chunk = Sha256::new();
            chunk.update([0xa5]);
            chunk.update((len as u32).to_le_bytes());
            chunk.update(&vec![fill; len][..]);
            expected.update(chunk.finalize());
        }
        let expected_digest: [u8; 32] = expected.finalize().into();
        assert_eq!(digest, expected_digest);
    }

    #[test]
    fn empty_input_has_no_chunks() {
        let digest = ChunkedDigester::new().finish();
        let mut expected = Sha256::new();
        expected.update([0x5a]);
        expected.update(0u32.to_le_bytes());
        let expected_digest: [u8; 32] = expected.finalize().into();
        assert_eq!(digest, expected_digest);
    }

    #[test]
    fn sequence_framing() {
        let body = concat_prefixed(&[b"abc", b"de"]);
        assert_eq!(body, [len_prefixed(b"abc"), len_prefixed(b"de")].concat());
        assert_eq!(
            parse_elements(&body).unwrap(),
            vec![b"abc".as_slice(), b"de".as_slice()]
        );
        assert!(parse_elements(&body[..body.len() - 1]).is_none());
        // A length-prefixed sequence (like the v2 block itself) strips once.
        let outer = len_prefixed(&body);
        assert_eq!(strip_prefix(&outer).unwrap(), body.as_slice());
        assert!(strip_prefix(&outer[..outer.len() - 1]).is_none());
    }

    #[test]
    fn algorithm_entry_framing() {
        let entry = algorithm_entry(0x0201, b"digest");
        // Pair length covers the ID and the length-prefixed payload.
        assert_eq!(&entry[..4], &(4 + 4 + 6u32).to_le_bytes());
        assert_eq!(&entry[4..8], &0x0201u32.to_le_bytes());
        assert_eq!(&entry[8..12], &6u32.to_le_bytes());
        assert_eq!(&entry[12..], b"digest");
        assert_eq!(
            parse_algorithm_entries(&entry).unwrap(),
            vec![(0x0201, b"digest".to_vec())]
        );
    }
}
