//! Immutable durable byte storage.

use async_trait::async_trait;
use bytes::Bytes;
use futures::{stream, stream::BoxStream, StreamExt, TryStreamExt};
use object_store::{
    aws::{AmazonS3, AwsAuthorizer, S3CopyIfNotExists},
    gcp::GoogleCloudStorage,
    path::Path,
    Attribute, Attributes, CopyMode, CopyOptions, GetOptions, ObjectStore, ObjectStoreExt,
    PutMultipartOptions, PutPayloadMut,
};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    ops::Range,
    path::{Path as StdPath, PathBuf},
    sync::Arc,
};
use tokio_util::io::ReaderStream;

const DIGEST_METADATA_KEY: &str = "attune-sha256";
const MULTIPART_CHUNK_SIZE: usize = 5 * 1024 * 1024;

/// A producer-controlled byte stream. `put` retains at most one source chunk and one
/// 5 MiB provider part, so producers must also keep individual chunks bounded.
pub type BlobBody = BoxStream<'static, Result<Bytes, BlobStoreError>>;
pub type BlobReader = BoxStream<'static, Result<Bytes, BlobStoreError>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectKey(String);

impl ObjectKey {
    pub fn new(value: impl Into<String>) -> Result<Self, BlobStoreError> {
        let value = value.into();
        Path::parse(&value).map_err(|_| BlobStoreError::InvalidKey(value.clone()))?;
        if value.is_empty() || value.starts_with('/') {
            return Err(BlobStoreError::InvalidKey(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderVersion(String);

impl ProviderVersion {
    pub fn from_stored(value: impl Into<String>) -> Result<Self, BlobStoreError> {
        let value = value.into();
        let valid = value
            .strip_prefix("v:")
            .is_some_and(|version| !version.trim().is_empty() && version.trim() != "null")
            || value
                .strip_prefix("e:")
                .is_some_and(|etag| !etag.trim().is_empty());
        if valid {
            Ok(Self(value))
        } else {
            Err(BlobStoreError::InvalidVersion)
        }
    }

    pub fn as_stored(&self) -> &str {
        &self.0
    }

    fn apply(&self, options: GetOptions) -> GetOptions {
        match self.0.split_at(2) {
            ("v:", value) => options.with_version(Some(value)),
            ("e:", value) => options.with_if_match(Some(value)),
            _ => options,
        }
    }

    fn version_id(&self) -> Result<&str, BlobStoreError> {
        self.0
            .strip_prefix("v:")
            .ok_or(BlobStoreError::InvalidVersion)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub key: ObjectKey,
    pub provider_version: ProviderVersion,
    pub size: u64,
    pub sha256: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    pub fn new(start: u64, end: u64) -> Result<Self, BlobStoreError> {
        if start >= end {
            return Err(BlobStoreError::InvalidRange);
        }
        Ok(Self { start, end })
    }

    fn as_range(self) -> Range<u64> {
        self.start..self.end
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BlobStoreError {
    #[error("invalid blob object key: {0}")]
    InvalidKey(String),
    #[error("invalid provider version")]
    InvalidVersion,
    #[error("invalid byte range")]
    InvalidRange,
    #[error("blob already exists")]
    Conflict,
    #[error("blob not found")]
    NotFound,
    #[error("the requested blob version does not match")]
    VersionMismatch,
    #[error("blob SHA-256 digest does not match")]
    DigestMismatch,
    #[error("blob body was interrupted: {0}")]
    Interrupted(String),
    #[error("blob storage operation failed: {0}")]
    Backend(String),
}

#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn preflight(&self) -> Result<(), BlobStoreError>;

    async fn put(
        &self,
        key: &ObjectKey,
        body: BlobBody,
        expected_sha256: [u8; 32],
    ) -> Result<StoredObject, BlobStoreError>;

    async fn get(
        &self,
        key: &ObjectKey,
        provider_version: &ProviderVersion,
        range: Option<ByteRange>,
    ) -> Result<BlobReader, BlobStoreError>;

    async fn get_pinned(
        &self,
        key: &ObjectKey,
        expected_version: &ProviderVersion,
        object_size: u64,
        object_sha256: [u8; 32],
        range: Option<ByteRange>,
    ) -> Result<BlobReader, BlobStoreError>;

    async fn head(&self, key: &ObjectKey) -> Result<Option<StoredObject>, BlobStoreError>;

    async fn delete(
        &self,
        key: &ObjectKey,
        provider_version: &ProviderVersion,
    ) -> Result<(), BlobStoreError>;
}

pub fn from_config(
    config: &crate::config::BlobStorageConfig,
) -> Result<Arc<dyn BlobStore>, BlobStoreError> {
    use crate::config::BlobStorageConfig;
    match config {
        BlobStorageConfig::Filesystem { root } => Ok(Arc::new(FilesystemBlobStore::new(root)?)),
        BlobStorageConfig::S3 {
            bucket,
            region,
            prefix,
            endpoint,
            kms_key,
        } => Ok(Arc::new(S3BlobStore::new(
            bucket,
            region,
            prefix,
            endpoint.as_deref(),
            kms_key.as_deref(),
        )?)),
        BlobStorageConfig::Gcs {
            bucket,
            prefix,
            endpoint,
        } => Ok(Arc::new(GcsBlobStore::new(
            bucket,
            prefix,
            endpoint.as_deref(),
        )?)),
    }
}

pub fn body_from_bytes(bytes: Bytes) -> BlobBody {
    stream::once(async move { Ok(bytes) }).boxed()
}

pub async fn body_from_file(path: impl AsRef<StdPath>) -> Result<BlobBody, BlobStoreError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| BlobStoreError::Backend(error.to_string()))?;
    Ok(ReaderStream::with_capacity(file, 64 * 1024)
        .map(|result| result.map_err(|error| BlobStoreError::Interrupted(error.to_string())))
        .boxed())
}

pub async fn hash_file(path: impl AsRef<StdPath>) -> Result<(u64, [u8; 32]), BlobStoreError> {
    let mut body = body_from_file(path).await?;
    let mut size = 0_u64;
    let mut hasher = Sha256::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        size = size
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| BlobStoreError::Backend("blob size overflow".into()))?;
        hasher.update(chunk);
    }
    Ok((size, hasher.finalize().into()))
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn verify_reader(
    source: BlobReader,
    expected_size: u64,
    expected_digest: Option<[u8; 32]>,
) -> BlobReader {
    stream::try_unfold(
        (source, Sha256::new(), 0_u64, false),
        move |(mut source, mut hasher, mut size, done)| async move {
            if done {
                return Ok(None);
            }
            match source.next().await {
                Some(Ok(chunk)) => {
                    size = size
                        .checked_add(chunk.len() as u64)
                        .ok_or_else(|| BlobStoreError::Backend("blob size overflow".into()))?;
                    hasher.update(&chunk);
                    Ok(Some((chunk, (source, hasher, size, false))))
                }
                Some(Err(error)) => Err(error),
                None if size != expected_size => Err(BlobStoreError::Interrupted(format!(
                    "expected {expected_size} bytes, received {size}"
                ))),
                None if expected_digest
                    .is_some_and(|expected| <[u8; 32]>::from(hasher.finalize()) != expected) =>
                {
                    Err(BlobStoreError::DigestMismatch)
                }
                None => Ok(Some((Bytes::new(), (source, Sha256::new(), size, true)))),
            }
        },
    )
    .filter(|result| futures::future::ready(!matches!(result, Ok(bytes) if bytes.is_empty())))
    .boxed()
}

struct ObjectStoreBlobStore {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    metadata_supported: bool,
    require_provider_version: bool,
}

impl ObjectStoreBlobStore {
    fn new(
        store: Arc<dyn ObjectStore>,
        prefix: &str,
        metadata_supported: bool,
        require_provider_version: bool,
    ) -> Self {
        Self {
            store,
            prefix: prefix.trim_matches('/').to_string(),
            metadata_supported,
            require_provider_version,
        }
    }

    fn path(&self, key: &ObjectKey) -> Result<Path, BlobStoreError> {
        let value = if self.prefix.is_empty() {
            key.as_str().to_string()
        } else {
            format!("{}/{}", self.prefix, key.as_str())
        };
        Path::parse(value).map_err(|error| BlobStoreError::InvalidKey(error.to_string()))
    }

    async fn metadata_from_result(
        &self,
        key: &ObjectKey,
        result: object_store::GetResult,
    ) -> Result<StoredObject, BlobStoreError> {
        let version =
            provider_version(&result.meta, self.require_provider_version).ok_or_else(|| {
                BlobStoreError::Backend("provider did not return an object version or ETag".into())
            })?;
        let size = result.meta.size;
        let digest = match result
            .attributes
            .get(&Attribute::Metadata(Cow::Borrowed(DIGEST_METADATA_KEY)))
        {
            Some(value) => decode_digest(value.as_ref())?,
            None => {
                let mut stream = self
                    .store
                    .get_opts(&self.path(key)?, GetOptions::new())
                    .await
                    .map_err(map_backend_error)?
                    .into_stream();
                let mut hasher = Sha256::new();
                while let Some(chunk) = stream.next().await {
                    hasher.update(chunk.map_err(map_backend_error)?);
                }
                hasher.finalize().into()
            }
        };
        Ok(StoredObject {
            key: key.clone(),
            provider_version: version,
            size,
            sha256: digest,
        })
    }

    async fn reconcile_unknown_write(
        &self,
        key: &ObjectKey,
        size: u64,
        digest: [u8; 32],
    ) -> Result<Option<StoredObject>, BlobStoreError> {
        let Some(object) = self.head(key).await? else {
            return Ok(None);
        };
        Ok((object.size == size && object.sha256 == digest).then_some(object))
    }

    async fn get_pinned(
        &self,
        key: &ObjectKey,
        expected_version: &ProviderVersion,
        object_size: u64,
        object_sha256: [u8; 32],
        range: Option<ByteRange>,
    ) -> Result<BlobReader, BlobStoreError> {
        let mut options = expected_version.apply(GetOptions::new());
        if let Some(range) = range {
            options = options.with_range(Some(range.as_range()));
        }
        let result = self
            .store
            .get_opts(&self.path(key)?, options)
            .await
            .map_err(map_backend_error)?;
        let actual_version = provider_version(&result.meta, self.require_provider_version)
            .ok_or_else(|| {
                BlobStoreError::Backend("provider did not return an object version or ETag".into())
            })?;
        if actual_version != *expected_version {
            return Err(BlobStoreError::VersionMismatch);
        }
        if result.meta.size != object_size {
            return Err(BlobStoreError::Interrupted(format!(
                "expected object length {object_size}, provider reported {}",
                result.meta.size
            )));
        }
        let expected_size = range
            .map(|range| range.end - range.start)
            .unwrap_or(object_size);
        let expected_digest = range.is_none().then_some(object_sha256);
        let source = result
            .into_stream()
            .map(|result| result.map_err(map_backend_error))
            .boxed();
        Ok(verify_reader(source, expected_size, expected_digest))
    }
}

#[async_trait]
impl BlobStore for ObjectStoreBlobStore {
    async fn preflight(&self) -> Result<(), BlobStoreError> {
        preflight_exact_operations(
            self,
            "object store",
            "configured storage",
            "check storage access",
        )
        .await
    }

    async fn put(
        &self,
        key: &ObjectKey,
        mut body: BlobBody,
        expected_sha256: [u8; 32],
    ) -> Result<StoredObject, BlobStoreError> {
        let path = self.path(key)?;
        let staging_path = Path::parse(format!(".attune-upload/{}", rand::random::<u64>()))
            .map_err(|error| BlobStoreError::InvalidKey(error.to_string()))?;
        let mut attributes = Attributes::new();
        if self.metadata_supported {
            attributes.insert(
                Attribute::Metadata(Cow::Borrowed(DIGEST_METADATA_KEY)),
                encode_digest(&expected_sha256).into(),
            );
        }
        let mut upload = self
            .store
            .put_multipart_opts(
                &staging_path,
                PutMultipartOptions {
                    attributes,
                    ..Default::default()
                },
            )
            .await
            .map_err(map_backend_error)?;
        let mut part = PutPayloadMut::new();
        let mut hasher = Sha256::new();
        let mut size = 0_u64;
        while let Some(chunk) = body.next().await {
            let mut chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    let _ = upload.abort().await;
                    return Err(BlobStoreError::Interrupted(error.to_string()));
                }
            };
            hasher.update(&chunk);
            size = match size.checked_add(chunk.len() as u64) {
                Some(size) => size,
                None => {
                    let _ = upload.abort().await;
                    return Err(BlobStoreError::Backend("blob size overflow".into()));
                }
            };
            while !chunk.is_empty() {
                let remaining = MULTIPART_CHUNK_SIZE - part.content_length();
                part.push(chunk.split_to(chunk.len().min(remaining)));
                if part.content_length() == MULTIPART_CHUNK_SIZE {
                    let payload = std::mem::take(&mut part).into();
                    if let Err(error) = upload.put_part(payload).await {
                        let _ = upload.abort().await;
                        return Err(map_backend_error(error));
                    }
                }
            }
        }
        let actual_sha256: [u8; 32] = hasher.finalize().into();
        if actual_sha256 != expected_sha256 {
            let _ = upload.abort().await;
            return Err(BlobStoreError::DigestMismatch);
        }
        if !part.is_empty() || size == 0 {
            if let Err(error) = upload.put_part(part.into()).await {
                let _ = upload.abort().await;
                return Err(map_backend_error(error));
            }
        }
        if let Err(error) = upload.complete().await {
            let _ = upload.abort().await;
            return Err(map_backend_error(error));
        }

        let copy_result = self
            .store
            .copy_opts(
                &staging_path,
                &path,
                CopyOptions {
                    mode: CopyMode::Create,
                    ..Default::default()
                },
            )
            .await;
        let _ = self.store.delete(&staging_path).await;
        match copy_result {
            Ok(()) => self.head(key).await?.ok_or(BlobStoreError::NotFound),
            Err(object_store::Error::AlreadyExists { .. })
            | Err(object_store::Error::Precondition { .. }) => Err(BlobStoreError::Conflict),
            Err(error) => self
                .reconcile_unknown_write(key, size, actual_sha256)
                .await?
                .ok_or_else(|| map_backend_error(error)),
        }
    }

    async fn get(
        &self,
        key: &ObjectKey,
        provider_version: &ProviderVersion,
        range: Option<ByteRange>,
    ) -> Result<BlobReader, BlobStoreError> {
        let expected = self.head(key).await?.ok_or(BlobStoreError::NotFound)?;
        if &expected.provider_version != provider_version {
            return Err(BlobStoreError::VersionMismatch);
        }
        self.get_pinned(key, provider_version, expected.size, expected.sha256, range)
            .await
    }

    async fn get_pinned(
        &self,
        key: &ObjectKey,
        provider_version: &ProviderVersion,
        object_size: u64,
        object_sha256: [u8; 32],
        range: Option<ByteRange>,
    ) -> Result<BlobReader, BlobStoreError> {
        ObjectStoreBlobStore::get_pinned(
            self,
            key,
            provider_version,
            object_size,
            object_sha256,
            range,
        )
        .await
    }

    async fn head(&self, key: &ObjectKey) -> Result<Option<StoredObject>, BlobStoreError> {
        let result = match self
            .store
            .get_opts(&self.path(key)?, GetOptions::new().with_head(true))
            .await
        {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(error) => return Err(map_backend_error(error)),
        };
        self.metadata_from_result(key, result).await.map(Some)
    }

    async fn delete(
        &self,
        key: &ObjectKey,
        provider_version: &ProviderVersion,
    ) -> Result<(), BlobStoreError> {
        let options = provider_version.apply(GetOptions::new().with_head(true));
        match self.store.get_opts(&self.path(key)?, options).await {
            Ok(_) => {}
            Err(object_store::Error::NotFound { .. }) => return Ok(()),
            Err(error) => return Err(map_backend_error(error)),
        }
        match self.store.delete(&self.path(key)?).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(error) => Err(map_backend_error(error)),
        }
    }
}

pub struct FilesystemBlobStore {
    inner: ObjectStoreBlobStore,
    root: PathBuf,
    mutation_lock: tokio::sync::Mutex<()>,
}

impl FilesystemBlobStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, BlobStoreError> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .map_err(|error| BlobStoreError::Backend(error.to_string()))?;
        let store = object_store::local::LocalFileSystem::new_with_prefix(&root)
            .map_err(map_backend_error)?;
        Ok(Self {
            inner: ObjectStoreBlobStore::new(Arc::new(store), "", false, false),
            root,
            mutation_lock: tokio::sync::Mutex::new(()),
        })
    }
}

