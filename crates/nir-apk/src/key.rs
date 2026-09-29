//! APK signing identity: a P-256 ECDSA keypair with a minimal self-signed
//! X.509 v3 certificate, generated in-process so novel authors need no
//! Java toolchain to produce a key.
//!
//! # Persistence format
//! [`SigningKey::to_pem`] emits two adjacent PEM blocks in one document:
//!
//! ```text
//! -----BEGIN NIR APK SIGNING KEY-----
//! <base64 of the 32-byte big-endian private scalar>
//! -----END NIR APK SIGNING KEY-----
//! -----BEGIN CERTIFICATE-----
//! <base64 of the DER certificate>
//! -----END CERTIFICATE-----
//! ```
//!
//! Loading re-derives the certificate's SubjectPublicKeyInfo and rejects a
//! document whose key and certificate do not match.
#![forbid(unsafe_code)]

use anyhow::{bail, ensure, Context, Result};
use p256::ecdsa::{signature::Signer, Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

use crate::der;

const KEY_BLOCK: &str = "NIR APK SIGNING KEY";
const CERT_BLOCK: &str = "CERTIFICATE";
/// Fixed validity window (UTC): 2026-01-01 through 2046-01-01.
const NOT_BEFORE: &str = "260101000000Z";
const NOT_AFTER: &str = "460101000000Z";
const SUBJECT_CN: &str = "nir";

/// id-ecPublicKey, prime256v1, ecdsa-with-SHA256, commonName.
const OID_EC_PUBLIC_KEY: &[u64] = &[1, 2, 840, 10045, 2, 1];
const OID_PRIME256V1: &[u64] = &[1, 2, 840, 10045, 3, 1, 7];
const OID_ECDSA_WITH_SHA256: &[u64] = &[1, 2, 840, 10045, 4, 3, 2];
const OID_COMMON_NAME: &[u64] = &[2, 5, 4, 3];

/// A P-256 signing identity for APK Signature Scheme v2 (algorithm 0x0201,
/// ECDSA with SHA-256).
pub struct SigningIdentity {
    key: SigningKey,
    certificate: Vec<u8>,
}

impl SigningIdentity {
    /// Generates a fresh identity from OS entropy.
    pub fn generate() -> Result<Self> {
        use p256::ecdsa::signature::rand_core::{OsRng, RngCore};
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        Self::from_seed(&seed)
    }

    /// Derives an identity from 32 bytes of key material (hashed first so any
    /// entropy quality is acceptable for tests and reproducible builds).
    pub fn from_seed(seed: &[u8]) -> Result<Self> {
        ensure!(seed.len() == 32, "E_SIGNING_SEED: expected 32 bytes");
        let scalar = Sha256::digest(seed);
        let key = SigningKey::from_bytes(&scalar)
            .map_err(|_| anyhow::anyhow!("E_SIGNING_SEED: not a valid P-256 scalar"))?;
        let certificate = build_certificate(&key);
        Ok(Self { key, certificate })
    }

    /// Derives an identity from an arbitrary passphrase, hashed to the 32
    /// seed bytes [`from_seed`](Self::from_seed) requires, so any printable
    /// string can pin a reproducible key.
    pub fn from_seed_phrase(phrase: &str) -> Result<Self> {
        Self::from_seed(&Sha256::digest(phrase.as_bytes()))
    }

    /// Reconstructs an identity from its persisted PEM document.
    pub fn from_pem(pem: &str) -> Result<Self> {
        let mut scalar = None;
        let mut certificate = None;
        let mut block = None;
        let mut buffer = String::new();
        for line in pem.lines() {
            let line = line.trim();
            if let Some(label) = line.strip_prefix("-----BEGIN ") {
                let label = label
                    .strip_suffix("-----")
                    .context("E_SIGNING_PEM: malformed begin line")?;
                ensure!(block.is_none(), "E_SIGNING_PEM: nested block");
                block = Some(label.to_owned());
                buffer.clear();
            } else if let Some(label) = line.strip_prefix("-----END ") {
                let label = label
                    .strip_suffix("-----")
                    .context("E_SIGNING_PEM: malformed end line")?;
                let current = block.take().context("E_SIGNING_PEM: stray end line")?;
                ensure!(current == label, "E_SIGNING_PEM: block label mismatch");
                let decoded = base64_decode(buffer.trim())
                    .with_context(|| format!("E_SIGNING_PEM: bad base64 in {label} block"))?;
                match label {
                    KEY_BLOCK => scalar = Some(decoded),
                    CERT_BLOCK => certificate = Some(decoded),
                    _ => bail!("E_SIGNING_PEM: unexpected block {label}"),
                }
            } else if block.is_some() {
                buffer.push_str(line);
            } else {
                ensure!(line.is_empty(), "E_SIGNING_PEM: stray content");
            }
        }
        ensure!(block.is_none(), "E_SIGNING_PEM: unterminated block");
        let scalar = scalar.context("E_SIGNING_PEM: missing NIR APK SIGNING KEY block")?;
        ensure!(scalar.len() == 32, "E_SIGNING_PEM: bad scalar length");
        let certificate = certificate.context("E_SIGNING_PEM: missing CERTIFICATE block")?;
        let bytes: [u8; 32] = scalar
            .as_slice()
            .try_into()
            .context("E_SIGNING_PEM: bad scalar length")?;
        let key = SigningKey::from_bytes(&p256::FieldBytes::from(bytes))
            .map_err(|_| anyhow::anyhow!("E_SIGNING_PEM: not a valid P-256 scalar"))?;
        let identity = Self { key, certificate };
        ensure!(
            identity.certificate_subject_spki() == Some(identity.public_key_der()),
            "E_SIGNING_PEM: certificate does not match private key"
        );
        Ok(identity)
    }

    /// Serializes the identity as a PEM document (see module docs).
    pub fn to_pem(&self) -> String {
        let mut out = String::new();
        out.push_str(&pem_block(KEY_BLOCK, &self.key.to_bytes()));
        out.push_str(&pem_block(CERT_BLOCK, &self.certificate));
        out
    }

    /// Writes the identity to `path` (plain UTF-8 PEM document).
    pub fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, self.to_pem()).context("E_SIGNING_WRITE")
    }

    /// Loads an identity previously written by [`SigningIdentity::save`].
    pub fn load(path: &Path) -> Result<Self> {
        let pem = fs::read_to_string(path).context("E_SIGNING_READ")?;
        Self::from_pem(&pem)
    }

    /// The DER X.509 v3 certificate carrying this key.
    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate
    }

    /// The DER SubjectPublicKeyInfo for this key.
    pub fn public_key_der(&self) -> Vec<u8> {
        spki_der(self.key.verifying_key())
    }

    /// Signs `message` with ECDSA/SHA-256 and returns the DER ECDSA-Sig-Value.
    pub(crate) fn sign_der(&self, message: &[u8]) -> Vec<u8> {
        let signature: Signature = self.key.sign(message);
        let bytes = signature.to_bytes();
        der::ecdsa_sig_der(&bytes[..32], &bytes[32..])
    }

    /// Verifies a DER ECDSA-Sig-Value over `message` with the given
    /// SubjectPublicKeyInfo.
    pub(crate) fn verify_der(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
        let (r, s) = match der::parse_ecdsa_sig_der(signature) {
            Some(parts) => parts,
            None => return false,
        };
        let point = match extract_spki_point(public_key) {
            Some(point) => point,
            None => return false,
        };
        let Ok(key) = VerifyingKey::from_sec1_bytes(&point) else {
            return false;
        };
        let Ok(signature) = Signature::from_slice(&[r, s].concat()) else {
            return false;
        };
        use p256::ecdsa::signature::Verifier;
        key.verify(message, &signature).is_ok()
    }

    fn certificate_subject_spki(&self) -> Option<Vec<u8>> {
        certificate_spki(&self.certificate).ok()
    }
}

