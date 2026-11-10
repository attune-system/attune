pub struct SealGetBarrier {
    pub inner: std::sync::Arc<dyn attune_common::blob_store::BlobStore>,
    pub uploaded: tokio::sync::Notify,
    pub finish: tokio::sync::Semaphore,
    pub gets: std::sync::atomic::AtomicUsize,
}
impl std::fmt::Debug for SealGetBarrier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SealGetBarrier")
    }
}
#[async_trait::async_trait]
impl attune_common::blob_store::BlobStore for SealGetBarrier {
    async fn preflight(
        &self,
    ) -> std::result::Result<(), attune_common::blob_store::BlobStoreError> {
        self.inner.preflight().await
    }
    async fn put(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        body: attune_common::blob_store::BlobBody,
        digest: [u8; 32],
    ) -> std::result::Result<
        attune_common::blob_store::StoredObject,
        attune_common::blob_store::BlobStoreError,
    > {
        let stored = self.inner.put(key, body, digest).await?;
        Ok(stored)
    }
    async fn get(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        version: &attune_common::blob_store::ProviderVersion,
        range: Option<attune_common::blob_store::ByteRange>,
    ) -> std::result::Result<
        attune_common::blob_store::BlobReader,
        attune_common::blob_store::BlobStoreError,
    > {
        self.inner.get(key, version, range).await
    }
    async fn put_file_atomic(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        body: attune_common::blob_store::BlobBody,
        size: u64,
        digest: [u8; 32],
    ) -> std::result::Result<
        attune_common::blob_store::StoredObject,
        attune_common::blob_store::BlobStoreError,
    > {
        let stored = self.inner.put_file_atomic(key, body, size, digest).await?;
        Ok(stored)
    }
    async fn get_pinned(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        version: &attune_common::blob_store::ProviderVersion,
        size: u64,
        digest: [u8; 32],
        range: Option<attune_common::blob_store::ByteRange>,
    ) -> std::result::Result<
        attune_common::blob_store::BlobReader,
        attune_common::blob_store::BlobStoreError,
    > {
        self.gets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.uploaded.notify_one();
        self.finish
            .acquire()
            .await
            .expect("owned GET gate")
            .forget();
        self.inner
            .get_pinned(key, version, size, digest, range)
            .await
    }
    async fn head(
        &self,
        key: &attune_common::blob_store::ObjectKey,
    ) -> std::result::Result<
        Option<attune_common::blob_store::StoredObject>,
        attune_common::blob_store::BlobStoreError,
    > {
        self.inner.head(key).await
    }
    async fn head_version(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        version: &attune_common::blob_store::ProviderVersion,
    ) -> std::result::Result<
        Option<attune_common::blob_store::StoredObject>,
        attune_common::blob_store::BlobStoreError,
    > {
        self.inner.head_version(key, version).await
    }
    async fn delete(
        &self,
        key: &attune_common::blob_store::ObjectKey,
        version: &attune_common::blob_store::ProviderVersion,
    ) -> std::result::Result<(), attune_common::blob_store::BlobStoreError> {
        self.inner.delete(key, version).await
    }
}