pub struct S3BlobStore {
    inner: ObjectStoreBlobStore,
    native: Option<S3NativeDelete>,
}

struct S3NativeDelete {
    store: AmazonS3,
    bucket: String,
    region: String,
    client: reqwest::Client,
}

impl S3BlobStore {
    pub fn new(
        bucket: &str,
        region: &str,
        prefix: &str,
        endpoint: Option<&str>,
        kms_key: Option<&str>,
    ) -> Result<Self, BlobStoreError> {
        let mut builder = object_store::aws::AmazonS3Builder::from_env()
            .with_bucket_name(bucket)
            .with_region(region)
            .with_copy_if_not_exists(S3CopyIfNotExists::Multipart);
        if let Some(endpoint) = endpoint {
            builder = builder
                .with_endpoint(endpoint)
                .with_allow_http(endpoint.starts_with("http://"));
        }
        if let Some(kms_key) = kms_key {
            builder = builder.with_sse_kms_encryption(kms_key);
        }
        let store = builder.build().map_err(map_backend_error)?;
        Ok(Self {
            inner: ObjectStoreBlobStore::new(Arc::new(store.clone()), prefix, true, true),
            native: Some(S3NativeDelete {
                store,
                bucket: bucket.to_string(),
                region: region.to_string(),
                client: reqwest::Client::new(),
            }),
        })
    }

