//! Minimal DER (X.690) encoding and parsing for the self-signed certificate
//! and ECDSA signatures. Only the subset needed by this crate is implemented.
#![forbid(unsafe_code)]

pub(crate) const TAG_INTEGER: u8 = 0x02;
pub(crate) const TAG_BIT_STRING: u8 = 0x03;
pub(crate) const TAG_OID: u8 = 0x06;
pub(crate) const TAG_PRINTABLE_STRING: u8 = 0x13;
pub(crate) const TAG_UTC_TIME: u8 = 0x17;
pub(crate) const TAG_SEQUENCE: u8 = 0x30;
pub(crate) const TAG_SET: u8 = 0x31;

/// Encodes one DER TLV with a definite-length header.
pub(crate) fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 4);
    out.push(tag);
    if content.len() < 0x80 {
        out.push(content.len() as u8);
    } else {
        let mut len = content.len();
        let mut bytes = [0u8; 8];
        let mut n = 0;
        while len > 0 {
            bytes[n] = (len & 0xff) as u8;
            len >>= 8;
            n += 1;
        }
        out.push(0x80 | n as u8);
        for i in (0..n).rev() {
            out.push(bytes[i]);
        }
    }
    out.extend_from_slice(content);
    out
}

/// Encodes `SEQUENCE { parts... }`.
pub(crate) fn sequence(parts: &[&[u8]]) -> Vec<u8> {
    let mut content = Vec::new();
    for part in parts {
        content.extend_from_slice(part);
    }
    tlv(TAG_SEQUENCE, &content)
}

/// Encodes a DER OBJECT IDENTIFIER from its dotted form.
pub(crate) fn oid(parts: &[u64]) -> Vec<u8> {
    let mut content = vec![(parts[0] * 40 + parts[1]) as u8];
    for &part in &parts[2..] {
        let mut stack = [0u8; 10];
        let mut n = 0;
        let mut value = part;
        stack[n] = (value & 0x7f) as u8;
        n += 1;
        value >>= 7;
        while value > 0 {
            stack[n] = 0x80 | (value & 0x7f) as u8;
            n += 1;
            value >>= 7;
        }
        for i in (0..n).rev() {
            content.push(stack[i]);
        }
    }
    tlv(TAG_OID, &content)
}

/// Encodes a small non-negative INTEGER.
pub(crate) fn uint(value: u64) -> Vec<u8> {
    positive_int(&value.to_be_bytes())
}

/// Encodes a positive INTEGER from a big-endian magnitude, adding a zero pad
/// byte when the high bit would make it look negative.
pub(crate) fn positive_int(magnitude: &[u8]) -> Vec<u8> {
    let start = magnitude
        .iter()
        .position(|&b| b != 0)
        .unwrap_or(magnitude.len() - 1);
    let trimmed = &magnitude[start..];
    let mut content = Vec::with_capacity(trimmed.len() + 1);
    if trimmed[0] & 0x80 != 0 {
        content.push(0);
    }
    content.extend_from_slice(trimmed);
    tlv(TAG_INTEGER, &content)
}

/// Encodes a BIT STRING with zero unused bits.
pub(crate) fn bit_string(data: &[u8]) -> Vec<u8> {
    let mut content = Vec::with_capacity(data.len() + 1);
    content.push(0);
    content.extend_from_slice(data);
    tlv(TAG_BIT_STRING, &content)
}

/// Forward-only DER reader over borrowed bytes.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }

    /// Reads the next TLV and returns `(tag, content)` without the header.
    pub(crate) fn next(&mut self) -> Option<(u8, &'a [u8])> {
        let header = self.buf.get(self.pos..)?;
        let tag = *header.first()?;
        if tag & 0x1f == 0x1f {
            return None; // multi-byte tags unsupported
        }
        let len_byte = *header.get(1)?;
        let (header_len, content_len) = if len_byte < 0x80 {
            (2, len_byte as usize)
        } else {
            let n = (len_byte & 0x7f) as usize;
            if n == 0 || n > 8 || header.len() < 2 + n {
                return None;
            }
            let mut len: usize = 0;
            for i in 0..n {
                len = (len << 8) | header[2 + i] as usize;
            }
            (2 + n, len)
        };
        let end = self.pos.checked_add(header_len)?.checked_add(content_len)?;
        if end > self.buf.len() {
            return None;
        }
        let content = &self.buf[self.pos + header_len..end];
        self.pos = end;
        Some((tag, content))
    }

    /// Expects the next TLV to have the given tag.
    pub(crate) fn expect(&mut self, tag: u8) -> Option<&'a [u8]> {
        let (found, content) = self.next()?;
        (found == tag).then_some(content)
    }
}