/// Extracts the SubjectPublicKeyInfo (full DER TLV) from a certificate.
pub(crate) fn certificate_spki(certificate: &[u8]) -> Result<Vec<u8>> {
    let mut cert = der::Reader::new(certificate);
    let body = cert
        .expect(der::TAG_SEQUENCE)
        .context("E_CERTIFICATE: not a DER sequence")?;
    ensure!(cert.is_empty(), "E_CERTIFICATE: trailing data");
    let mut outer = der::Reader::new(body);
    let mut tbs = der::Reader::new(
        outer
            .expect(der::TAG_SEQUENCE)
            .context("E_CERTIFICATE: not a TBS sequence")?,
    );
    ensure!(
        tbs.next().map(|(tag, _)| tag) == Some(0xa0),
        "E_CERTIFICATE: missing v3 version"
    );
    tbs.next().context("E_CERTIFICATE: serial")?; // serialNumber
    tbs.next().context("E_CERTIFICATE: algorithm")?; // signature AlgorithmIdentifier
    tbs.next().context("E_CERTIFICATE: issuer")?; // issuer
    tbs.next().context("E_CERTIFICATE: validity")?; // validity
    tbs.next().context("E_CERTIFICATE: subject")?; // subject
    let (tag, spki) = tbs.next().context("E_CERTIFICATE: SubjectPublicKeyInfo")?;
    ensure!(tag == der::TAG_SEQUENCE, "E_CERTIFICATE: bad SPKI");
    ensure!(tbs.is_empty(), "E_CERTIFICATE: unexpected TBS fields");
    Ok(der::tlv(der::TAG_SEQUENCE, spki))
}