    #[cfg(test)]
    fn with_store(store: Arc<dyn ObjectStore>, prefix: &str) -> Self {
        Self {
            inner: ObjectStoreBlobStore::new(store, prefix, true, true),
            native: None,
        }
    }

    async fn preflight_check(&self) -> Result<(), BlobStoreError> {
        let bucket = match &self.native {
            Some(native) => {
                native.check_bucket_versioning().await?;
                native.bucket.clone()
            }
            None => "test bucket".to_string(),
        };
        preflight_exact_operations(
            self,
            "S3",
            &bucket,
            "enable bucket versioning and grant the API identity object create, versioned read, and versioned delete access",
        )
        .await
    }
}

pub struct GcsBlobStore {
    inner: ObjectStoreBlobStore,
    native: Option<GcsNativeDelete>,
}

struct GcsNativeDelete {
    store: GoogleCloudStorage,
    bucket: String,
    base_url: String,
    client: reqwest::Client,
}

impl GcsBlobStore {
    pub fn new(bucket: &str, prefix: &str, endpoint: Option<&str>) -> Result<Self, BlobStoreError> {
        let mut builder =
            object_store::gcp::GoogleCloudStorageBuilder::from_env().with_bucket_name(bucket);
        if let Some(endpoint) = endpoint {
            builder = builder.with_base_url(endpoint);
        }
        let store = builder.build().map_err(map_backend_error)?;
        Ok(Self {
            inner: ObjectStoreBlobStore::new(Arc::new(store.clone()), prefix, true, true),
            native: Some(GcsNativeDelete {
                store,
                bucket: bucket.to_string(),
                base_url: endpoint
                    .unwrap_or("https://storage.googleapis.com")
                    .trim_end_matches('/')
                    .to_string(),
                client: reqwest::Client::new(),
            }),
        })
    }

    #[cfg(test)]
    fn with_store(store: Arc<dyn ObjectStore>, prefix: &str) -> Self {
        Self {
            inner: ObjectStoreBlobStore::new(store, prefix, true, true),
            native: None,
        }
    }

    async fn preflight_check(&self) -> Result<(), BlobStoreError> {
        let bucket = self
            .native
            .as_ref()
            .map(|native| native.bucket.as_str())
            .unwrap_or("test bucket");
        preflight_exact_operations(
            self,
            "GCS",
            bucket,
            "grant the API identity generation-aware object create, read, and delete access",
        )
        .await
    }
}

#[async_trait]
impl BlobStore for FilesystemBlobStore {
    async fn preflight(&self) -> Result<(), BlobStoreError> {
        preflight_exact_operations(
            self,
            "filesystem",
            &self.root.display().to_string(),
            "make the directory writable by the API process",
        )
        .await
    }

    async fn put(
        &self,
        key: &ObjectKey,
        body: BlobBody,
        expected_sha256: [u8; 32],
    ) -> Result<StoredObject, BlobStoreError> {
        let _guard = self.mutation_lock.lock().await;
        self.inner.put(key, body, expected_sha256).await
    }

    async fn get(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
        range: Option<ByteRange>,
    ) -> Result<BlobReader, BlobStoreError> {
        self.inner.get(key, version, range).await
    }

    async fn get_pinned(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
        object_size: u64,
        object_sha256: [u8; 32],
        range: Option<ByteRange>,
    ) -> Result<BlobReader, BlobStoreError> {
        self.inner
            .get_pinned(key, version, object_size, object_sha256, range)
            .await
    }

    async fn head(&self, key: &ObjectKey) -> Result<Option<StoredObject>, BlobStoreError> {
        self.inner.head(key).await
    }

    async fn delete(
        &self,
        key: &ObjectKey,
        version: &ProviderVersion,
    ) -> Result<(), BlobStoreError> {
        let _guard = self.mutation_lock.lock().await;
        let source = self.root.join(key.as_str());
        let quarantine_name = format!(".attune-delete-{}", rand::random::<u64>());
        let quarantine = source.with_file_name(&quarantine_name);
        match tokio::fs::rename(&source, &quarantine).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(BlobStoreError::Backend(error.to_string())),
        }

