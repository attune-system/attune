//! Whole-file authenticated encryption. No unauthenticated prefix is disclosed.
use crate::{crypto, Error, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ENCODING: &str = "attune_encrypted_file_v1";

/// Conservative live-buffer allowance for the unchanged v1 base64/JSON/GCM
/// codec, SQLx retry-body delivery and plaintext response ownership. This is
/// admission accounting, not a new maximum file size or an authorization grant.
pub fn allocation_budget(plaintext: u64) -> Option<u64> {
    plaintext.checked_mul(12)?.checked_add(16 * 1024 * 1024)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileBodyManifest {
    pub encoding: String,
    pub version: i64,
    pub origin_read: i64,
    pub plaintext_bytes: u64,
    pub plaintext_sha256: String,
    pub ciphertext_bytes: u64,
    pub ciphertext_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    encoding: String,
    version: i64,
    origin_read: i64,
    body: String,
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn seal(
    bytes: &[u8],
    version: i64,
    origin_read: i64,
    key: &str,
) -> Result<(Vec<u8>, FileBodyManifest, String)> {
    let body = crypto::encrypt(
        &serde_json::to_string(&Envelope {
            encoding: ENCODING.into(),
            version,
            origin_read,
            body: STANDARD.encode(bytes),
        })?,
        key,
    )?
    .into_bytes();
    let manifest = FileBodyManifest {
        encoding: ENCODING.into(),
        version,
        origin_read,
        plaintext_bytes: bytes.len() as u64,
        plaintext_sha256: sha256(bytes),
        ciphertext_bytes: body.len() as u64,
        ciphertext_sha256: sha256(&body),
    };
    let authenticated = crypto::encrypt(&serde_json::to_string(&manifest)?, key)?;
    Ok((body, manifest, authenticated))
}

pub fn open(
    body: &[u8],
    manifest: &FileBodyManifest,
    authenticated: &str,
    key: &str,
) -> Result<Vec<u8>> {
    let invalid = || Error::PermissionDenied("Encrypted file body proof is invalid".into());
    let retained: FileBodyManifest = serde_json::from_str(&crypto::decrypt(authenticated, key)?)?;
    if &retained != manifest
        || manifest.encoding != ENCODING
        || manifest.version <= 0
        || manifest.origin_read <= 0
        || manifest.ciphertext_bytes != body.len() as u64
        || manifest.ciphertext_sha256 != sha256(body)
    {
        return Err(invalid());
    }
    let envelope: Envelope = serde_json::from_str(&crypto::decrypt(
        std::str::from_utf8(body).map_err(|_| invalid())?,
        key,
    )?)?;
    if envelope.encoding != ENCODING
        || envelope.version != manifest.version
        || envelope.origin_read != manifest.origin_read
    {
        return Err(invalid());
    }
    let bytes = STANDARD.decode(envelope.body).map_err(|_| invalid())?;
    if bytes.len() as u64 != manifest.plaintext_bytes || sha256(&bytes) != manifest.plaintext_sha256
    {
        return Err(invalid());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_file_proof_rejects_prefix_substitution_and_wrong_version() {
        let key = "a".repeat(64);
        let (body, manifest, proof) = seal(b"private\0binary", 1, 2, &key).unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("private"));
        assert_eq!(
            open(&body, &manifest, &proof, &key).unwrap(),
            b"private\0binary"
        );
        assert!(open(&body[..body.len() - 1], &manifest, &proof, &key).is_err());
        let mut wrong = manifest.clone();
        wrong.version = 3;
        assert!(open(&body, &wrong, &proof, &key).is_err());
    }
}