fn pem_block(label: &str, data: &[u8]) -> String {
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in base64_encode(data).as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn inverse(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a') as u32 + 26),
            b'0'..=b'9' => Some((b - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = text.bytes().filter(|&b| b != b'\r' && b != b'\n').collect();
    let pad = bytes.iter().filter(|&&b| b == b'=').count();
    if pad > 2 || !bytes.len().is_multiple_of(4) || bytes[..bytes.len() - pad].contains(&b'=') {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let mut n: u32 = 0;
        for (i, &b) in chunk.iter().enumerate() {
            let digit = if b == b'=' { 0 } else { inverse(b)? };
            n |= digit << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk[2] != b'=' {
            out.push((n >> 8) as u8);
        }
        if chunk[3] != b'=' {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn spki_der(public_key: &VerifyingKey) -> Vec<u8> {
    der::sequence(&[
        &der::sequence(&[&der::oid(OID_EC_PUBLIC_KEY), &der::oid(OID_PRIME256V1)]),
        &der::bit_string(&public_key.to_sec1_bytes()),
    ])
}

/// Extracts the EC point from a DER SubjectPublicKeyInfo.
fn extract_spki_point(spki: &[u8]) -> Option<Vec<u8>> {
    let mut reader = der::Reader::new(spki);
    let body = reader.expect(der::TAG_SEQUENCE)?;
    if !reader.is_empty() {
        return None;
    }
    let mut inner = der::Reader::new(body);
    inner.expect(der::TAG_SEQUENCE)?; // algorithm identifier
    let (_, bit_string) = inner.next()?;
    let valid = bit_string.first() == Some(&0) // zero unused bits
        && bit_string.get(1) == Some(&0x04) // uncompressed point
        && bit_string.len() == 66;
    valid.then(|| bit_string[1..].to_vec())
}

/// Hand-builds the DER for a minimal self-signed certificate:
///
/// ```text
/// Certificate ::= SEQUENCE {
///   tbsCertificate ::= SEQUENCE {
///     [0] EXPLICIT INTEGER 2,            -- v3
///     INTEGER serialNumber,              -- derived from the public key
///     SEQUENCE { OID ecdsa-with-SHA256 },
///     Name CN=nir (issuer and subject),
///     SEQUENCE { UTCTime notBefore, UTCTime notAfter },
///     SubjectPublicKeyInfo },
///   SEQUENCE { OID ecdsa-with-SHA256 },
///   BIT STRING signature }
/// ```
fn build_certificate(key: &SigningKey) -> Vec<u8> {
    let public_key = key.verifying_key();
    let digest = Sha256::digest(public_key.to_sec1_bytes());
    let serial = der::positive_int(&digest[..8]);
    // Name ::= SEQUENCE OF RelativeDistinguishedName; RDN ::= SET OF
    // AttributeTypeAndValue.
    let name = der::sequence(&[&der::tlv(
        der::TAG_SET,
        &der::sequence(&[
            &der::oid(OID_COMMON_NAME),
            &der::tlv(der::TAG_PRINTABLE_STRING, SUBJECT_CN.as_bytes()),
        ]),
    )]);
    let algorithm = der::sequence(&[&der::oid(OID_ECDSA_WITH_SHA256)]);
    let validity = der::sequence(&[
        &der::tlv(der::TAG_UTC_TIME, NOT_BEFORE.as_bytes()),
        &der::tlv(der::TAG_UTC_TIME, NOT_AFTER.as_bytes()),
    ]);
    let tbs = der::sequence(&[
        &der::tlv(0xa0, &der::uint(2)),
        &serial,
        &algorithm,
        &name,
        &validity,
        &name,
        &spki_der(public_key),
    ]);
    let signature: Signature = key.sign(&tbs);
    let bytes = signature.to_bytes();
    let signature_der = der::ecdsa_sig_der(&bytes[..32], &bytes[32..]);
    der::sequence(&[&tbs, &algorithm, &der::bit_string(&signature_der)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity() -> SigningIdentity {
        SigningIdentity::from_seed(&[7u8; 32]).unwrap()
    }

    #[test]
    fn pem_round_trip() {
        let identity = test_identity();
        let pem = identity.to_pem();
        assert!(pem.contains("-----BEGIN NIR APK SIGNING KEY-----"));
        assert!(pem.contains("-----BEGIN CERTIFICATE-----"));
        let loaded = SigningIdentity::from_pem(&pem).unwrap();
        assert_eq!(loaded.certificate_der(), identity.certificate_der());
        let path = std::env::temp_dir().join("nir-apk-key-test.pem");
        SigningIdentity::save(&loaded, &path).unwrap();
        let reloaded = SigningIdentity::load(&path).unwrap();
        assert_eq!(reloaded.to_pem(), pem);
    }

    #[test]
    fn pem_rejects_key_certificate_mismatch() {
        let identity = test_identity();
        let _other = SigningIdentity::from_seed(&[9u8; 32]).unwrap();
        let mismatched = format!(
            "{}{}",
            pem_block(KEY_BLOCK, &[9u8; 32]),
            pem_block(CERT_BLOCK, identity.certificate_der())
        );
        assert!(SigningIdentity::from_pem(&mismatched).is_err());
    }

    #[test]
    fn pem_rejects_garbage() {
        assert!(SigningIdentity::from_pem("").is_err());
        assert!(SigningIdentity::from_pem("hello").is_err());
        let identity = test_identity();
        let pem = identity
            .to_pem()
            .replace("CERTIFICATE", "WHATEVER")
            .replace(KEY_BLOCK, "WHATEVER");
        assert!(SigningIdentity::from_pem(&pem).is_err());
        assert!(SigningIdentity::from_pem(
            &identity
                .to_pem()
                .replace("-----END NIR APK SIGNING KEY-----", "")
        )
        .is_err());
    }

    #[test]
    fn sign_verify_round_trip() {
        let identity = test_identity();
        let signature = identity.sign_der(b"nir message");
        assert!(SigningIdentity::verify_der(
            &identity.public_key_der(),
            b"nir message",
            &signature
        ));
        assert!(!SigningIdentity::verify_der(
            &identity.public_key_der(),
            b"tampered message",
            &signature
        ));
        let mut broken = signature.clone();
        let last = broken.len() - 1;
        broken[last] ^= 1;
        assert!(!SigningIdentity::verify_der(
            &identity.public_key_der(),
            b"nir message",
            &broken
        ));
    }

    #[test]
    fn certificate_is_self_signed_v3() {
        let identity = test_identity();
        let mut cert = der::Reader::new(identity.certificate_der());
        let body = cert.expect(der::TAG_SEQUENCE).unwrap();
        assert!(cert.is_empty());
        let mut outer = der::Reader::new(body);
        let tbs = outer.expect(der::TAG_SEQUENCE).unwrap().to_vec();
        assert_eq!(outer.expect(der::TAG_SEQUENCE).unwrap().len(), 10); // SEQ{OID ecdsa-with-SHA256}
        let (_, bit_string) = outer.next().unwrap();
        assert!(outer.is_empty());
        let signature = &bit_string[1..]; // strip unused-bits octet

        let mut fields = der::Reader::new(&tbs);
        let (version_tag, version) = fields.next().unwrap();
        assert_eq!(version_tag, 0xa0);
        assert_eq!(version, &[0x02, 0x01, 0x02]); // EXPLICIT INTEGER 2 (v3)
        assert_eq!(fields.next().unwrap().0, der::TAG_INTEGER); // serial
        assert_eq!(fields.next().unwrap().0, der::TAG_SEQUENCE); // signature alg
        assert_eq!(fields.next().unwrap().0, der::TAG_SEQUENCE); // issuer
        assert_eq!(fields.next().unwrap().0, der::TAG_SEQUENCE); // validity
        let issuer = {
            let mut replay = der::Reader::new(&tbs);
            replay.next().unwrap();
            replay.next().unwrap();
            replay.next().unwrap();
            replay.next().unwrap().1.to_vec()
        };
        let (_, subject) = fields.next().unwrap();
        assert_eq!(issuer, subject); // self-signed
        let (_, spki) = fields.next().unwrap();
        assert_eq!(spki, &identity.public_key_der()[2..]);
        assert!(fields.is_empty());
        let spki_der = der::tlv(der::TAG_SEQUENCE, spki);
        let tbs_der = der::tlv(der::TAG_SEQUENCE, &tbs);
        assert!(SigningIdentity::verify_der(&spki_der, &tbs_der, signature));
    }

    #[test]
    fn base64_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        for input in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foobar",
            &[0u8, 255, 10, 128],
        ] {
            let encoded = base64_encode(input);
            assert_eq!(base64_decode(&encoded).unwrap(), input);
        }
        assert!(base64_decode("!!!!").is_none());
        assert!(base64_decode("Zm9vY").is_none());
    }

    #[test]
    fn seed_determinism() {
        assert_eq!(
            test_identity().to_pem(),
            SigningIdentity::from_seed(&[7u8; 32]).unwrap().to_pem()
        );
        assert_ne!(
            test_identity().certificate_der(),
            SigningIdentity::from_seed(&[8u8; 32])
                .unwrap()
                .certificate_der()
        );
        assert!(SigningIdentity::from_seed(&[7u8; 31]).is_err());
    }

    #[test]
    fn seed_phrase_determinism() {
        assert_eq!(
            SigningIdentity::from_seed_phrase("sample")
                .unwrap()
                .to_pem(),
            SigningIdentity::from_seed_phrase("sample")
                .unwrap()
                .to_pem()
        );
        assert_ne!(
            SigningIdentity::from_seed_phrase("sample")
                .unwrap()
                .certificate_der(),
            SigningIdentity::from_seed_phrase("other")
                .unwrap()
                .certificate_der()
        );
    }
}