        let quarantine_key = ObjectKey::new(
            key.as_str()
                .rsplit_once('/')
                .map(|(parent, _)| format!("{parent}/{quarantine_name}"))
                .unwrap_or(quarantine_name),
        )?;
        let quarantined_version = self
            .inner
            .head(&quarantine_key)
            .await?
            .ok_or_else(|| BlobStoreError::Backend("quarantined blob disappeared".into()))?
            .provider_version;
        if quarantined_version != *version {
            match tokio::fs::hard_link(&quarantine, &source).await {
                Ok(()) => {
                    tokio::fs::remove_file(&quarantine)
                        .await
                        .map_err(|error| BlobStoreError::Backend(error.to_string()))?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(BlobStoreError::VersionMismatch);
                }
                Err(error) => return Err(BlobStoreError::Backend(error.to_string())),
            }
            return Err(BlobStoreError::VersionMismatch);
        }
        tokio::fs::remove_file(quarantine)
            .await
            .map_err(|error| BlobStoreError::Backend(error.to_string()))
    }
}
macro_rules! delegate_cloud_blob_store {
    ($store:ty) => {
        #[async_trait]
        impl BlobStore for $store {
            async fn preflight(&self) -> Result<(), BlobStoreError> {
                self.preflight_check().await
            }

            async fn put(
                &self,
                key: &ObjectKey,
                body: BlobBody,
                expected_sha256: [u8; 32],
            ) -> Result<StoredObject, BlobStoreError> {
                self.inner.put(key, body, expected_sha256).await
            }

            async fn get(
                &self,
                key: &ObjectKey,
                version: &ProviderVersion,
                range: Option<ByteRange>,
            ) -> Result<BlobReader, BlobStoreError> {
                self.inner.get(key, version, range).await
            }

            async fn get_pinned(
                &self,
                key: &ObjectKey,
                version: &ProviderVersion,
                object_size: u64,
                object_sha256: [u8; 32],
                range: Option<ByteRange>,
            ) -> Result<BlobReader, BlobStoreError> {
                self.inner
                    .get_pinned(key, version, object_size, object_sha256, range)
                    .await
            }

            async fn head(&self, key: &ObjectKey) -> Result<Option<StoredObject>, BlobStoreError> {
                self.inner.head(key).await
            }

            async fn delete(
                &self,
                key: &ObjectKey,
                version: &ProviderVersion,
            ) -> Result<(), BlobStoreError> {
                match &self.native {
                    Some(native) => native.delete(&self.inner.path(key)?, version).await,
                    None => self.inner.delete(key, version).await,
                }
            }
        }
    };
}

delegate_cloud_blob_store!(S3BlobStore);
delegate_cloud_blob_store!(GcsBlobStore);

async fn preflight_exact_operations(
    store: &dyn BlobStore,
    provider: &str,
    target: &str,
    correction: &str,
) -> Result<(), BlobStoreError> {
    let key = ObjectKey::new(format!(
        ".attune-preflight/{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ))?;
    let bytes = Bytes::from_static(b"attune storage preflight");
    let stored = store
        .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
        .await
        .map_err(|_| {
            preflight_error(
                provider,
                target,
                "could not create a versioned object",
                correction,
            )
        })?;

    let read_result = async {
        let read = store.get(&key, &stored.provider_version, None).await?;
        let chunks = read.try_collect::<Vec<_>>().await?;
        if chunks.concat() != bytes {
            return Err(BlobStoreError::Backend(
                "preflight object contents differed".into(),
            ));
        }
        Ok(())
    }
    .await;
    if read_result.is_err() {
        let _ = store.delete(&key, &stored.provider_version).await;
        return Err(preflight_error(
            provider,
            target,
            "could not read the exact object version returned by the provider",
            correction,
        ));
    }

    store
        .delete(&key, &stored.provider_version)
        .await
        .map_err(|_| {
            preflight_error(
                provider,
                target,
                "could not delete the exact object version returned by the provider",
                correction,
            )
        })?;
    match store.head(&key).await {
        Ok(None) => Ok(()),
        _ => Err(preflight_error(
            provider,
            target,
            "the exact-version delete did not remove the preflight object",
            correction,
        )),
    }
}

fn preflight_error(
    provider: &str,
    target: &str,
    capability: &str,
    correction: &str,
) -> BlobStoreError {
    BlobStoreError::Backend(format!(
        "{provider} storage preflight failed for bucket or path '{target}': {capability}; {correction}"
    ))
}

impl S3NativeDelete {
    async fn check_bucket_versioning(&self) -> Result<(), BlobStoreError> {
        use object_store::signer::Signer;

        let mut url = self
            .store
            .signed_url(
                reqwest::Method::GET,
                &Path::from(""),
                std::time::Duration::from_secs(60),
            )
            .await
            .map_err(|_| s3_versioning_access_error(&self.bucket))?;
        url.set_query(None);
        url.query_pairs_mut().append_key_only("versioning");

        let credential = self
            .store
            .credentials()
            .get_credential()
            .await
            .map_err(|_| s3_versioning_access_error(&self.bucket))?;
        let mut signed =
            object_store::client::HttpRequest::new(object_store::client::HttpRequestBody::empty());
        *signed.method_mut() = reqwest::Method::GET;
        *signed.uri_mut() = url
            .as_str()
            .parse()
            .map_err(|_| s3_versioning_access_error(&self.bucket))?;
        AwsAuthorizer::new(&credential, "s3", &self.region)
            .try_authorize(&mut signed, None)
            .map_err(|_| s3_versioning_access_error(&self.bucket))?;
        let response = self
            .client
            .get(url)
            .headers(signed.headers().clone())
            .send()
            .await
            .map_err(|_| s3_versioning_access_error(&self.bucket))?;
        if !response.status().is_success() {
            return Err(s3_versioning_access_error(&self.bucket));
        }
        let body = response
            .text()
            .await
            .map_err(|_| s3_versioning_access_error(&self.bucket))?;
        let status = body
            .split_once("<Status>")
            .and_then(|(_, rest)| rest.split_once("</Status>"))
            .map(|(status, _)| status.trim());
        if status == Some("Enabled") {
            return Ok(());
        }
        let state = if status == Some("Suspended") {
            "suspended"
        } else {
            "disabled"
        };
        Err(preflight_error(
            "S3",
            &self.bucket,
            &format!("bucket versioning is {state}"),
            "enable S3 bucket versioning before starting Attune",
        ))
    }

    async fn delete(
        &self,
        path: &Path,
        provider_version: &ProviderVersion,
    ) -> Result<(), BlobStoreError> {
        use object_store::signer::Signer;

        let version_id = provider_version.version_id()?;
        let mut url = self
            .store
            .signed_url(
                reqwest::Method::DELETE,
                path,
                std::time::Duration::from_secs(60),
            )
            .await
            .map_err(map_backend_error)?;
        url.set_query(None);
        url.query_pairs_mut().append_pair("versionId", version_id);

        let credential = self
            .store
            .credentials()
            .get_credential()
            .await
            .map_err(map_backend_error)?;
        let mut signed =
            object_store::client::HttpRequest::new(object_store::client::HttpRequestBody::empty());
        *signed.method_mut() = reqwest::Method::DELETE;
        let uri: reqwest::Url = url.clone();
        *signed.uri_mut() = uri
            .as_str()
            .parse()
            .map_err(|error| BlobStoreError::Backend(format!("invalid S3 URL: {error}")))?;
        AwsAuthorizer::new(&credential, "s3", &self.region)
            .try_authorize(&mut signed, None)
            .map_err(map_backend_error)?;
        let response = self
            .client
            .delete(url)
            .headers(signed.headers().clone())
            .send()
            .await
            .map_err(|error| BlobStoreError::Backend(error.to_string()))?;
        map_exact_delete_status(response.status())
    }
}

fn s3_versioning_access_error(bucket: &str) -> BlobStoreError {
    preflight_error(
        "S3",
        bucket,
        "bucket versioning could not be inspected",
        "grant the API identity s3:GetBucketVersioning and check bucket connectivity",
    )
}

impl GcsNativeDelete {
    async fn delete(
        &self,
        path: &Path,
        provider_version: &ProviderVersion,
    ) -> Result<(), BlobStoreError> {
        let generation = provider_version.version_id()?;
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|error| BlobStoreError::Backend(error.to_string()))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| BlobStoreError::Backend("GCS base URL cannot be a base".into()))?;
            segments.pop_if_empty().push(&self.bucket);
            for segment in path.as_ref().split('/') {
                segments.push(segment);
            }
        }
        let credential = self
            .store
            .credentials()
            .get_credential()
            .await
            .map_err(map_backend_error)?;
        let response = self
            .client
            .delete(url)
            .bearer_auth(&credential.bearer)
            .header("x-goog-if-generation-match", generation)
            .send()
            .await
            .map_err(|error| BlobStoreError::Backend(error.to_string()))?;
        map_exact_delete_status(response.status())
    }
}