/// Encodes an ECDSA-Sig-Value: `SEQUENCE { INTEGER r, INTEGER s }` from
/// fixed-width big-endian scalars.
pub(crate) fn ecdsa_sig_der(r: &[u8], s: &[u8]) -> Vec<u8> {
    sequence(&[&positive_int(r), &positive_int(s)])
}

/// Parses an ECDSA-Sig-Value into two fixed-width big-endian scalars.
pub(crate) fn parse_ecdsa_sig_der(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut reader = Reader::new(bytes);
    let body = reader.expect(TAG_SEQUENCE)?;
    if !reader.is_empty() {
        return None;
    }
    let mut inner = Reader::new(body);
    let scalars = [inner.expect(TAG_INTEGER)?, inner.expect(TAG_INTEGER)?];
    if !inner.is_empty() {
        return None;
    }
    let mut out = [Vec::new(), Vec::new()];
    for (slot, scalar) in out.iter_mut().zip(scalars) {
        let start = scalar.iter().position(|&b| b != 0).unwrap_or(scalar.len());
        let trimmed = &scalar[start..];
        if trimmed.len() > 32 {
            return None;
        }
        slot.resize(32 - trimmed.len(), 0);
        slot.extend_from_slice(trimmed);
    }
    Some((out[0].clone(), out[1].clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_forms_round_trip() {
        let small = tlv(TAG_INTEGER, &[1]);
        assert_eq!(small, vec![0x02, 0x01, 0x01]);
        let long = vec![0xff; 300];
        let encoded = tlv(TAG_BIT_STRING, &long);
        assert_eq!(&encoded[..4], &[0x03, 0x82, 0x01, 0x2c]);
        let mut reader = Reader::new(&encoded);
        let (tag, content) = reader.next().unwrap();
        assert_eq!(tag, TAG_BIT_STRING);
        assert!(reader.is_empty());
        assert_eq!(content, &long[..]);
    }

    #[test]
    fn oid_encoding() {
        // id-ecPublicKey, prime256v1, ecdsa-with-SHA256, commonName
        assert_eq!(
            oid(&[1, 2, 840, 10045, 2, 1]),
            vec![0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]
        );
        assert_eq!(
            oid(&[1, 2, 840, 10045, 3, 1, 7]),
            vec![0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]
        );
        assert_eq!(
            oid(&[1, 2, 840, 10045, 4, 3, 2]),
            vec![0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02]
        );
        assert_eq!(oid(&[2, 5, 4, 3]), vec![0x06, 0x03, 0x55, 0x04, 0x03]);
    }

    #[test]
    fn ecdsa_signature_round_trip() {
        let r = [0x90u8; 32];
        let s = [0x01u8, 0x02, 0x03];
        let der = ecdsa_sig_der(&r, &s);
        // SEQUENCE { INTEGER(33 bytes, padded), INTEGER(3 bytes) } = 40 content bytes.
        assert_eq!(&der[..6], &[0x30, 0x28, 0x02, 0x21, 0x00, 0x90]);
        let (parsed_r, parsed_s) = parse_ecdsa_sig_der(&der).unwrap();
        assert_eq!(parsed_r, r.to_vec());
        assert_eq!(parsed_s.len(), 32);
        assert_eq!(&parsed_s[29..], &[1, 2, 3]);
        assert!(parse_ecdsa_sig_der(&[0x30, 0x00]).is_none());
        assert!(parse_ecdsa_sig_der(&der[..der.len() - 1]).is_none());
    }
}
