//! Trusted control-plane preparation. Hosts submit no paths, image, argv or
//! environment. Durable admission precedes work; FileTransfers owns preparation
//! and native PUT through settlement even if the original HTTP caller leaves.
use crate::{
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::{
    blob_store::{BlobStoreError, ObjectKey, ProviderVersion},
    pack_format,
    repositories::{
        pack_release::PackReleaseRepository, workload_host::ClaimedActionSource,
        workload_launch::WorkloadLaunchRepository as Ledger,
    },
    workload_archive::ArchivePolicy,
    workload_launch::{LaunchInputKind, PinnedLaunchInput, Sha256Digest},
    workload_pack_input::{release_zip, ReleaseInputPin},
};
use futures::{StreamExt, TryStreamExt};
use std::{sync::Arc, time::Duration};
use tokio::io::AsyncWriteExt;
fn unavailable() -> ApiError {
    ApiError::InternalServerError("Pinned workload input preparation unavailable".into())
}

/// Acquire only the execution's immutable provider version. The public release
/// becomes a validated launch ZIP; selected config, parameters and credentials
/// are never passed to this materializer or embedded in the archive.
pub async fn materialize_action_pack(
    state: Arc<AppState>,
    source: &ClaimedActionSource,
) -> ApiResult<PinnedLaunchInput> {
    if let Some(pin) = Ledger::prepared_pack_input(&state.db, source.admission).await? {
        return Ok(pin);
    }
    let snapshot = source
        .execution
        .executable_snapshot
        .as_ref()
        .ok_or_else(unavailable)?;
    let release = PackReleaseRepository::find_by_id(&state.db, snapshot.release.id)
        .await?
        .ok_or_else(unavailable)?;
    if release.digest != snapshot.release.digest
        || release.pack_ref != snapshot.executable.action.pack_ref
    {
        return Err(unavailable());
    }
    let mut limits = pack_format::Limits::default();
    let size = u64::try_from(release.archive_size).map_err(|_| unavailable())?;
    let files = release
        .manifest
        .get("files")
        .and_then(|value| value.as_array())
        .ok_or_else(unavailable)?
        .len() as u64;
    if size == 0 || size > limits.compressed_bytes || files >= limits.entries {
        return Err(unavailable());
    }
    // Canonical release metadata is immutable CP state. These narrower limits
    // reject a body whose declared inventory differs before allocating its ZIP
    // metadata. Admission accounts for both encoded body and directory tables.
    limits.entries = files.checked_add(1).ok_or_else(unavailable)?;
    let manifest_size = serde_json::to_vec(&release.manifest)
        .map_err(|_| unavailable())?
        .len() as u64;
    limits.manifest_bytes = limits
        .manifest_bytes
        .min(manifest_size.checked_add(4096).ok_or_else(unavailable)?);
    let lease = state
        .file_transfers
        .archive_upload(size, limits.entries)
        .map_err(ApiError::InternalServerError)?;
    let transfers = state.file_transfers.clone();
    let admission = source.admission;
    let work = async move {
        let key = ObjectKey::new(release.object_key.ok_or_else(unavailable)?)
            .map_err(|_| unavailable())?;
        let version =
            ProviderVersion::from_stored(release.provider_version.ok_or_else(unavailable)?)
                .map_err(|_| unavailable())?;
        let digest: [u8; 32] = hex::decode(&release.digest)
            .map_err(|_| unavailable())?
            .try_into()
            .map_err(|_| unavailable())?;
        let mut reader = state
            .blob_store
            .get_pinned(&key, &version, size, digest, None)
            .await
            .map_err(|_| unavailable())?;
        let private = tempfile::NamedTempFile::new().map_err(|_| unavailable())?;
        let mut download = tokio::fs::File::from_std(private.reopen().map_err(|_| unavailable())?);
        let mut count = 0u64;
        loop {
            let chunk = tokio::time::timeout(Duration::from_secs(30), reader.try_next())
                .await
                .map_err(|_| unavailable())?
                .map_err(|_| unavailable())?;
            let Some(chunk) = chunk else {
                break;
            };
            count = count
                .checked_add(chunk.len() as u64)
                .filter(|value| *value <= size)
                .ok_or_else(unavailable)?;
            download
                .write_all(&chunk)
                .await
                .map_err(|_| unavailable())?;
        }
        if count != size {
            return Err(unavailable());
        }
        download.flush().await.map_err(|_| unavailable())?;
        drop(download);
        let pin = ReleaseInputPin {
            pack_ref: release.pack_ref,
            size_bytes: size,
            sha256: Sha256Digest::parse(release.digest)?,
        };
        let policy = ArchivePolicy {
            root: LaunchInputKind::Pack,
            max_compressed_bytes: limits.extracted_bytes,
            max_expanded_bytes: limits.extracted_bytes,
            max_files: 50_000,
        };
        let ready = tokio::task::spawn_blocking(move || {
            let input = private.reopen().map_err(|_| unavailable())?;
            release_zip(input, &pin, limits, &policy).map_err(|_| unavailable())
        })
        .await
        .map_err(|_| unavailable())??;
        let permit = Ledger::reserve_pack_input(&state.db, admission, &ready).await?;
        let key = permit.key()?;
        let expected: [u8; 32] = hex::decode(permit.sha256().as_str())
            .map_err(|_| unavailable())?
            .try_into()
            .map_err(|_| unavailable())?;
        // Positive, exact immutable-object evidence can recover a lost PUT ACK.
        // A 404 is not settlement; the durable reservation remains outstanding.
        let existing = state
            .blob_store
            .head(&key)
            .await
            .map_err(|_| unavailable())?;
        let object = match existing {
            Some(object) if object.size == permit.size_bytes() && object.sha256 == expected => {
                object
            }
            Some(_) => return Err(unavailable()),
            None => {
                let body = ready.into_file();
                let file = tokio::fs::File::from_std(body.reopen().map_err(|_| unavailable())?);
                let stream = tokio_util::io::ReaderStream::new(file)
                    .map_err(|_| {
                        BlobStoreError::Backend("Public workload input read failed".into())
                    })
                    .boxed();
                let stored = state
                    .blob_store
                    .put_file_atomic(&key, stream, permit.size_bytes(), expected)
                    .await
                    .map_err(|_| unavailable())?;
                drop(body);
                stored
            }
        };
        Ledger::publish_pack_input(&state.db, &permit, &object)
            .await
            .map_err(Into::into)
    };
    transfers
        .finish_publication(lease, work)
        .await
        .map_err(|_| unavailable())?
}