fn map_exact_delete_status(status: reqwest::StatusCode) -> Result<(), BlobStoreError> {
    match status {
        status if status.is_success() || status == reqwest::StatusCode::NOT_FOUND => Ok(()),
        reqwest::StatusCode::PRECONDITION_FAILED | reqwest::StatusCode::CONFLICT => {
            Err(BlobStoreError::VersionMismatch)
        }
        status => Err(BlobStoreError::Backend(format!(
            "exact object deletion returned HTTP {status}"
        ))),
    }
}

fn provider_version(
    meta: &object_store::ObjectMeta,
    require_provider_version: bool,
) -> Option<ProviderVersion> {
    let version = meta
        .version
        .as_ref()
        .and_then(|value| ProviderVersion::from_stored(format!("v:{value}")).ok());
    if require_provider_version {
        version
    } else {
        version.or_else(|| {
            meta.e_tag
                .as_ref()
                .and_then(|value| ProviderVersion::from_stored(format!("e:{value}")).ok())
        })
    }
}

fn map_backend_error(error: object_store::Error) -> BlobStoreError {
    match error {
        object_store::Error::NotFound { .. } => BlobStoreError::NotFound,
        object_store::Error::AlreadyExists { .. } => BlobStoreError::Conflict,
        object_store::Error::Precondition { .. } => BlobStoreError::VersionMismatch,
        error => BlobStoreError::Backend(error.to_string()),
    }
}

