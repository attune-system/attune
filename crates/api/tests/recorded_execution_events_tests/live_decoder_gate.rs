//! An owned blob-reader wait, not an authorization override. The inner store
//! still verifies the exact provider version, expected length and digest.
use attune_common::blob_store::*;
use futures::{StreamExt, TryStreamExt};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(super) enum WaitAt {
    FirstRead,
    Eof,
}

pub(super) struct DecoderGate {
    pub inner: Arc<dyn BlobStore>,
    pub key: String,
    pub at: WaitAt,
    pub arrived: Arc<tokio::sync::Semaphore>,
    pub release: Arc<tokio::sync::Semaphore>,
}

impl std::fmt::Debug for DecoderGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DecoderGate")
    }
}

impl DecoderGate {
    fn wrap(&self, key: &ObjectKey, reader: BlobReader) -> BlobReader {
        if key.as_str() != self.key {
            return reader;
        }
        let (arrived, release, at) = (self.arrived.clone(), self.release.clone(), self.at);
        futures::stream::try_unfold(
            (reader, false, arrived, release),
            move |(mut reader, mut waited, arrived, release)| async move {
                if !waited && matches!(at, WaitAt::FirstRead) {
                    arrived.add_permits(1);
                    release
                        .acquire()
                        .await
                        .expect("owned decoder release")
                        .forget();
                    waited = true;
                }
                let item = reader.try_next().await?;
                if item.is_none() && !waited && matches!(at, WaitAt::Eof) {
                    arrived.add_permits(1);
                    release
                        .acquire()
                        .await
                        .expect("owned decoder EOF release")
                        .forget();
                    waited = true;
                }
                Ok(item.map(|bytes| (bytes, (reader, waited, arrived, release))))
            },
        )
        .boxed()
    }
}

#[async_trait::async_trait]
impl BlobStore for DecoderGate {
    async fn preflight(&self) -> std::result::Result<(), BlobStoreError> {
        self.inner.preflight().await
    }
    async fn put(
        &self,
        key: &ObjectKey,
        body: BlobBody,
        digest: [u8; 32],
    ) -> std::result::Result<StoredObject, BlobStoreError> {
        self.inner.put(key, body, digest).await
    }
    async fn get(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
        range: Option<ByteRange>,
    ) -> std::result::Result<BlobReader, BlobStoreError> {
        Ok(self.wrap(key, self.inner.get(key, version, range).await?))
    }
    async fn get_pinned(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
        size: u64,
        digest: [u8; 32],
        range: Option<ByteRange>,
    ) -> std::result::Result<BlobReader, BlobStoreError> {
        Ok(self.wrap(
            key,
            self.inner
                .get_pinned(key, version, size, digest, range)
                .await?,
        ))
    }
    async fn head(
        &self,
        key: &ObjectKey,
    ) -> std::result::Result<Option<StoredObject>, BlobStoreError> {
        self.inner.head(key).await
    }
    async fn head_version(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
    ) -> std::result::Result<Option<StoredObject>, BlobStoreError> {
        self.inner.head_version(key, version).await
    }
    async fn delete(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
    ) -> std::result::Result<(), BlobStoreError> {
        self.inner.delete(key, version).await
    }
}
