//! Full accepted-byte verification. This module deliberately has no database or
//! application-state input. Its result proves bytes, never current authority.
use crate::blob_store::{BlobStore, BlobStoreError, ObjectKey, ProviderVersion};
use crate::models::log_stream::{LogSegment, LogStream};
use crate::runtime_log_completion::{FinalLogManifest, FrameVerifier};
use futures::StreamExt;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum VerificationError {
    #[error("Invalid accepted log frontier")]
    Frontier,
    #[error("Encrypted log final bytes do not match the authenticated manifest")]
    Manifest,
    #[error(transparent)]
    Storage(#[from] BlobStoreError),
    #[error(transparent)]
    Encrypted(#[from] crate::Error),
}

/// Crypto inputs only; no permit, mutable source proof or connection is retained.
pub struct EncryptedInput<'a> {
    pub key: &'a str,
    pub manifest: &'a FinalLogManifest,
    pub plaintext_cap: u64,
    pub ciphertext_cap: u64,
}

/// Constructible only by consuming every accepted provider reader to successful
/// EOF and, for encrypted streams, validating the complete authenticated manifest.
/// This is not a publication permit. Publication must freshly authorize its owner.
#[derive(Debug)]
pub struct VerifiedBytes {
    size: i64,
    sha256: String,
}
impl VerifiedBytes {
    pub fn size(&self) -> i64 {
        self.size
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

pub async fn verify(
    storage: &dyn BlobStore,
    stream: &LogStream,
    segments: &[LogSegment],
    encrypted: Option<EncryptedInput<'_>>,
) -> Result<VerifiedBytes, VerificationError> {
    let mut frontier = 0_i64;
    if stream.backend != crate::models::enums::LogStreamBackend::ObjectSegments
        || stream.next_sequence
            != i64::try_from(segments.len()).map_err(|_| VerificationError::Frontier)?
    {
        return Err(VerificationError::Frontier);
    }
    // Validate the complete frontier before issuing any provider read.
    for (sequence, segment) in segments.iter().enumerate() {
        if segment.stream != stream.id
            || segment.sequence != sequence as i64
            || segment.size_bytes < 0
            || segment.byte_start != frontier
            || frontier.checked_add(segment.size_bytes) != Some(segment.byte_end)
        {
            return Err(VerificationError::Frontier);
        }
        frontier = segment.byte_end;
    }
    if frontier != stream.total_bytes {
        return Err(VerificationError::Frontier);
    }
    if encrypted.as_ref().is_some_and(|input| {
        input.manifest.version != stream.artifact_version
            || input.manifest.ciphertext_bytes != frontier as u64
    }) {
        return Err(VerificationError::Manifest);
    }
    let mut frames = encrypted.as_ref().map(|input| {
        FrameVerifier::new(
            input.key,
            input.manifest.execution,
            stream.artifact_version,
            input.plaintext_cap,
            input.ciphertext_cap,
        )
    });
    let mut total_hash = Sha256::new();
    for segment in segments {
        let key = ObjectKey::new(segment.object_key.clone())?;
        let provider = ProviderVersion::from_stored(segment.provider_version.clone())?;
        let digest = crate::blob_store::decode_digest(&segment.sha256)?;
        let size = segment.size_bytes as u64;
        let mut reader = storage
            .get_pinned(&key, &provider, size, digest, None)
            .await?;
        let mut read = 0_u64;
        let mut segment_hash = Sha256::new();
        while let Some(chunk) = reader.next().await {
            let chunk = chunk?;
            read = read
                .checked_add(chunk.len() as u64)
                .ok_or(VerificationError::Frontier)?;
            if read > size {
                return Err(BlobStoreError::DigestMismatch.into());
            }
            segment_hash.update(&chunk);
            total_hash.update(&chunk);
            if let Some(frames) = &mut frames {
                frames.update(&chunk)?;
            }
        }
        // Do not rely on the provider adapter to validate size/digest, nor stop
        // at the expected length: a terminal reader error must still reject.
        if read != size || <[u8; 32]>::from(segment_hash.finalize()) != digest {
            return Err(BlobStoreError::DigestMismatch.into());
        }
    }
    let sha256 = crate::blob_store::encode_digest(&total_hash.finalize().into());
    if let Some(input) = encrypted {
        if input.manifest.ciphertext_sha256 != sha256 {
            return Err(VerificationError::Manifest);
        }
        frames.expect("encrypted verifier").finish(input.manifest)?;
    }
    Ok(VerifiedBytes {
        size: frontier,
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob_store::{BlobBody, BlobReader, ByteRange, StoredObject};
    use crate::models::enums::LogStreamBackend;
    use bytes::Bytes;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Storage {
        bytes: Vec<u8>,
        terminal_error: bool,
        wrong_provider: bool,
        reads: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl BlobStore for Storage {
        async fn preflight(&self) -> Result<(), BlobStoreError> {
            unreachable!()
        }
        async fn put(
            &self,
            _: &ObjectKey,
            _: BlobBody,
            _: [u8; 32],
        ) -> Result<StoredObject, BlobStoreError> {
            unreachable!()
        }
        async fn get(
            &self,
            _: &ObjectKey,
            _: &ProviderVersion,
            _: Option<ByteRange>,
        ) -> Result<BlobReader, BlobStoreError> {
            unreachable!()
        }
        async fn get_pinned(
            &self,
            _: &ObjectKey,
            provider: &ProviderVersion,
            _: u64,
            _: [u8; 32],
            range: Option<ByteRange>,
        ) -> Result<BlobReader, BlobStoreError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            assert_eq!(*provider, ProviderVersion::from_stored("v:1").unwrap());
            assert!(range.is_none());
            if self.wrong_provider {
                return Err(BlobStoreError::VersionMismatch);
            }
            let mut chunks: Vec<_> = self
                .bytes
                .chunks(7)
                .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
                .collect();
            if self.terminal_error {
                chunks.push(Err(BlobStoreError::DigestMismatch));
            }
            Ok(futures::stream::iter(chunks).boxed())
        }
        async fn head(&self, _: &ObjectKey) -> Result<Option<StoredObject>, BlobStoreError> {
            unreachable!()
        }
        async fn delete(&self, _: &ObjectKey, _: &ProviderVersion) -> Result<(), BlobStoreError> {
            unreachable!()
        }
    }
    fn fixture(bytes: &[u8]) -> (Storage, LogStream, Vec<LogSegment>) {
        let stream = LogStream {
            id: 1,
            artifact_version: 2,
            backend: LogStreamBackend::ObjectSegments,
            max_unflushed_bytes: 1024,
            max_unflushed_milliseconds: 500,
            next_sequence: 1,
            total_bytes: bytes.len() as i64,
            truncated: false,
            sealed: false,
            created: chrono::Utc::now(),
            sealed_at: None,
        };
        let segment = LogSegment {
            id: 3,
            stream: 1,
            sequence: 0,
            byte_start: 0,
            byte_end: bytes.len() as i64,
            size_bytes: bytes.len() as i64,
            sha256: crate::blob_store::encode_digest(&Sha256::digest(bytes).into()),
            object_key: "logs/accepted".into(),
            provider_version: "v:1".into(),
            created: stream.created,
        };
        (
            Storage {
                bytes: bytes.to_vec(),
                terminal_error: false,
                wrong_provider: false,
                reads: AtomicUsize::new(0),
            },
            stream,
            vec![segment],
        )
    }
    #[tokio::test]
    async fn plaintext_and_empty_frontier_verify() {
        let (store, mut stream, segments) = fixture(b"accepted bytes");
        let verified = verify(&store, &stream, &segments, None).await.unwrap();
        assert_eq!(verified.size(), 14);
        assert_eq!(
            verified.sha256(),
            crate::blob_store::encode_digest(&Sha256::digest(b"accepted bytes").into())
        );
        stream.next_sequence = 0;
        stream.total_bytes = 0;
        let empty = verify(&store, &stream, &[], None).await.unwrap();
        assert_eq!(
            empty.sha256(),
            crate::blob_store::encode_digest(&Sha256::digest([]).into())
        );
        assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn provider_short_extra_digest_and_terminal_errors_reject() {
        for failure in 0..5 {
            let (mut store, stream, segments) = fixture(b"accepted bytes");
            match failure {
                0 => store.wrong_provider = true,
                1 => {
                    store.bytes.pop();
                }
                2 => store.bytes.push(0),
                3 => store.bytes[0] ^= 1,
                4 => store.terminal_error = true,
                _ => unreachable!(),
            }
            assert!(
                verify(&store, &stream, &segments, None).await.is_err(),
                "failure {failure}"
            );
        }
    }
    #[tokio::test]
    async fn invalid_membership_never_reads_storage() {
        let (store, stream, mut segments) = fixture(b"accepted bytes");
        segments[0].stream += 1;
        assert!(verify(&store, &stream, &segments, None).await.is_err());
        assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn encrypted_frames_and_manifest_must_both_complete() {
        let key = "0123456789abcdef0123456789abcdef";
        let frame = crate::runtime_read_log::encode(key, 1, 2, 0, b"secret").unwrap();
        let mut totals = crate::runtime_log_completion::FrameTotals::default();
        totals.record(&frame, 6).unwrap();
        let manifest = totals.manifest(1, 2).unwrap();
        let (store, stream, segments) = fixture(&frame);
        fn input<'a>(key: &'a str, manifest: &'a FinalLogManifest) -> EncryptedInput<'a> {
            EncryptedInput {
                key,
                manifest,
                plaintext_cap: 1024,
                ciphertext_cap: 4096,
            }
        }
        verify(&store, &stream, &segments, Some(input(key, &manifest)))
            .await
            .unwrap();
        let mut wrong = manifest.clone();
        wrong.ciphertext_sha256 = "0".repeat(64);
        assert!(matches!(
            verify(&store, &stream, &segments, Some(input(key, &wrong))).await,
            Err(VerificationError::Manifest)
        ));
        wrong = manifest.clone();
        wrong.ciphertext_bytes += 1;
        assert!(matches!(
            verify(&store, &stream, &segments, Some(input(key, &wrong))).await,
            Err(VerificationError::Manifest)
        ));
        wrong = manifest.clone();
        wrong.frame_count += 1;
        assert!(verify(&store, &stream, &segments, Some(input(key, &wrong)))
            .await
            .is_err());
        let mut corrupt_frame = frame.clone();
        *corrupt_frame.last_mut().unwrap() ^= 1;
        let (store, stream, segments) = fixture(&corrupt_frame);
        wrong = manifest.clone();
        wrong.ciphertext_sha256 = segments[0].sha256.clone();
        assert!(verify(&store, &stream, &segments, Some(input(key, &wrong)))
            .await
            .is_err());
    }
}
