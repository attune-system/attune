//! Independently authenticated bounded frames for recorded process logs. Frame
//! plaintext binds execution, version and sequence to prevent stream splicing.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

pub const FRAME_BYTES: usize = 16 * 1024;
pub const ENCODING: &str = "attune_encrypted_log_v1";
pub const MAX_FINITE_PLAINTEXT: u64 = 8 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 8192;
pub use crate::runtime_log_completion::FinalLogManifest as FinalManifest;

pub fn open_final_manifest(
    key: &str,
    encrypted: &str,
    execution: i64,
    version: i64,
) -> Result<FinalManifest> {
    let manifest = FinalManifest::open(key, encrypted)?;
    if manifest.execution != execution || manifest.version != version {
        return Err(Error::PermissionDenied(
            "Protected log manifest binding mismatch".into(),
        ));
    }
    Ok(manifest)
}

/// Finite reads expose nothing until both the authenticated final manifest and
/// every frame have been checked. A valid-frame prefix is never a sealed log.
pub fn decode_complete(
    key: &str,
    encrypted_manifest: &str,
    execution: i64,
    version: i64,
    bytes: &[u8],
) -> Result<Vec<u8>> {
    let manifest = open_final_manifest(key, encrypted_manifest, execution, version)?;
    if manifest.plaintext_bytes > MAX_FINITE_PLAINTEXT
        || manifest.ciphertext_bytes > maximum_encoded_bytes(MAX_FINITE_PLAINTEXT)?
    {
        return Err(Error::validation(
            "Finite protected log exceeds its delivery bound",
        ));
    }
    let mut verifier = crate::runtime_log_completion::FrameVerifier::for_manifest(key, &manifest);
    verifier.update(bytes)?;
    verifier.finish(&manifest)?;
    let mut plaintext = Vec::with_capacity(manifest.plaintext_bytes as usize);
    let mut count = 0;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let decoded = decode(key, execution, version, count, line)?;
        count += 1;
        if plaintext
            .len()
            .checked_add(decoded.len())
            .is_none_or(|total| total as u64 > manifest.plaintext_bytes)
        {
            return Err(Error::validation("Protected log frame count mismatch"));
        }
        plaintext.extend_from_slice(&decoded);
    }
    if count != manifest.frame_count || plaintext.len() as u64 != manifest.plaintext_bytes {
        return Err(Error::validation("Protected log plaintext count mismatch"));
    }
    Ok(plaintext)
}

pub fn maximum_encoded_bytes(plaintext_cap: u64) -> Result<u64> {
    let with_notice = plaintext_cap
        .checked_add(128)
        .ok_or_else(|| Error::validation("Log byte limit overflow"))?;
    with_notice
        .div_ceil(FRAME_BYTES as u64)
        .checked_mul((FRAME_BYTES * 8) as u64)
        .ok_or_else(|| Error::validation("Log byte limit overflow"))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    execution: i64,
    version: i64,
    sequence: u64,
    bytes: Vec<u8>,
}

pub fn encode(
    key: &str,
    execution: i64,
    version: i64,
    sequence: u64,
    bytes: &[u8],
) -> Result<Vec<u8>> {
    if execution <= 0 || version <= 0 || bytes.len() > FRAME_BYTES {
        return Err(Error::validation("Invalid protected log frame"));
    }
    let plaintext = serde_json::to_string(&Frame {
        execution,
        version,
        sequence,
        bytes: bytes.to_vec(),
    })?;
    let encrypted = crate::crypto::encrypt(&plaintext, key)?;
    Ok(format!("{encrypted}\n").into_bytes())
}

