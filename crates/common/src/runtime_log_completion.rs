//! Authenticated, complete encrypted-log manifests. A valid frame prefix is not
//! proof of completion. Finite reads verify before disclosure; streaming readers
//! may disclose authenticated frames but must verify this proof before successful EOF.
use crate::{runtime_read_log, Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalLogManifest {
    pub encoding: String,
    pub execution: i64,
    pub version: i64,
    pub frame_count: u64,
    pub plaintext_bytes: u64,
    pub ciphertext_bytes: u64,
    pub ciphertext_sha256: String,
}

fn invalid() -> Error {
    Error::PermissionDenied("Encrypted log completion proof is invalid or incomplete".into())
}

#[derive(Default)]
pub struct FrameTotals {
    frames: u64,
    plain: u64,
    cipher: u64,
    digest: Sha256,
}

impl FrameTotals {
    pub fn record(&mut self, ciphertext_frame: &[u8], plaintext_bytes: usize) -> Result<()> {
        self.frames = self.frames.checked_add(1).ok_or_else(invalid)?;
        self.plain = self
            .plain
            .checked_add(plaintext_bytes as u64)
            .ok_or_else(invalid)?;
        self.cipher = self
            .cipher
            .checked_add(ciphertext_frame.len() as u64)
            .ok_or_else(invalid)?;
        self.digest.update(ciphertext_frame);
        Ok(())
    }

    pub fn manifest(self, execution: i64, version: i64) -> Result<FinalLogManifest> {
        let manifest = FinalLogManifest {
            encoding: runtime_read_log::ENCODING.into(),
            execution,
            version,
            frame_count: self.frames,
            plaintext_bytes: self.plain,
            ciphertext_bytes: self.cipher,
            ciphertext_sha256: self
                .digest
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        };
        manifest.validate()?;
        Ok(manifest)
    }
}

impl FinalLogManifest {
    fn validate(&self) -> Result<()> {
        if self.encoding != runtime_read_log::ENCODING
            || self.execution <= 0
            || self.version <= 0
            || [
                self.frame_count,
                self.plaintext_bytes,
                self.ciphertext_bytes,
            ]
            .iter()
            .any(|n| *n > i64::MAX as u64)
            || self.ciphertext_sha256.len() != 64
            || !self
                .ciphertext_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn authenticate(&self, key: &str) -> Result<String> {
        self.validate()?;
        crate::crypto::encrypt(&serde_json::to_string(self)?, key)
    }

    pub fn open(key: &str, authenticated: &str) -> Result<Self> {
        if authenticated.len() > 8192 {
            return Err(invalid());
        }
        let plaintext = crate::crypto::decrypt(authenticated, key).map_err(|_| invalid())?;
        let manifest: Self = serde_json::from_str(&plaintext).map_err(|_| invalid())?;
        manifest.validate()?;
        Ok(manifest)
    }
}

pub struct FrameVerifier<'a> {
    key: &'a str,
    execution: i64,
    version: i64,
    max_plain: u64,
    max_cipher: u64,
    max_frames: u64,
    received: u64,
    pending: Vec<u8>,
    totals: FrameTotals,
}

impl<'a> FrameVerifier<'a> {
    pub fn new(
        key: &'a str,
        execution: i64,
        version: i64,
        max_plain: u64,
        max_cipher: u64,
    ) -> Self {
        Self {
            key,
            execution,
            version,
            max_plain,
            max_cipher,
            max_frames: max_plain.div_ceil(runtime_read_log::FRAME_BYTES as u64),
            received: 0,
            pending: Vec::new(),
            totals: FrameTotals::default(),
        }
    }

    /// Exact independent bounds from an authenticated final manifest. Callers
    /// still enforce their finite/operator limits before constructing a reader.
    pub fn for_manifest(key: &'a str, manifest: &FinalLogManifest) -> Self {
        Self {
            max_frames: manifest.frame_count,
            ..Self::new(
                key,
                manifest.execution,
                manifest.version,
                manifest.plaintext_bytes,
                manifest.ciphertext_bytes,
            )
        }
    }

    pub fn update(&mut self, mut bytes: &[u8]) -> Result<()> {
        self.received = self
            .received
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= self.max_cipher)
            .ok_or_else(invalid)?;
        while !bytes.is_empty() {
            let count = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |n| n + 1);
            if self
                .pending
                .len()
                .checked_add(count)
                .is_none_or(|n| n > runtime_read_log::FRAME_BYTES * 8)
            {
                return Err(invalid());
            }
            self.pending.extend_from_slice(&bytes[..count]);
            bytes = &bytes[count..];
            if self.pending.last() == Some(&b'\n') {
                let plaintext = runtime_read_log::decode(
                    self.key,
                    self.execution,
                    self.version,
                    self.totals.frames,
                    &self.pending,
                )?;
                self.totals.record(&self.pending, plaintext.len())?;
                if self.totals.plain > self.max_plain || self.totals.frames > self.max_frames {
                    return Err(invalid());
                }
                self.pending.clear();
            }
        }
        Ok(())
    }

    pub fn finish(self, expected: &FinalLogManifest) -> Result<()> {
        if !self.pending.is_empty() {
            return Err(invalid());
        }
        let actual = self.totals.manifest(self.execution, self.version)?;
        if actual != *expected {
            return Err(invalid());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_plaintext_nonzero_empty_frames_require_exact_manifest() {
        let key = "empty-frame-manifest-key-at-least-32-bytes";
        let zero = FrameTotals::default().manifest(1, 2).unwrap();
        for frame_count in [1, 3] {
            let mut totals = FrameTotals::default();
            let mut ciphertext = Vec::new();
            for sequence in 0..frame_count {
                let frame = runtime_read_log::encode(key, 1, 2, sequence, b"").unwrap();
                totals.record(&frame, 0).unwrap();
                ciphertext.extend(frame);
            }
            let manifest = totals.manifest(1, 2).unwrap();
            let proof = manifest.authenticate(key).unwrap();
            let opened = FinalLogManifest::open(key, &proof).unwrap();
            assert_eq!(opened.frame_count, frame_count);
            assert_eq!(opened.plaintext_bytes, 0);
            assert_eq!(opened.ciphertext_bytes, ciphertext.len() as u64);
            assert!(opened.ciphertext_bytes > 0);
            assert_ne!(opened.ciphertext_sha256, zero.ciphertext_sha256);
            let mut verifier = FrameVerifier::for_manifest(key, &opened);
            for chunk in ciphertext.chunks(7) {
                verifier.update(chunk).unwrap();
            }
            verifier.finish(&opened).unwrap();
            assert!(
                runtime_read_log::decode_complete(key, &proof, 1, 2, &ciphertext)
                    .unwrap()
                    .is_empty()
            );

            let mut wrong_hash = opened.clone();
            wrong_hash.ciphertext_sha256.replace_range(
                ..1,
                if opened.ciphertext_sha256.starts_with('0') {
                    "1"
                } else {
                    "0"
                },
            );
            for inconsistent in [
                FinalLogManifest {
                    frame_count: frame_count - 1,
                    ..opened.clone()
                },
                FinalLogManifest {
                    plaintext_bytes: 1,
                    ..opened.clone()
                },
                FinalLogManifest {
                    ciphertext_bytes: opened.ciphertext_bytes - 1,
                    ..opened.clone()
                },
                FinalLogManifest {
                    ciphertext_bytes: opened.ciphertext_bytes + 1,
                    ..opened.clone()
                },
                wrong_hash,
            ] {
                let mut bounded = FrameVerifier::for_manifest(key, &inconsistent);
                let updated = bounded.update(&ciphertext);
                assert!(updated
                    .and_then(|()| bounded.finish(&inconsistent))
                    .is_err());
                let inconsistent_proof = inconsistent.authenticate(key).unwrap();
                assert!(runtime_read_log::decode_complete(
                    key,
                    &inconsistent_proof,
                    1,
                    2,
                    &ciphertext,
                )
                .is_err());
            }

            let mut empty = FrameVerifier::for_manifest(key, &zero);
            assert!(empty.update(&ciphertext).is_err());
            assert!(runtime_read_log::decode_complete(
                key,
                &zero.authenticate(key).unwrap(),
                1,
                2,
                &ciphertext,
            )
            .is_err());
        }
        FrameVerifier::for_manifest(key, &zero)
            .finish(&zero)
            .unwrap();
        assert!(runtime_read_log::decode_complete(
            key,
            &zero.authenticate(key).unwrap(),
            1,
            2,
            &[],
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn sealed_short_and_empty_frames_keep_independent_exact_limits() {
        let key = "short-frame-manifest-key-at-least-32-bytes";
        let mut totals = FrameTotals::default();
        let mut ciphertext = Vec::new();
        for (sequence, plain) in [b"first".as_slice(), b"last".as_slice(), b"".as_slice()]
            .iter()
            .enumerate()
        {
            let frame = runtime_read_log::encode(key, 1, 2, sequence as u64, plain).unwrap();
            totals.record(&frame, plain.len()).unwrap();
            ciphertext.extend(frame);
        }
        let manifest = totals.manifest(1, 2).unwrap();
        let mut valid = FrameVerifier::for_manifest(key, &manifest);
        for part in ciphertext.chunks(7) {
            valid.update(part).unwrap();
        }
        valid.finish(&manifest).unwrap();
        let proof = manifest.authenticate(key).unwrap();
        assert_eq!(
            runtime_read_log::decode_complete(key, &proof, 1, 2, &ciphertext).unwrap(),
            b"firstlast"
        );
        for restricted in [
            FinalLogManifest {
                frame_count: 2,
                ..manifest.clone()
            },
            FinalLogManifest {
                plaintext_bytes: 8,
                ..manifest.clone()
            },
            FinalLogManifest {
                ciphertext_bytes: manifest.ciphertext_bytes - 1,
                ..manifest.clone()
            },
        ] {
            let mut bounded = FrameVerifier::for_manifest(key, &restricted);
            assert!(bounded.update(&ciphertext).is_err());
        }
        let mut truncated = FrameVerifier::for_manifest(key, &manifest);
        truncated
            .update(&ciphertext[..ciphertext.len() - 1])
            .unwrap();
        assert!(truncated.finish(&manifest).is_err());
    }
    #[test]
    fn authenticated_final_manifest_rejects_valid_prefix_splice_and_partial_frame() {
        let key = "completion-test-key-at-least-32-bytes";
        let first = runtime_read_log::encode(key, 1, 2, 0, b"first").unwrap();
        let last = runtime_read_log::encode(key, 1, 2, 1, b"last").unwrap();
        let mut totals = FrameTotals::default();
        totals.record(&first, 5).unwrap();
        totals.record(&last, 4).unwrap();
        let manifest = totals.manifest(1, 2).unwrap();
        let proof = manifest.authenticate(key).unwrap();
        let opened = FinalLogManifest::open(key, &proof).unwrap();
        assert!(opened == manifest);
        let mut full = FrameVerifier::new(key, 1, 2, 32768, 262144);
        for chunk in first.chunks(3).chain(last.chunks(7)) {
            full.update(chunk).unwrap();
        }
        full.finish(&opened).unwrap();
        let mut prefix = FrameVerifier::new(key, 1, 2, 32768, 262144);
        prefix.update(&first).unwrap();
        assert!(prefix.finish(&opened).is_err());
        let mut partial = FrameVerifier::new(key, 1, 2, 32768, 262144);
        partial.update(&first[..first.len() - 1]).unwrap();
        assert!(partial.finish(&opened).is_err());
        let mut wrong = FrameVerifier::new(key, 1, 3, 32768, 262144);
        assert!(wrong.update(&first).is_err());
        assert!(FinalLogManifest::open("other-completion-key-at-least-32-bytes", &proof).is_err());
    }
    #[test]
    fn empty_final_manifest_and_encoded_expansion_are_bounded() {
        let key = "completion-test-key-at-least-32-bytes";
        let empty = FrameTotals::default().manifest(1, 2).unwrap();
        FrameVerifier::new(
            key,
            1,
            2,
            128,
            runtime_read_log::maximum_encoded_bytes(0).unwrap(),
        )
        .finish(&empty)
        .unwrap();
        let encoded = runtime_read_log::encode(key, 1, 2, 0, &[255; 16]).unwrap();
        assert!(encoded.len() > 16);
        let mut bounded = FrameVerifier::new(
            key,
            1,
            2,
            144,
            runtime_read_log::maximum_encoded_bytes(16).unwrap(),
        );
        bounded.update(&encoded).unwrap();
        let mut short = FrameVerifier::new(key, 1, 2, 15, 262144);
        assert!(short.update(&encoded).is_err());
    }
}