fn encode_digest(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_digest(value: &str) -> Result<[u8; 32], BlobStoreError> {
    if value.len() != 64 {
        return Err(BlobStoreError::DigestMismatch);
    }
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| BlobStoreError::DigestMismatch)?;
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{stream::BoxStream, TryStreamExt};
    use object_store::memory::InMemory;
    use object_store::{
        CopyOptions, GetResult, ListResult, MultipartUpload, PutMultipartOptions, PutOptions,
        PutPayload, PutResult,
    };
    use std::{
        fmt,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Debug)]
    struct UnknownResultStore(InMemory);

    impl fmt::Display for UnknownResultStore {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("UnknownResultStore")
        }
    }

    #[derive(Debug)]
    struct VersionedStore(Arc<dyn ObjectStore>);

    #[derive(Default)]
    struct UploadStats {
        aborted: AtomicBool,
        in_flight_bytes: AtomicUsize,
        max_in_flight_bytes: AtomicUsize,
        part_sizes: std::sync::Mutex<Vec<usize>>,
        read_ranges: std::sync::Mutex<Vec<Option<object_store::GetRange>>>,
    }

    #[derive(Debug)]
    struct RecordingStore {
        inner: Arc<dyn ObjectStore>,
        stats: Arc<UploadStats>,
    }

    impl fmt::Display for RecordingStore {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("RecordingStore")
        }
    }

    #[derive(Debug)]
    struct RecordingUpload {
        inner: Box<dyn MultipartUpload>,
        stats: Arc<UploadStats>,
    }

    impl fmt::Debug for UploadStats {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("UploadStats")
        }
    }

    #[async_trait]
    impl MultipartUpload for RecordingUpload {
        fn put_part(&mut self, data: PutPayload) -> object_store::UploadPart {
            let size = data.content_length();
            self.stats.part_sizes.lock().unwrap().push(size);
            let in_flight = self.stats.in_flight_bytes.fetch_add(size, Ordering::SeqCst) + size;
            self.stats
                .max_in_flight_bytes
                .fetch_max(in_flight, Ordering::SeqCst);
            let future = self.inner.put_part(data);
            let stats = self.stats.clone();
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                let result = future.await;
                stats.in_flight_bytes.fetch_sub(size, Ordering::SeqCst);
                result
            })
        }

        async fn complete(&mut self) -> object_store::Result<PutResult> {
            self.inner.complete().await
        }

        async fn abort(&mut self) -> object_store::Result<()> {
            self.stats.aborted.store(true, Ordering::SeqCst);
            self.inner.abort().await
        }
    }

    #[async_trait]
    impl ObjectStore for RecordingStore {
        async fn put_opts(
            &self,
            location: &Path,
            payload: PutPayload,
            options: PutOptions,
        ) -> object_store::Result<PutResult> {
            self.inner.put_opts(location, payload, options).await
        }

        async fn put_multipart_opts(
            &self,
            location: &Path,
            options: PutMultipartOptions,
        ) -> object_store::Result<Box<dyn MultipartUpload>> {
            Ok(Box::new(RecordingUpload {
                inner: self.inner.put_multipart_opts(location, options).await?,
                stats: self.stats.clone(),
            }))
        }

        async fn get_opts(
            &self,
            location: &Path,
            options: GetOptions,
        ) -> object_store::Result<GetResult> {
            self.stats
                .read_ranges
                .lock()
                .unwrap()
                .push(options.range.clone());
            self.inner.get_opts(location, options).await
        }

        fn delete_stream(
            &self,
            locations: BoxStream<'static, object_store::Result<Path>>,
        ) -> BoxStream<'static, object_store::Result<Path>> {
            self.inner.delete_stream(locations)
        }

        fn list(
            &self,
            prefix: Option<&Path>,
        ) -> BoxStream<'static, object_store::Result<object_store::ObjectMeta>> {
            self.inner.list(prefix)
        }

        async fn list_with_delimiter(
            &self,
            prefix: Option<&Path>,
        ) -> object_store::Result<ListResult> {
            self.inner.list_with_delimiter(prefix).await
        }

        async fn copy_opts(
            &self,
            from: &Path,
            to: &Path,
            options: CopyOptions,
        ) -> object_store::Result<()> {
            self.inner.copy_opts(from, to, options).await
        }
    }

    async fn capture_request_with_body(
        status: u16,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0_u8; 1024];
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}"), task)
    }

    async fn capture_request(status: u16) -> (String, tokio::task::JoinHandle<String>) {
        capture_request_with_body(status, "").await
    }

    impl fmt::Display for VersionedStore {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("VersionedStore")
        }
    }

    #[async_trait]
    impl ObjectStore for VersionedStore {
        async fn put_opts(
            &self,
            location: &Path,
            payload: PutPayload,
            options: PutOptions,
        ) -> object_store::Result<PutResult> {
            self.0.put_opts(location, payload, options).await
        }

        async fn put_multipart_opts(
            &self,
            location: &Path,
            options: PutMultipartOptions,
        ) -> object_store::Result<Box<dyn MultipartUpload>> {
            self.0.put_multipart_opts(location, options).await
        }

        async fn get_opts(
            &self,
            location: &Path,
            mut options: GetOptions,
        ) -> object_store::Result<GetResult> {
            let requested_version = options.version.take();
            let mut result = self.0.get_opts(location, options).await?;
            let current_version =
                result
                    .meta
                    .e_tag
                    .clone()
                    .ok_or_else(|| object_store::Error::Generic {
                        store: "versioned-test",
                        source: std::io::Error::other("missing test object ETag").into(),
                    })?;
            if requested_version
                .as_deref()
                .is_some_and(|requested| requested != current_version)
            {
                return Err(object_store::Error::Precondition {
                    path: location.to_string(),
                    source: std::io::Error::other("object version changed").into(),
                });
            }
            result.meta.version = Some(current_version);
            Ok(result)
        }

        fn delete_stream(
            &self,
            locations: BoxStream<'static, object_store::Result<Path>>,
        ) -> BoxStream<'static, object_store::Result<Path>> {
            self.0.delete_stream(locations)
        }

        fn list(
            &self,
            prefix: Option<&Path>,
        ) -> BoxStream<'static, object_store::Result<object_store::ObjectMeta>> {
            self.0.list(prefix)
        }

        async fn list_with_delimiter(
            &self,
            prefix: Option<&Path>,
        ) -> object_store::Result<ListResult> {
            self.0.list_with_delimiter(prefix).await
        }

        async fn copy_opts(
            &self,
            from: &Path,
            to: &Path,
            options: CopyOptions,
        ) -> object_store::Result<()> {
            self.0.copy_opts(from, to, options).await
        }
    }

    #[async_trait]
    impl ObjectStore for UnknownResultStore {
        async fn put_opts(
            &self,
            location: &Path,
            payload: PutPayload,
            options: PutOptions,
        ) -> object_store::Result<PutResult> {
            self.0.put_opts(location, payload, options).await?;
            Err(object_store::Error::Generic {
                store: "unknown-result",
                source: std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "response was lost",
                )
                .into(),
            })
        }

        async fn put_multipart_opts(
            &self,
            location: &Path,
            options: PutMultipartOptions,
        ) -> object_store::Result<Box<dyn MultipartUpload>> {
            self.0.put_multipart_opts(location, options).await
        }

        async fn get_opts(
            &self,
            location: &Path,
            options: GetOptions,
        ) -> object_store::Result<GetResult> {
            self.0.get_opts(location, options).await
        }

        fn delete_stream(
            &self,
            locations: BoxStream<'static, object_store::Result<Path>>,
        ) -> BoxStream<'static, object_store::Result<Path>> {
            self.0.delete_stream(locations)
        }

        fn list(
            &self,
            prefix: Option<&Path>,
        ) -> BoxStream<'static, object_store::Result<object_store::ObjectMeta>> {
            self.0.list(prefix)
        }

        async fn list_with_delimiter(
            &self,
            prefix: Option<&Path>,
        ) -> object_store::Result<ListResult> {
            self.0.list_with_delimiter(prefix).await
        }

        async fn copy_opts(
            &self,
            from: &Path,
            to: &Path,
            options: CopyOptions,
        ) -> object_store::Result<()> {
            self.0.copy_opts(from, to, options).await
        }
    }

    async fn contract(store: &dyn BlobStore) {
        let key = ObjectKey::new("v1/test/blob").unwrap();
        let bytes = Bytes::from_static(b"immutable bytes");
        let digest = sha256(&bytes);
        let stored = store
            .put(&key, body_from_bytes(bytes.clone()), digest)
            .await
            .unwrap();
        assert_eq!(stored.size, bytes.len() as u64);
        assert_eq!(stored.sha256, digest);
        assert!(matches!(
            store
                .put(&key, body_from_bytes(bytes.clone()), digest)
                .await,
            Err(BlobStoreError::Conflict)
        ));

        let read = store
            .get(&key, &stored.provider_version, None)
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(read.concat(), bytes);
        let range = store
            .get(
                &key,
                &stored.provider_version,
                Some(ByteRange::new(2, 7).unwrap()),
            )
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(range.concat(), Bytes::from_static(b"mutab"));

        assert_eq!(store.head(&key).await.unwrap().unwrap(), stored);
        store.delete(&key, &stored.provider_version).await.unwrap();
        store.delete(&key, &stored.provider_version).await.unwrap();
        assert!(store.head(&key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn filesystem_passes_contract() {
        let directory = tempfile::tempdir().unwrap();
        contract(&FilesystemBlobStore::new(directory.path()).unwrap()).await;
    }

    #[tokio::test]
    async fn filesystem_preflight_succeeds_and_removes_its_probe() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilesystemBlobStore::new(directory.path()).unwrap();

        store.preflight().await.unwrap();

        assert!(walkdir::WalkDir::new(directory.path())
            .into_iter()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_type().is_file()));
    }

    #[tokio::test]
    async fn filesystem_put_removes_its_staging_tree() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilesystemBlobStore::new(directory.path()).unwrap();
        let key = ObjectKey::new("logs/execution/segment").unwrap();
        let bytes = Bytes::from_static(b"segment");

        store
            .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
            .await
            .unwrap();

        assert!(std::fs::read_dir(directory.path().join(".attune-upload"))
            .unwrap()
            .next()
            .is_none());
    }

    #[tokio::test]
    async fn s3_passes_contract() {
        let store = Arc::new(VersionedStore(Arc::new(InMemory::new())));
        contract(&S3BlobStore::with_store(store, "s3")).await;
    }

    #[tokio::test]
    async fn gcs_passes_contract() {
        let store = Arc::new(VersionedStore(Arc::new(InMemory::new())));
        contract(&GcsBlobStore::with_store(store, "gcs")).await;
    }

    #[tokio::test]
    async fn ranged_get_reaches_the_provider_without_fetching_the_full_object() {
        let stats = Arc::new(UploadStats::default());
        let recording = Arc::new(RecordingStore {
            inner: Arc::new(InMemory::new()),
            stats: stats.clone(),
        });
        let store = S3BlobStore::with_store(Arc::new(VersionedStore(recording)), "");
        let key = ObjectKey::new("recorded-range").unwrap();
        let bytes = Bytes::from_static(b"0123456789");
        let stored = store
            .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
            .await
            .unwrap();
        stats.read_ranges.lock().unwrap().clear();

        let read = store
            .get(
                &key,
                &stored.provider_version,
                Some(ByteRange::new(3, 5).unwrap()),
            )
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .concat();

        assert_eq!(read, Bytes::from_static(b"34"));
        assert_eq!(
            *stats.read_ranges.lock().unwrap(),
            vec![None, Some(object_store::GetRange::Bounded(3..5))]
        );
    }

    #[tokio::test]
    async fn pinned_get_halves_provider_requests_for_a_ten_mib_object() {
        let stats = Arc::new(UploadStats::default());
        let recording = Arc::new(RecordingStore {
            inner: Arc::new(InMemory::new()),
            stats: stats.clone(),
        });
        let store = S3BlobStore::with_store(Arc::new(VersionedStore(recording)), "");
        let key = ObjectKey::new("ten-mib-log-segment").unwrap();
        let bytes = Bytes::from(vec![b'x'; 10 * 1024 * 1024]);
        let digest = sha256(&bytes);
        let stored = store
            .put(&key, body_from_bytes(bytes.clone()), digest)
            .await
            .unwrap();
        stats.read_ranges.lock().unwrap().clear();

        let legacy_read = store
            .get(&key, &stored.provider_version, None)
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .concat();
        assert_eq!(legacy_read, bytes);
        assert_eq!(*stats.read_ranges.lock().unwrap(), vec![None, None]);
        stats.read_ranges.lock().unwrap().clear();

        let read = store
            .get_pinned(
                &key,
                &stored.provider_version,
                stored.size,
                stored.sha256,
                None,
            )
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .concat();

        assert_eq!(read, bytes);
        assert_eq!(*stats.read_ranges.lock().unwrap(), vec![None]);
    }

    #[tokio::test]
    async fn pinned_get_validates_recorded_length_and_digest() {
        let store =
            S3BlobStore::with_store(Arc::new(VersionedStore(Arc::new(InMemory::new()))), "");
        let key = ObjectKey::new("pinned-validation").unwrap();
        let bytes = Bytes::from_static(b"validated bytes");
        let stored = store
            .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
            .await
            .unwrap();

        assert!(matches!(
            store
                .get_pinned(
                    &key,
                    &stored.provider_version,
                    stored.size + 1,
                    stored.sha256,
                    None,
                )
                .await,
            Err(BlobStoreError::Interrupted(_))
        ));
        let result = store
            .get_pinned(&key, &stored.provider_version, stored.size, [0; 32], None)
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await;
        assert!(matches!(result, Err(BlobStoreError::DigestMismatch)));
    }

    #[tokio::test]
    async fn cloud_adapters_reject_objects_without_provider_versions() {
        for store in [
            &S3BlobStore::with_store(Arc::new(InMemory::new()), "s3") as &dyn BlobStore,
            &GcsBlobStore::with_store(Arc::new(InMemory::new()), "gcs") as &dyn BlobStore,
        ] {
            let key = ObjectKey::new("unversioned").unwrap();
            let bytes = Bytes::from_static(b"bytes");
            assert!(matches!(
                store
                    .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
                    .await,
                Err(BlobStoreError::Backend(message)) if message.contains("object version")
            ));
        }
    }

    #[test]
    fn provider_versions_reject_null_and_blank_values() {
        for version in ["v:", "v: ", "v:null", "e:", "e:  "] {
            assert!(matches!(
                ProviderVersion::from_stored(version),
                Err(BlobStoreError::InvalidVersion)
            ));
        }
    }

    #[tokio::test]
    async fn gcs_preflight_accepts_generation_aware_operations() {
        let store = GcsBlobStore::with_store(
            Arc::new(VersionedStore(Arc::new(InMemory::new()))),
            "gcs-preflight",
        );

        store.preflight().await.unwrap();
    }

    #[tokio::test]
    async fn interrupted_and_digest_mismatched_writes_publish_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilesystemBlobStore::new(directory.path()).unwrap();
        let key = ObjectKey::new("interrupted").unwrap();
        let interrupted = stream::iter([
            Ok(Bytes::from_static(b"partial")),
            Err(BlobStoreError::Backend("connection lost".into())),
        ])
        .boxed();
        assert!(matches!(
            store.put(&key, interrupted, sha256(b"partial")).await,
            Err(BlobStoreError::Interrupted(_))
        ));
        assert!(store.head(&key).await.unwrap().is_none());

        assert!(matches!(
            store
                .put(&key, body_from_bytes(Bytes::from_static(b"body")), [0; 32])
                .await,
            Err(BlobStoreError::DigestMismatch)
        ));
        assert!(store.head(&key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn unknown_successful_write_is_reconciled() {
        let store = S3BlobStore::with_store(
            Arc::new(VersionedStore(Arc::new(
                UnknownResultStore(InMemory::new()),
            ))),
            "unknown",
        );
        let key = ObjectKey::new("result").unwrap();
        let bytes = Bytes::from_static(b"committed before connection loss");
        let stored = store
            .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
            .await
            .unwrap();

        assert_eq!(stored.sha256, sha256(&bytes));
        assert_eq!(stored.size, bytes.len() as u64);
    }

    #[tokio::test]
    async fn exact_version_and_stored_digest_detect_tampering() {
        let inner = Arc::new(InMemory::new());
        let store = S3BlobStore::with_store(Arc::new(VersionedStore(inner.clone())), "");
        let key = ObjectKey::new("tampered").unwrap();
        let original = Bytes::from_static(b"original");
        let stored = store
            .put(&key, body_from_bytes(original.clone()), sha256(&original))
            .await
            .unwrap();

        let mut attributes = Attributes::new();
        attributes.insert(
            Attribute::Metadata(Cow::Borrowed(DIGEST_METADATA_KEY)),
            encode_digest(&sha256(&original)).into(),
        );
        inner
            .put_opts(
                &Path::from("tampered"),
                Bytes::from_static(b"changed!").into(),
                PutOptions::from(attributes),
            )
            .await
            .unwrap();
        assert!(matches!(
            store.get(&key, &stored.provider_version, None).await,
            Err(BlobStoreError::VersionMismatch)
        ));

        let current = store.head(&key).await.unwrap().unwrap();
        let result = store
            .get(&key, &current.provider_version, None)
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await;
        assert!(matches!(result, Err(BlobStoreError::DigestMismatch)));
    }

    #[tokio::test]
    async fn deletion_refuses_a_different_provider_version() {
        let store =
            S3BlobStore::with_store(Arc::new(VersionedStore(Arc::new(InMemory::new()))), "");
        let key = ObjectKey::new("exact-delete").unwrap();
        let bytes = Bytes::from_static(b"body");
        let stored = store
            .put(&key, body_from_bytes(bytes.clone()), sha256(&bytes))
            .await
            .unwrap();
        let wrong = ProviderVersion::from_stored("v:not-the-recorded-version").unwrap();

        assert!(matches!(
            store.delete(&key, &wrong).await,
            Err(BlobStoreError::VersionMismatch)
        ));
        assert!(store.head(&key).await.unwrap().is_some());
        store.delete(&key, &stored.provider_version).await.unwrap();
        store.delete(&key, &stored.provider_version).await.unwrap();
    }

    #[tokio::test]
    async fn multipart_upload_is_backpressured_and_aborted_on_interruption() {
        let stats = Arc::new(UploadStats::default());
        let recording = Arc::new(RecordingStore {
            inner: Arc::new(InMemory::new()),
            stats: stats.clone(),
        });
        let store = ObjectStoreBlobStore::new(recording, "", false, false);
        let key = ObjectKey::new("bounded").unwrap();
        let chunk = Bytes::from(vec![7_u8; 1024 * 1024]);
        let body = stream::iter(
            (0..12)
                .map(move |_| Ok(chunk.clone()))
                .chain(std::iter::once(Err(BlobStoreError::Interrupted(
                    "lost".into(),
                )))),
        )
        .boxed();

        assert!(matches!(
            store.put(&key, body, [0; 32]).await,
            Err(BlobStoreError::Interrupted(_))
        ));
        assert!(stats.aborted.load(Ordering::SeqCst));
        assert!(stats
            .part_sizes
            .lock()
            .unwrap()
            .iter()
            .all(|size| *size <= MULTIPART_CHUNK_SIZE));
        assert!(
            stats.max_in_flight_bytes.load(Ordering::SeqCst) <= MULTIPART_CHUNK_SIZE,
            "one upload retains at most one {MULTIPART_CHUNK_SIZE}-byte provider part"
        );
        assert!(store.head(&key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn mismatched_multipart_upload_is_aborted() {
        let stats = Arc::new(UploadStats::default());
        let store = ObjectStoreBlobStore::new(
            Arc::new(RecordingStore {
                inner: Arc::new(InMemory::new()),
                stats: stats.clone(),
            }),
            "",
            false,
            false,
        );
        let key = ObjectKey::new("digest-mismatch").unwrap();
        let bytes = Bytes::from(vec![3_u8; MULTIPART_CHUNK_SIZE + 1]);

        assert!(matches!(
            store.put(&key, body_from_bytes(bytes), [0; 32]).await,
            Err(BlobStoreError::DigestMismatch)
        ));
        assert!(stats.aborted.load(Ordering::SeqCst));
        assert!(store.head(&key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn concurrent_upload_provider_memory_scales_by_one_part_each() {
        const TRANSFERS: usize = 4;
        const SOURCE_CHUNKS: usize = 12;
        let stats = Arc::new(UploadStats::default());
        let store = Arc::new(ObjectStoreBlobStore::new(
            Arc::new(RecordingStore {
                inner: Arc::new(InMemory::new()),
                stats: stats.clone(),
            }),
            "",
            true,
            false,
        ));
        let source_chunk = Bytes::from(vec![9_u8; 1024 * 1024]);
        let mut hasher = Sha256::new();
        for _ in 0..SOURCE_CHUNKS {
            hasher.update(&source_chunk);
        }
        let digest = hasher.finalize().into();

        let uploads = (0..TRANSFERS).map(|index| {
            let store = store.clone();
            let source_chunk = source_chunk.clone();
            async move {
                let body = stream::iter(
                    (0..SOURCE_CHUNKS)
                        .map(move |_| Ok(source_chunk.clone()))
                        .collect::<Vec<_>>(),
                )
                .boxed();
                store
                    .put(
                        &ObjectKey::new(format!("concurrent/{index}")).unwrap(),
                        body,
                        digest,
                    )
                    .await
                    .unwrap();
            }
        });
        futures::future::join_all(uploads).await;

        assert!(
            stats.max_in_flight_bytes.load(Ordering::SeqCst) <= TRANSFERS * MULTIPART_CHUNK_SIZE,
            "each concurrent upload retains at most one provider part"
        );
    }

    #[tokio::test]
    async fn verified_reads_are_lazy_and_check_digest_at_eof() {
        let polls = Arc::new(AtomicUsize::new(0));
        let source_polls = polls.clone();
        let source = stream::unfold(0_usize, move |index| {
            let polls = source_polls.clone();
            async move {
                if index == 4 {
                    return None;
                }
                polls.fetch_add(1, Ordering::SeqCst);
                Some((Ok(Bytes::from(vec![index as u8; 64 * 1024])), index + 1))
            }
        })
        .boxed();
        let mut reader = verify_reader(source, 4 * 64 * 1024, Some([0; 32]));

        assert_eq!(polls.load(Ordering::SeqCst), 0);
        for expected_polls in 1..=4 {
            assert_eq!(reader.next().await.unwrap().unwrap().len(), 64 * 1024);
            assert_eq!(polls.load(Ordering::SeqCst), expected_polls);
        }
        assert!(matches!(
            reader.next().await,
            Some(Err(BlobStoreError::DigestMismatch))
        ));
    }

    #[tokio::test]
    async fn filesystem_delete_does_not_remove_a_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilesystemBlobStore::new(directory.path()).unwrap();
        let key = ObjectKey::new("race/object").unwrap();
        let original = Bytes::from_static(b"original");
        let stored = store
            .put(&key, body_from_bytes(original.clone()), sha256(&original))
            .await
            .unwrap();
        let path = directory.path().join(key.as_str());
        tokio::fs::remove_file(&path).await.unwrap();
        tokio::fs::write(&path, b"replacement").await.unwrap();

        assert!(matches!(
            store.delete(&key, &stored.provider_version).await,
            Err(BlobStoreError::VersionMismatch)
        ));
        assert_eq!(tokio::fs::read(path).await.unwrap(), b"replacement");
    }

    #[tokio::test]
    async fn s3_exact_delete_sends_recorded_version_id() {
        let (endpoint, request) = capture_request(204).await;
        let store = object_store::aws::AmazonS3Builder::new()
            .with_bucket_name("bucket")
            .with_region("test-region")
            .with_endpoint(&endpoint)
            .with_allow_http(true)
            .with_access_key_id("access")
            .with_secret_access_key("secret")
            .build()
            .unwrap();
        S3NativeDelete {
            store,
            bucket: "bucket".into(),
            region: "test-region".into(),
            client: reqwest::Client::new(),
        }
        .delete(
            &Path::from("prefix/object"),
            &ProviderVersion::from_stored("v:recorded-version").unwrap(),
        )
        .await
        .unwrap();

        let request = request.await.unwrap().to_ascii_lowercase();
        assert!(request.starts_with("delete /bucket/prefix/object?versionid=recorded-version "));
        assert!(request.contains("\r\nauthorization: aws4-hmac-sha256 "));
    }

    async fn s3_versioning_check(
        status: u16,
        body: &'static str,
    ) -> (Result<(), BlobStoreError>, String) {
        let (endpoint, request) = capture_request_with_body(status, body).await;
        let store = object_store::aws::AmazonS3Builder::new()
            .with_bucket_name("preflight-bucket")
            .with_region("test-region")
            .with_endpoint(&endpoint)
            .with_allow_http(true)
            .with_access_key_id("access")
            .with_secret_access_key("secret")
            .build()
            .unwrap();
        let result = S3NativeDelete {
            store,
            bucket: "preflight-bucket".into(),
            region: "test-region".into(),
            client: reqwest::Client::new(),
        }
        .check_bucket_versioning()
        .await;
        (result, request.await.unwrap())
    }

    #[tokio::test]
    async fn s3_versioning_check_accepts_enabled_bucket() {
        let (result, request) = s3_versioning_check(
            200,
            "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
        )
        .await;

        result.unwrap();
        let request = request.to_ascii_lowercase();
        assert!(
            request.starts_with("get /preflight-bucket/?versioning "),
            "unexpected request: {request:?}"
        );
        assert!(request.contains("\r\nauthorization: aws4-hmac-sha256 "));
    }

    #[tokio::test]
    async fn s3_versioning_check_rejects_disabled_and_suspended_buckets() {
        for (body, expected) in [
            ("<VersioningConfiguration/>", "disabled"),
            (
                "<VersioningConfiguration><Status>Suspended</Status></VersioningConfiguration>",
                "suspended",
            ),
        ] {
            let (result, _) = s3_versioning_check(200, body).await;
            assert!(matches!(
                result,
                Err(BlobStoreError::Backend(message))
                    if message.contains("S3")
                        && message.contains("preflight-bucket")
                        && message.contains(expected)
                        && message.contains("enable S3 bucket versioning")
            ));
        }
    }

    #[tokio::test]
    async fn s3_versioning_check_reports_inaccessible_bucket_without_credentials() {
        let (result, _) = s3_versioning_check(403, "access=secret").await;
        let Err(BlobStoreError::Backend(message)) = result else {
            panic!("expected inaccessible bucket error");
        };
        assert!(message.contains("S3"));
        assert!(message.contains("preflight-bucket"));
        assert!(message.contains("s3:GetBucketVersioning"));
        assert!(!message.contains("access"));
        assert!(!message.contains("secret"));
    }

    #[tokio::test]
    async fn gcs_exact_delete_sends_generation_precondition() {
        let (endpoint, request) = capture_request(204).await;
        let store = object_store::gcp::GoogleCloudStorageBuilder::new()
            .with_bucket_name("bucket")
            .with_base_url(&endpoint)
            .with_bearer_token("token")
            .build()
            .unwrap();
        GcsNativeDelete {
            store,
            bucket: "bucket".into(),
            base_url: endpoint,
            client: reqwest::Client::new(),
        }
        .delete(
            &Path::from("prefix/object"),
            &ProviderVersion::from_stored("v:123456").unwrap(),
        )
        .await
        .unwrap();

        let request = request.await.unwrap().to_ascii_lowercase();
        assert!(request.starts_with("delete /bucket/prefix/object "));
        assert!(request.contains("\r\nx-goog-if-generation-match: 123456\r\n"));
        assert!(request.contains("\r\nauthorization: bearer token\r\n"));
    }

    #[test]
    fn missing_exact_versions_are_idempotent() {
        assert!(map_exact_delete_status(reqwest::StatusCode::NOT_FOUND).is_ok());
    }
}