/// Only complete sealed streams may be decoded. Callers independently verify
/// the stream seal/manifest and expected frame count, so truncation cannot be
/// mistaken for successful delivery.
pub fn decode(
    key: &str,
    execution: i64,
    version: i64,
    sequence: u64,
    line: &[u8],
) -> Result<Vec<u8>> {
    if line.len() > FRAME_BYTES * 8 || !line.ends_with(b"\n") {
        return Err(Error::validation("Invalid protected log frame"));
    }
    let encrypted = std::str::from_utf8(&line[..line.len() - 1])
        .map_err(|_| Error::validation("Invalid protected log frame"))?;
    let plaintext = crate::crypto::decrypt(encrypted, key)?;
    let frame: Frame = serde_json::from_str(&plaintext)?;
    if frame.execution != execution
        || frame.version != version
        || frame.sequence != sequence
        || frame.bytes.len() > FRAME_BYTES
    {
        return Err(Error::validation("Protected log frame binding mismatch"));
    }
    Ok(frame.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_preserves_binary_and_rejects_splice_reorder_and_tampering() {
        let key = "runtime-log-test-key-at-least-32-bytes";
        let bytes = [0, 255, b'\n', b's'];
        let mut encoded = encode(key, 1, 2, 0, &bytes).unwrap();
        assert_eq!(decode(key, 1, 2, 0, &encoded).unwrap(), bytes);
        assert!(decode(key, 2, 2, 0, &encoded).is_err());
        assert!(decode(key, 1, 3, 0, &encoded).is_err());
        assert!(decode(key, 1, 2, 1, &encoded).is_err());
        encoded[0] ^= 1;
        assert!(decode(key, 1, 2, 0, &encoded).is_err());
    }

    #[test]
    fn finite_decoder_requires_authenticated_complete_stream_not_a_valid_prefix() {
        let key = "finite-log-decoder-test-key-at-least-32-bytes";
        let first = encode(key, 11, 12, 0, b"first\0").unwrap();
        let last = encode(key, 11, 12, 1, &[255, 1, 2]).unwrap();
        let mut totals = crate::runtime_log_completion::FrameTotals::default();
        totals.record(&first, 6).unwrap();
        totals.record(&last, 3).unwrap();
        let manifest = totals.manifest(11, 12).unwrap();
        let proof = manifest.authenticate(key).unwrap();
        let full = [first.clone(), last.clone()].concat();
        assert_eq!(
            decode_complete(key, &proof, 11, 12, &full).unwrap(),
            [b"first\0".as_slice(), &[255, 1, 2]].concat()
        );
        assert!(decode_complete(key, &proof, 11, 12, &first).is_err());
        assert!(decode_complete(key, &proof, 11, 12, &full[..full.len() - 1]).is_err());
        assert!(
            decode_complete(key, &proof, 11, 12, &[last.clone(), first.clone()].concat()).is_err()
        );
        assert!(decode_complete(
            key,
            &proof,
            11,
            12,
            &[full.clone(), encode(key, 11, 12, 2, b"extra").unwrap()].concat()
        )
        .is_err());
        assert!(decode_complete(key, &proof, 11, 13, &full).is_err());
        let mut tampered = full.clone();
        tampered[0] ^= 1;
        assert!(decode_complete(key, &proof, 11, 12, &tampered).is_err());
        let spliced = [
            first.clone(),
            encode(key, 99, 12, 1, b"other source").unwrap(),
        ]
        .concat();
        assert!(decode_complete(key, &proof, 11, 12, &spliced).is_err());
        let mut forged = manifest.clone();
        forged.frame_count += 1;
        assert!(decode_complete(key, &forged.authenticate(key).unwrap(), 11, 12, &full).is_err());
        forged = manifest.clone();
        forged.ciphertext_bytes += 1;
        assert!(decode_complete(key, &forged.authenticate(key).unwrap(), 11, 12, &full).is_err());
        forged = manifest.clone();
        forged.plaintext_bytes += 1;
        assert!(decode_complete(key, &forged.authenticate(key).unwrap(), 11, 12, &full).is_err());
        forged.plaintext_bytes = MAX_FINITE_PLAINTEXT + 1;
        assert!(decode_complete(key, &forged.authenticate(key).unwrap(), 11, 12, &full).is_err());
        let empty = crate::runtime_log_completion::FrameTotals::default()
            .manifest(11, 12)
            .unwrap()
            .authenticate(key)
            .unwrap();
        assert!(decode_complete(key, &empty, 11, 12, &[])
            .unwrap()
            .is_empty());
    }
}
