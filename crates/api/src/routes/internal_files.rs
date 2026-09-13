//! Internal file transfer endpoints for artifact content distribution.
//!
//! These endpoints allow workers and sensors to upload and download
//! raw file content when they do not share a mounted volume with the API.
//!
//! **Authentication**: Requires a valid JWT (Access, Execution, or Worker token).
//!
//! **Path parameter**: `file_path` is the relative path within `artifacts_dir`,
//! matching what is stored in `artifact_version.file_path`.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, head, post, put},
    Router,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tracing::{debug, info, warn};

use attune_common::artifact_transport::{
    ArtifactFileTransport, ValidatedRelativePath, VolumeTransport,
};
use attune_common::blob_store::{
    body_from_bytes, body_from_file, hash_file, verify_reader, BlobBody, BlobReader,
    BlobStoreError, ByteRange, ObjectKey, ProviderVersion,
};
use attune_common::models::{
    enums::{ArtifactBodyState, LogStreamBackend},
    log_stream::{LogSegment, LogStream},
};
use attune_common::repositories::artifact::{ArtifactRepository, ArtifactVersionRepository};
use attune_common::repositories::log_stream::LogStreamRepository;
use attune_common::repositories::pack_install::PackInstallRepository;
use attune_common::repositories::{
    ExecutionRepository, FindById, FindByRef, ObjectMaintenanceRepository, PackReleaseRepository,
    SensorRepository, SensorWorkloadRepository,
};

use crate::{
    auth::{jwt::TokenType, middleware::AuthenticatedUser, middleware::RequireAuth},
    http_range::{
        insert_range_headers, range_not_satisfiable, range_status, resolve_range, ResolvedRange,
    },
    routes::artifacts::artifact_read_context_for_user,
    state::AppState,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FileOperation {
    Read,
    Mutate,
}

struct SizeLimitedWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
}

impl SizeLimitedWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max_bytes,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl std::io::Write for SizeLimitedWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.len() > self.max_bytes.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other(
                "pack candidate archive exceeds configured size limit",
            ));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn validate_candidate_archive_source(
    root: &std::path::Path,
    max_entries: u32,
    max_entry_bytes: u64,
    max_total_bytes: u64,
) -> std::io::Result<()> {
    let mut directories = vec![root.to_path_buf()];
    let mut entries = 0_u32;
    let mut total_bytes = 0_u64;

    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            entries = entries.saturating_add(1);
            if entries > max_entries {
                return Err(std::io::Error::other(
                    "pack candidate archive has too many entries",
                ));
            }
            if metadata.file_type().is_symlink() {
                return Err(std::io::Error::other(
                    "pack candidate archive contains a symbolic link",
                ));
            }
            if metadata.is_dir() {
                directories.push(path);
            } else if metadata.is_file() {
                if metadata.len() > max_entry_bytes {
                    return Err(std::io::Error::other(
                        "pack candidate archive entry exceeds configured size limit",
                    ));
                }
                total_bytes = total_bytes.saturating_add(metadata.len());
                if total_bytes > max_total_bytes {
                    return Err(std::io::Error::other(
                        "pack candidate archive exceeds configured extracted-size limit",
                    ));
                }
            } else {
                return Err(std::io::Error::other(
                    "pack candidate archive contains a special file",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum FileAuthorizationScope<'a> {
    Worker,
    ExecutionRead,
    ExecutionMutation(i64),
    Sensor(&'a str),
}

fn file_authorization_scope(
    user: &AuthenticatedUser,
    operation: FileOperation,
) -> Result<FileAuthorizationScope<'_>, (StatusCode, String)> {
    match user.claims.token_type {
        TokenType::Worker => Ok(FileAuthorizationScope::Worker),
        TokenType::Execution if operation == FileOperation::Read => {
            Ok(FileAuthorizationScope::ExecutionRead)
        }
        TokenType::Execution => user
            .execution_id()
            .map(FileAuthorizationScope::ExecutionMutation)
            .ok_or_else(|| {
                (
                    StatusCode::FORBIDDEN,
                    "Execution token is missing its execution scope".to_string(),
                )
            }),
        TokenType::Sensor => user
            .claims
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("sensor_ref"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(FileAuthorizationScope::Sensor)
            .ok_or_else(|| {
                (
                    StatusCode::FORBIDDEN,
                    "Sensor token is missing its sensor scope".to_string(),
                )
            }),
        TokenType::Access | TokenType::Refresh => Err((
            StatusCode::FORBIDDEN,
            "Internal file transfer endpoints require execution, sensor, or worker tokens"
                .to_string(),
        )),
    }
}

/// Upload or overwrite a file at the given path.
///
/// The request body is the raw file content.
/// Content-Type header is stored alongside the file if needed.
#[utoipa::path(
    put,
    path = "/api/v1/internal/files/{file_path}",
    tag = "internal",
    params(
        ("file_path" = String, Path, description = "Relative artifact file path")
    ),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 201, description = "File uploaded"),
        (status = 400, description = "Invalid file path"),
        (status = 401, description = "Unauthorized"),
        (status = 413, description = "Payload too large"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn upload_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authorize_file_transfer(&state, &user, &file_path, FileOperation::Mutate).await?;

    let artifacts_dir = &state.config.artifacts_dir;
    let max_size = state.config.artifacts.max_upload_size;

    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");

    if let (Some(size), Some(digest)) = (
        headers
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok()),
        headers
            .get("x-attune-sha256")
            .and_then(|value| value.to_str().ok()),
    ) {
        if size > max_size {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("artifact exceeds maximum size of {max_size} bytes"),
            ));
        }
        if let Some(version) =
            ArtifactVersionRepository::find_unique_by_file_path(&state.db, &file_path)
                .await
                .map_err(map_repository_error)?
                .filter(|version| version.body_state == Some(ArtifactBodyState::Pending))
        {
            use futures::StreamExt;
            let digest = decode_hex_digest(digest)?;
            let body = futures::stream::try_unfold(
                (body.into_data_stream(), 0_u64),
                move |(mut body, received)| async move {
                    match body.next().await {
                        Some(result) => {
                            let chunk = result.map_err(|error| {
                                BlobStoreError::Interrupted(format!(
                                    "artifact upload was interrupted: {error}"
                                ))
                            })?;
                            let received =
                                received.checked_add(chunk.len() as u64).ok_or_else(|| {
                                    BlobStoreError::Backend("artifact size overflow".into())
                                })?;
                            if received > size {
                                return Err(BlobStoreError::Interrupted(format!(
                                    "expected {size} bytes, received more"
                                )));
                            }
                            Ok(Some((chunk, (body, received))))
                        }
                        None if received != size => Err(BlobStoreError::Interrupted(format!(
                            "expected {size} bytes, received {received}"
                        ))),
                        None => Ok(None),
                    }
                },
            );
            publish_stream(&state, &version, body.boxed(), size, digest).await?;
            debug!(
                path = %file_path,
                size,
                content_type = %content_type,
                "File streamed into durable storage via internal endpoint"
            );
            return Ok(StatusCode::CREATED);
        }
    }

    use futures::StreamExt;
    let body = body
        .into_data_stream()
        .map(|result| {
            result.map_err(|error| {
                attune_common::error::Error::Io(format!("artifact upload was interrupted: {error}"))
            })
        })
        .boxed();
    let size = VolumeTransport::new(artifacts_dir)
        .write_stream(&file_path, body, max_size)
        .await
        .map_err(|error| match error {
            attune_common::error::Error::Validation(message) => {
                (StatusCode::PAYLOAD_TOO_LARGE, message)
            }
            other => map_transport_error(other),
        })?;

    debug!(
        path = %file_path,
        size,
        content_type = %content_type,
        "File uploaded via internal endpoint"
    );

    Ok(StatusCode::CREATED)
}

/// Download file content at the given path.
#[utoipa::path(
    get,
    path = "/api/v1/internal/files/{file_path}",
    tag = "internal",
    params(
        ("file_path" = String, Path, description = "Relative artifact file path")
    ),
    responses(
        (status = 200, description = "File content", content_type = "application/octet-stream"),
        (status = 206, description = "Requested byte range", content_type = "application/octet-stream"),
        (status = 416, description = "Requested range is not satisfiable"),
        (status = 400, description = "Invalid file path"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "File not found"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn download_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
    request_headers: HeaderMap,
) -> Result<axum::response::Response, (StatusCode, String)> {
    authorize_file_transfer(&state, &user, &file_path, FileOperation::Read).await?;

    let version = ArtifactVersionRepository::find_unique_by_file_path(&state.db, &file_path)
        .await
        .map_err(map_repository_error)?;
    match version {
        Some(version) => {
            if matches!(
                version.body_state,
                Some(ArtifactBodyState::Deleting | ArtifactBodyState::CleanupClaimed)
            ) {
                return Err((StatusCode::NOT_FOUND, "File not found".to_string()));
            }
            if let Some(stream) =
                LogStreamRepository::find_by_artifact_version(&state.db, version.id)
                    .await
                    .map_err(map_repository_error)?
            {
                if !stream.sealed {
                    return Err((StatusCode::CONFLICT, "Log stream is not sealed".to_string()));
                }
                let size = u64::try_from(stream.total_bytes).map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Invalid log size".to_string(),
                    )
                })?;
                let range = match resolve_range(&request_headers, size) {
                    Ok(range) => range,
                    Err(message) => return Ok(range_not_satisfiable(size, message)),
                };
                return stream_download_response(
                    stream_log_stream(&state, stream.id, range.bytes)
                        .await
                        .map_err(map_log_stream_read_error)?,
                    size,
                    range,
                    &file_path,
                );
            } else if version.body_state == Some(ArtifactBodyState::Ready) {
                let size = u64::try_from(version.size_bytes.ok_or_else(|| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Missing object size".to_string(),
                    )
                })?)
                .map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Invalid object size".to_string(),
                    )
                })?;
                let range = match resolve_range(&request_headers, size) {
                    Ok(range) => range,
                    Err(message) => return Ok(range_not_satisfiable(size, message)),
                };
                return stream_download_response(
                    stream_object_body(&state, &version, range.bytes).await?,
                    size,
                    range,
                    &file_path,
                );
            } else {
                return stream_volume_download(
                    &state.config.artifacts_dir,
                    &file_path,
                    &request_headers,
                )
                .await;
            }
        }
        None => {
            return stream_volume_download(
                &state.config.artifacts_dir,
                &file_path,
                &request_headers,
            )
            .await;
        }
    }
}

async fn stream_volume_download(
    artifacts_dir: &str,
    file_path: &str,
    request_headers: &HeaderMap,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let transport = VolumeTransport::new(artifacts_dir);
    let size = transport
        .file_size(file_path)
        .await
        .map_err(map_transport_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "File not found".to_string()))?;
    let range = match resolve_range(request_headers, size) {
        Ok(range) => range,
        Err(message) => return Ok(range_not_satisfiable(size, message)),
    };
    let reader = transport
        .open_reader(file_path, range.start)
        .await
        .map_err(map_transport_error)?;
    let body = tokio_util::io::ReaderStream::with_capacity(reader.take(range.len()), 64 * 1024);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        mime_from_extension(file_path).parse().unwrap(),
    );
    insert_range_headers(&mut headers, range, size);
    let status = range_status(range);
    Ok((status, headers, Body::from_stream(body)).into_response())
}

fn stream_download_response(
    reader: BlobReader,
    size: u64,
    range: ResolvedRange,
    file_path: &str,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        mime_from_extension(file_path).parse().unwrap(),
    );
    insert_range_headers(&mut headers, range, size);
    let status = range_status(range);
    Ok((status, headers, Body::from_stream(reader)).into_response())
}

async fn complete_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authorize_file_transfer(&state, &user, &file_path, FileOperation::Mutate).await?;
    let version = ArtifactVersionRepository::find_unique_by_file_path(&state.db, &file_path)
        .await
        .map_err(map_repository_error)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "Artifact version not found".to_string(),
            )
        })?;

    if version.body_state == Some(ArtifactBodyState::Ready) {
        let _ = VolumeTransport::new(&state.config.artifacts_dir)
            .delete_file(&file_path)
            .await;
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-attune-size",
            version.size_bytes.unwrap_or(0).to_string().parse().unwrap(),
        );
        return Ok((StatusCode::OK, headers));
    }
    if version.body_state != Some(ArtifactBodyState::Pending) {
        return Err((
            StatusCode::CONFLICT,
            "Artifact body is not pending".to_string(),
        ));
    }

    let staged_path = std::path::Path::new(&state.config.artifacts_dir).join(&file_path);
    publish_file(&state, &version, &staged_path).await?;
    let _ = VolumeTransport::new(&state.config.artifacts_dir)
        .delete_file(&file_path)
        .await;
    let ready = ArtifactVersionRepository::find_by_id(&state.db, version.id)
        .await
        .map_err(map_repository_error)?
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                "Artifact version disappeared during upload".to_string(),
            )
        })?;
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-attune-size",
        ready.size_bytes.unwrap_or(0).to_string().parse().unwrap(),
    );
    Ok((StatusCode::OK, headers))
}

async fn commit_log_segment(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((artifact_version, sequence)): Path<(i64, i64)>,
    body: Body,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let version = authorize_log_version(&state, &user, artifact_version).await?;
    let stream = LogStreamRepository::find_by_artifact_version(&state.db, artifact_version)
        .await
        .map_err(map_repository_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Log stream not found".to_string()))?;
    if stream.backend != LogStreamBackend::ObjectSegments {
        return Err((
            StatusCode::CONFLICT,
            "Shared-file log streams do not accept object segments".to_string(),
        ));
    }
    let bytes = axum::body::to_bytes(body, stream.max_unflushed_bytes as usize)
        .await
        .map_err(|error| (StatusCode::PAYLOAD_TOO_LARGE, error.to_string()))?;
    if bytes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Log segments cannot be empty".to_string(),
        ));
    }
    let digest = Sha256::digest(&bytes);
    let digest_hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    let existing = LogStreamRepository::find_segment_by_sequence(&state.db, stream.id, sequence)
        .await
        .map_err(map_repository_error)?;
    match log_segment_commit_decision(
        existing
            .as_ref()
            .map(|segment| (segment.sha256.as_str(), segment.size_bytes)),
        stream.sealed,
        stream.next_sequence,
        sequence,
        &digest_hex,
        bytes.len() as i64,
    )? {
        LogSegmentCommitDecision::Retry => return Ok(StatusCode::OK),
        LogSegmentCommitDecision::Commit => {}
    }

    let key = ObjectKey::new(format!("logs/{}/segments/{sequence}", stream.id))
        .map_err(map_blob_error)?;
    ObjectMaintenanceRepository::reserve_upload(&state.db, key.as_str(), "log")
        .await
        .map_err(map_repository_error)?;
    let digest_array: [u8; 32] = digest.into();
    let stored = match state
        .blob_store
        .put(&key, body_from_bytes(bytes.clone()), digest_array)
        .await
    {
        Ok(stored) => stored,
        Err(BlobStoreError::Conflict) => {
            let stored = state
                .blob_store
                .head(&key)
                .await
                .map_err(map_blob_error)?
                .ok_or_else(|| {
                    (
                        StatusCode::CONFLICT,
                        "Log segment object write conflicted".to_string(),
                    )
                })?;
            if !stored_object_matches(&stored, bytes.len() as u64, digest_array) {
                return Err((
                    StatusCode::CONFLICT,
                    "Log sequence object contains different bytes".to_string(),
                ));
            }
            stored
        }
        Err(error) => return Err(map_blob_error(error)),
    };

    let mut transaction = state.db.begin().await.map_err(map_sqlx_error)?;
    let locked = LogStreamRepository::lock(&mut transaction, stream.id)
        .await
        .map_err(map_repository_error)?;
    let existing = LogStreamRepository::find_segment(&mut transaction, stream.id, sequence)
        .await
        .map_err(map_repository_error)?;
    match log_segment_commit_decision(
        existing
            .as_ref()
            .map(|segment| (segment.sha256.as_str(), segment.size_bytes)),
        locked.sealed,
        locked.next_sequence,
        sequence,
        &digest_hex,
        bytes.len() as i64,
    )? {
        LogSegmentCommitDecision::Retry => {
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(StatusCode::OK);
        }
        LogSegmentCommitDecision::Commit => {}
    }
    ObjectMaintenanceRepository::record_uploaded(
        &mut *transaction,
        key.as_str(),
        stored.provider_version.as_stored(),
        stored.size as i64,
    )
    .await
    .map_err(map_repository_error)?;
    LogStreamRepository::commit_segment(
        &mut transaction,
        &locked,
        sequence,
        bytes.len() as i64,
        &digest_hex,
        key.as_str(),
        stored.provider_version.as_stored(),
    )
    .await
    .map_err(map_repository_error)?;
    transaction.commit().await.map_err(map_sqlx_error)?;
    debug!(
        artifact_version = version.id,
        stream_id = stream.id,
        sequence,
        bytes = bytes.len(),
        "Committed log segment"
    );
    Ok(StatusCode::CREATED)
}

fn log_segment_retry_matches(
    existing_sha256: &str,
    existing_size: i64,
    retry_sha256: &str,
    retry_size: i64,
) -> bool {
    existing_sha256 == retry_sha256 && existing_size == retry_size
}

#[derive(Debug, Eq, PartialEq)]
enum LogSegmentCommitDecision {
    Retry,
    Commit,
}

fn log_segment_commit_decision(
    existing: Option<(&str, i64)>,
    sealed: bool,
    next_sequence: i64,
    sequence: i64,
    digest: &str,
    size: i64,
) -> Result<LogSegmentCommitDecision, (StatusCode, String)> {
    if let Some((existing_digest, existing_size)) = existing {
        if log_segment_retry_matches(existing_digest, existing_size, digest, size) {
            return Ok(LogSegmentCommitDecision::Retry);
        }
        return Err((
            StatusCode::CONFLICT,
            "Log sequence already contains different bytes".to_string(),
        ));
    }
    if sealed || sequence != next_sequence {
        return Err((
            StatusCode::CONFLICT,
            format!("Expected log sequence {next_sequence}"),
        ));
    }
    Ok(LogSegmentCommitDecision::Commit)
}

#[derive(serde::Deserialize)]
struct SealLogQuery {
    #[serde(default)]
    truncated: bool,
}

async fn seal_log_stream(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(artifact_version): Path<i64>,
    Query(query): Query<SealLogQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let version = authorize_log_version(&state, &user, artifact_version).await?;
    let stream = LogStreamRepository::find_by_artifact_version(&state.db, artifact_version)
        .await
        .map_err(map_repository_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Log stream not found".to_string()))?;
    let shared_snapshot = if stream.backend == LogStreamBackend::SharedFile {
        let file_path = version.file_path.as_deref().ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                "Shared log has no file path".to_string(),
            )
        })?;
        Some(
            VolumeTransport::new(&state.config.artifacts_dir)
                .seal_log_file(file_path)
                .await
                .map_err(map_transport_error)?,
        )
    } else {
        None
    };
    if stream.sealed {
        let unchanged = shared_snapshot.as_ref().is_none_or(|(_, size, digest)| {
            version.size_bytes == i64::try_from(*size).ok()
                && version.sha256.as_deref() == Some(hex_digest(digest).as_str())
        });
        return if unchanged
            && log_stream_seal_is_complete(stream.truncated, query.truncated, version.body_state)
        {
            Ok(StatusCode::OK)
        } else {
            Err((
                StatusCode::CONFLICT,
                "Log stream was already sealed with different state".to_string(),
            ))
        };
    }

    let (size, digest) = match stream.backend {
        LogStreamBackend::ObjectSegments => {
            let segments = LogStreamRepository::segments(&state.db, stream.id)
                .await
                .map_err(map_repository_error)?;
            validate_log_snapshot(&stream, &segments)?;
            let mut reader =
                stream_log_segments(&state, segments, None).map_err(map_log_stream_read_error)?;
            let mut hasher = Sha256::new();
            while let Some(chunk) = futures::StreamExt::next(&mut reader).await {
                hasher.update(chunk.map_err(map_blob_error)?);
            }
            (stream.total_bytes, hex_digest(&hasher.finalize().into()))
        }
        LogStreamBackend::SharedFile => {
            let (_, size, digest) = shared_snapshot.as_ref().expect("shared snapshot");
            let size = i64::try_from(*size).map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Log size overflow".to_string(),
                )
            })?;
            (size, hex_digest(digest))
        }
    };

    let mut transaction = state.db.begin().await.map_err(map_sqlx_error)?;
    let locked = LogStreamRepository::lock(&mut transaction, stream.id)
        .await
        .map_err(map_repository_error)?;
    let current_version =
        ArtifactVersionRepository::find_by_id(&mut *transaction, artifact_version)
            .await
            .map_err(map_repository_error)?
            .ok_or_else(|| {
                (
                    StatusCode::CONFLICT,
                    "Log artifact disappeared while sealing".to_string(),
                )
            })?;
    if locked.sealed {
        if log_stream_seal_is_complete(
            locked.truncated,
            query.truncated,
            current_version.body_state,
        ) {
            transaction.commit().await.map_err(map_sqlx_error)?;
            return Ok(StatusCode::OK);
        }
        return Err((
            StatusCode::CONFLICT,
            "Log stream was concurrently sealed with different state".to_string(),
        ));
    }
    if locked.backend != stream.backend {
        return Err((
            StatusCode::CONFLICT,
            "Log stream backend changed while sealing".to_string(),
        ));
    }
    if locked.backend == LogStreamBackend::ObjectSegments
        && (locked.next_sequence != stream.next_sequence
            || locked.total_bytes != stream.total_bytes)
    {
        return Err((
            StatusCode::CONFLICT,
            "Log stream changed while sealing".to_string(),
        ));
    }
    if current_version.body_state != Some(ArtifactBodyState::Pending) {
        return Err((
            StatusCode::CONFLICT,
            "Log artifact is not pending".to_string(),
        ));
    }
    match locked.backend {
        LogStreamBackend::ObjectSegments => {
            LogStreamRepository::seal(&mut transaction, stream.id, query.truncated)
                .await
                .map_err(map_repository_error)?;
        }
        LogStreamBackend::SharedFile => {
            LogStreamRepository::seal_shared_file(
                &mut transaction,
                stream.id,
                size,
                query.truncated,
            )
            .await
            .map_err(map_repository_error)?;
        }
    }
    let ready = if locked.backend == LogStreamBackend::ObjectSegments {
        ArtifactVersionRepository::mark_body_ready_in_transaction(
            &mut transaction,
            artifact_version,
            &format!("segments:{}", locked.next_sequence),
            size,
            &digest,
        )
        .await
    } else {
        ArtifactVersionRepository::mark_log_body_ready_in_transaction(
            &mut transaction,
            artifact_version,
            size,
            &digest,
        )
        .await
    }
    .map_err(map_repository_error)?;
    if ready.is_none() {
        return Err((
            StatusCode::CONFLICT,
            "Log artifact could not be marked ready".to_string(),
        ));
    }
    let artifact_updated =
        ArtifactRepository::update_size_bytes(&mut *transaction, version.artifact, size)
            .await
            .map_err(map_repository_error)?;
    if !artifact_updated {
        return Err((
            StatusCode::CONFLICT,
            "Log artifact disappeared while sealing".to_string(),
        ));
    }
    transaction.commit().await.map_err(map_sqlx_error)?;
    info!(
        artifact_version,
        stream_id = stream.id,
        backend = ?locked.backend,
        segments = locked.next_sequence,
        bytes = size,
        truncated = query.truncated,
        "Sealed runtime log stream"
    );
    Ok(StatusCode::OK)
}

fn log_stream_seal_is_complete(
    sealed_truncated: bool,
    requested_truncated: bool,
    body_state: Option<ArtifactBodyState>,
) -> bool {
    sealed_truncated == requested_truncated && body_state == Some(ArtifactBodyState::Ready)
}

fn validate_log_snapshot(
    stream: &LogStream,
    segments: &[LogSegment],
) -> Result<(), (StatusCode, String)> {
    let mut offset = 0_i64;
    for (expected_sequence, segment) in segments.iter().enumerate() {
        let expected_end = offset.checked_add(segment.size_bytes).ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                "Log segment offsets overflow".to_string(),
            )
        })?;
        if segment.sequence != expected_sequence as i64
            || segment.byte_start != offset
            || segment.byte_end != expected_end
        {
            return Err((
                StatusCode::CONFLICT,
                "Log segments are not contiguous and ordered".to_string(),
            ));
        }
        offset = segment.byte_end;
    }
    if stream.next_sequence != segments.len() as i64 || stream.total_bytes != offset {
        return Err((
            StatusCode::CONFLICT,
            "Log stream metadata does not match its segments".to_string(),
        ));
    }
    Ok(())
}

async fn authorize_log_version(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    artifact_version: i64,
) -> Result<attune_common::models::artifact_version::ArtifactVersion, (StatusCode, String)> {
    let version = ArtifactVersionRepository::find_by_id(&state.db, artifact_version)
        .await
        .map_err(map_repository_error)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "Artifact version not found".to_string(),
            )
        })?;
    let file_path = version.file_path.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Log artifact version has no authorization path".to_string(),
        )
    })?;
    authorize_file_transfer(state, user, file_path, FileOperation::Mutate).await?;
    Ok(version)
}

pub(crate) async fn publish_file(
    state: &AppState,
    version: &attune_common::models::artifact_version::ArtifactVersion,
    path: &std::path::Path,
) -> Result<(), (StatusCode, String)> {
    let (size, digest) = hash_file(path).await.map_err(map_blob_error)?;
    let body = body_from_file(path).await.map_err(map_blob_error)?;
    publish_stream(state, version, body, size, digest).await
}

async fn publish_stream(
    state: &AppState,
    version: &attune_common::models::artifact_version::ArtifactVersion,
    body: BlobBody,
    size: u64,
    digest: [u8; 32],
) -> Result<(), (StatusCode, String)> {
    let key = ObjectKey::new(version.object_key.clone().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Pending artifact has no object key".to_string(),
        )
    })?)
    .map_err(map_blob_error)?;
    ObjectMaintenanceRepository::reserve_upload(&state.db, key.as_str(), "artifact")
        .await
        .map_err(map_repository_error)?;
    let stored = match state.blob_store.put(&key, body, digest).await {
        Ok(stored) => stored,
        Err(BlobStoreError::Conflict) => {
            let stored = state
                .blob_store
                .head(&key)
                .await
                .map_err(map_blob_error)?
                .ok_or_else(|| {
                    (
                        StatusCode::CONFLICT,
                        "Artifact object write conflicted".to_string(),
                    )
                })?;
            if !stored_object_matches(&stored, size, digest) {
                return Err((
                    StatusCode::CONFLICT,
                    "Artifact object key already contains different bytes".to_string(),
                ));
            }
            stored
        }
        Err(error) => return Err(map_blob_error(error)),
    };
    if !stored_object_matches(&stored, size, digest) {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Artifact object verification failed".to_string(),
        ));
    }
    let digest_hex = hex_digest(&digest);
    ObjectMaintenanceRepository::record_uploaded(
        &state.db,
        key.as_str(),
        stored.provider_version.as_stored(),
        stored.size as i64,
    )
    .await
    .map_err(map_repository_error)?;
    let ready = ArtifactVersionRepository::mark_body_ready(
        &state.db,
        version.id,
        stored.provider_version.as_stored(),
        stored.size as i64,
        &digest_hex,
    )
    .await
    .map_err(map_repository_error)?;
    if ready.is_none() {
        let current = ArtifactVersionRepository::find_by_id(&state.db, version.id)
            .await
            .map_err(map_repository_error)?;
        if !current.is_some_and(|row| {
            row.body_state == Some(ArtifactBodyState::Ready)
                && row.object_key.as_deref() == Some(key.as_str())
                && row.provider_version.as_deref() == Some(stored.provider_version.as_stored())
                && row.size_bytes == Some(stored.size as i64)
                && row.sha256.as_deref() == Some(&digest_hex)
        }) {
            return Err((
                StatusCode::CONFLICT,
                "Artifact body state changed during upload".to_string(),
            ));
        }
    }
    ArtifactRepository::update_size_bytes(&state.db, version.artifact, stored.size as i64)
        .await
        .map_err(map_repository_error)?;
    Ok(())
}

pub(crate) async fn stream_object_body(
    state: &AppState,
    version: &attune_common::models::artifact_version::ArtifactVersion,
    range: Option<ByteRange>,
) -> Result<BlobReader, (StatusCode, String)> {
    if let Some(stream) = LogStreamRepository::find_by_artifact_version(&state.db, version.id)
        .await
        .map_err(map_repository_error)?
    {
        if !stream.sealed {
            return Err((StatusCode::CONFLICT, "Log stream is not sealed".to_string()));
        }
        return stream_log_stream(state, stream.id, range)
            .await
            .map_err(map_log_stream_read_error);
    }
    let key = ObjectKey::new(version.object_key.clone().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Ready artifact has no object key".to_string(),
        )
    })?)
    .map_err(map_blob_error)?;
    let provider_version =
        ProviderVersion::from_stored(version.provider_version.clone().ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Ready artifact has no provider version".to_string(),
            )
        })?)
        .map_err(map_blob_error)?;
    let size = u64::try_from(version.size_bytes.ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Ready artifact has no recorded size".to_string(),
        )
    })?)
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Ready artifact has a negative recorded size".to_string(),
        )
    })?;
    let digest = decode_hex_digest(version.sha256.as_deref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Ready artifact has no recorded SHA-256".to_string(),
        )
    })?)?;
    state
        .blob_store
        .get_pinned(&key, &provider_version, size, digest, range)
        .await
        .map_err(map_blob_error)
}

pub(crate) async fn stream_log_stream(
    state: &AppState,
    stream_id: i64,
    range: Option<ByteRange>,
) -> Result<BlobReader, LogStreamReadError> {
    let stream = LogStreamRepository::find_by_id_in_pool(&state.db, stream_id).await?;
    if stream.backend == LogStreamBackend::SharedFile {
        let snapshot = resolve_shared_file_log_snapshot(state, &stream).await?;
        return stream_shared_file_log(&state.config.artifacts_dir, stream.sealed, snapshot, range)
            .await;
    }

    let segments = match range {
        Some(range) => {
            let start = i64::try_from(range.start).map_err(|_| LogStreamReadError::SizeOverflow)?;
            let end = i64::try_from(range.end).map_err(|_| LogStreamReadError::SizeOverflow)?;
            LogStreamRepository::segments_in_byte_range(&state.db, stream_id, start, end).await?
        }
        None => LogStreamRepository::segments(&state.db, stream_id).await?,
    };
    stream_log_segments(state, segments, range)
}

pub(crate) struct SharedFileLogSnapshot {
    file_path: String,
    pub(crate) size: u64,
    sha256: Option<String>,
}

pub(crate) async fn resolve_shared_file_log_snapshot(
    state: &AppState,
    stream: &LogStream,
) -> Result<SharedFileLogSnapshot, LogStreamReadError> {
    let version = ArtifactVersionRepository::find_by_id(&state.db, stream.artifact_version)
        .await?
        .ok_or(LogStreamReadError::MissingArtifact)?;
    let file_path = version
        .file_path
        .ok_or(LogStreamReadError::MissingSharedFilePath)?;
    let size = if stream.sealed {
        u64::try_from(stream.total_bytes).map_err(|_| LogStreamReadError::NegativeStreamSize)?
    } else {
        VolumeTransport::new(&state.config.artifacts_dir)
            .file_size(&file_path)
            .await
            .map_err(LogStreamReadError::Repository)?
            .ok_or(LogStreamReadError::MissingSharedFile)?
    };
    Ok(SharedFileLogSnapshot {
        file_path,
        size,
        sha256: version.sha256,
    })
}

pub(crate) async fn stream_shared_file_log(
    artifacts_dir: &str,
    sealed: bool,
    snapshot: SharedFileLogSnapshot,
    range: Option<ByteRange>,
) -> Result<BlobReader, LogStreamReadError> {
    use futures::{StreamExt, TryStreamExt};

    let selected = range.unwrap_or(ByteRange {
        start: 0,
        end: snapshot.size,
    });
    let end = selected.end.min(snapshot.size);
    if selected.start >= end {
        return Ok(futures::stream::empty().boxed());
    }
    let reader = VolumeTransport::new(artifacts_dir)
        .open_reader(&snapshot.file_path, selected.start)
        .await?;
    let reader =
        tokio_util::io::ReaderStream::with_capacity(reader.take(end - selected.start), 64 * 1024)
            .map_err(|error| BlobStoreError::Interrupted(error.to_string()))
            .boxed();
    let whole_stream = selected.start == 0 && end == snapshot.size;
    let digest = if sealed && whole_stream {
        Some(decode_hex_digest_blob(
            snapshot
                .sha256
                .as_deref()
                .ok_or(BlobStoreError::DigestMismatch)?,
        )?)
    } else {
        None
    };
    Ok(verify_reader(reader, end - selected.start, digest))
}

pub(crate) fn stream_log_segments(
    state: &AppState,
    segments: Vec<LogSegment>,
    range: Option<ByteRange>,
) -> Result<BlobReader, LogStreamReadError> {
    use futures::{StreamExt, TryStreamExt};
    let mut selected = Vec::new();
    for segment in segments {
        let size = u64::try_from(segment.size_bytes)
            .map_err(|_| LogStreamReadError::NegativeSegmentSize)?;
        let segment_start = u64::try_from(segment.byte_start)
            .map_err(|_| LogStreamReadError::NegativeSegmentOffset)?;
        let segment_end = u64::try_from(segment.byte_end)
            .map_err(|_| LogStreamReadError::NegativeSegmentOffset)?;
        if segment_end.checked_sub(segment_start) != Some(size) {
            return Err(LogStreamReadError::InconsistentSegmentRange);
        }
        let requested = range.unwrap_or(ByteRange {
            start: 0,
            end: u64::MAX,
        });
        let start = requested.start.max(segment_start);
        let end = requested.end.min(segment_end);
        if start < end {
            selected.push((
                segment,
                ByteRange::new(start - segment_start, end - segment_start)?,
                size,
            ));
        }
    }
    let blob_store = state.blob_store.clone();
    Ok(futures::stream::iter(selected)
        .then(move |(segment, selected_range, size)| {
            let blob_store = blob_store.clone();
            async move {
                let key = ObjectKey::new(segment.object_key)?;
                let version = ProviderVersion::from_stored(segment.provider_version)?;
                let digest = decode_hex_digest_blob(&segment.sha256)?;
                let whole_segment = selected_range.start == 0 && selected_range.end == size;
                let provider_range = (!whole_segment).then_some(selected_range);
                blob_store
                    .get_pinned(&key, &version, size, digest, provider_range)
                    .await
            }
        })
        .try_flatten()
        .boxed())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LogStreamReadError {
    #[error(transparent)]
    Repository(#[from] attune_common::error::Error),
    #[error(transparent)]
    Blob(#[from] BlobStoreError),
    #[error("log artifact not found")]
    MissingArtifact,
    #[error("shared log has no file path")]
    MissingSharedFilePath,
    #[error("shared log file not found")]
    MissingSharedFile,
    #[error("log segment has a negative recorded size")]
    NegativeSegmentSize,
    #[error("log segment has a negative recorded byte offset")]
    NegativeSegmentOffset,
    #[error("log segment byte range does not match its recorded size")]
    InconsistentSegmentRange,
    #[error("log stream has a negative recorded size")]
    NegativeStreamSize,
    #[error("log stream size overflow")]
    SizeOverflow,
}

pub(crate) fn map_log_stream_read_error(error: LogStreamReadError) -> (StatusCode, String) {
    match error {
        LogStreamReadError::Repository(error) => map_repository_error(error),
        LogStreamReadError::Blob(error) => map_blob_error(error),
        error @ (LogStreamReadError::MissingArtifact | LogStreamReadError::MissingSharedFile) => {
            (StatusCode::NOT_FOUND, error.to_string())
        }
        error @ (LogStreamReadError::MissingSharedFilePath
        | LogStreamReadError::NegativeSegmentSize
        | LogStreamReadError::NegativeSegmentOffset
        | LogStreamReadError::InconsistentSegmentRange
        | LogStreamReadError::NegativeStreamSize
        | LogStreamReadError::SizeOverflow) => {
            (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

fn decode_hex_digest(value: &str) -> Result<[u8; 32], (StatusCode, String)> {
    decode_hex_digest_blob(value).map_err(map_blob_error)
}

fn decode_hex_digest_blob(value: &str) -> Result<[u8; 32], BlobStoreError> {
    let decoded = hex::decode(value).map_err(|_| BlobStoreError::DigestMismatch)?;
    decoded
        .try_into()
        .map_err(|_| BlobStoreError::DigestMismatch)
}

fn hex_digest(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn stored_object_matches(
    stored: &attune_common::blob_store::StoredObject,
    expected_size: u64,
    expected_sha256: [u8; 32],
) -> bool {
    stored.size == expected_size && stored.sha256 == expected_sha256
}

fn map_blob_error(error: BlobStoreError) -> (StatusCode, String) {
    match error {
        BlobStoreError::NotFound => (
            StatusCode::NOT_FOUND,
            "Artifact object not found".to_string(),
        ),
        BlobStoreError::Conflict | BlobStoreError::VersionMismatch => {
            (StatusCode::CONFLICT, error.to_string())
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

/// Check file existence and return size via HEAD request.
#[utoipa::path(
    head,
    path = "/api/v1/internal/files/{file_path}",
    tag = "internal",
    params(
        ("file_path" = String, Path, description = "Relative artifact file path")
    ),
    responses(
        (status = 200, description = "File exists; size is returned in Content-Length"),
        (status = 400, description = "Invalid file path"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "File not found"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn check_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    authorize_file_transfer(&state, &user, &file_path, FileOperation::Read)
        .await
        .map_err(|(status, _)| status)?;

    let version = ArtifactVersionRepository::find_unique_by_file_path(&state.db, &file_path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(version) = version {
        if matches!(
            version.body_state,
            Some(ArtifactBodyState::Deleting | ArtifactBodyState::CleanupClaimed)
        ) {
            return Err(StatusCode::NOT_FOUND);
        }
        if let Some(stream) = LogStreamRepository::find_by_artifact_version(&state.db, version.id)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_LENGTH,
                stream.total_bytes.to_string().parse().unwrap(),
            );
            headers.insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
            return Ok((StatusCode::OK, headers));
        }
        if version.body_state == Some(ArtifactBodyState::Ready) {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_LENGTH,
                version.size_bytes.unwrap_or(0).to_string().parse().unwrap(),
            );
            headers.insert(
                header::CONTENT_TYPE,
                mime_from_extension(&file_path).parse().unwrap(),
            );
            return Ok((StatusCode::OK, headers));
        }
    }

    match VolumeTransport::new(&state.config.artifacts_dir)
        .file_size(&file_path)
        .await
    {
        Ok(Some(size)) => {
            let mut headers = HeaderMap::new();
            headers.insert("Content-Length", size.to_string().parse().unwrap());
            let content_type = mime_from_extension(&file_path);
            headers.insert("Content-Type", content_type.parse().unwrap());
            Ok((StatusCode::OK, headers))
        }
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(error) => Err(map_transport_error(error).0),
    }
}

/// Delete a file. Returns 204 on success, 404 if not found.
async fn delete_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authorize_file_transfer(&state, &user, &file_path, FileOperation::Mutate).await?;

    let artifacts_dir = &state.config.artifacts_dir;

    let transport = VolumeTransport::new(artifacts_dir);
    let existed = transport
        .file_exists(&file_path)
        .await
        .map_err(map_transport_error)?;
    if !existed {
        return Err((StatusCode::NOT_FOUND, "File not found".to_string()));
    }
    match transport.delete_file(&file_path).await {
        Ok(()) => {
            debug!(path = %file_path, "File deleted via internal endpoint");
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) => {
            warn!("Failed to delete file {file_path}: {e}");
            Err(map_transport_error(e))
        }
    }
}

/// Guess MIME type from file extension.
fn mime_from_extension(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("txt" | "log") => "text/plain",
        Some("json") => "application/json",
        Some("yaml" | "yml") => "text/yaml",
        Some("html" | "htm") => "text/html",
        Some("csv") => "text/csv",
        Some("xml") => "application/xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("tar") => "application/x-tar",
        Some("gz") => "application/gzip",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Create internal file transfer routes.
///
/// These are mounted under `/api/v1/internal/files/` in the main router.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/internal/files/{*file_path}", get(download_file))
        .route("/internal/files/{*file_path}", put(upload_file))
        .route("/internal/files/{*file_path}", head(check_file))
        .route("/internal/files/{*file_path}", delete(delete_file_handler))
        .route(
            "/internal/artifacts/complete/{*file_path}",
            post(complete_file),
        )
        .route(
            "/internal/logs/{artifact_version}/segments/{sequence}",
            put(commit_log_segment),
        )
        .route(
            "/internal/logs/{artifact_version}/seal",
            post(seal_log_stream),
        )
        .route(
            "/internal/packs/{pack_ref}/archive",
            get(download_pack_archive),
        )
        .route(
            "/internal/pack-releases/{release_id}/archive",
            get(download_pack_release_archive),
        )
        .route(
            "/internal/pack-installs/{pack_install_id}/archive",
            get(download_pack_install_candidate_archive),
        )
}

/// Wrapper to avoid conflict with the `delete` import from axum::routing
#[utoipa::path(
    delete,
    path = "/api/v1/internal/files/{file_path}",
    tag = "internal",
    params(
        ("file_path" = String, Path, description = "Relative artifact file path")
    ),
    responses(
        (status = 204, description = "File deleted"),
        (status = 400, description = "Invalid file path"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "File not found"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn delete_file_handler(
    state: State<Arc<AppState>>,
    user: RequireAuth,
    path: Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    delete_file(state, user, path).await
}

/// Return the deterministic archive for a pack's active release.
///
/// Used by remote workers/sensors to download pack contents when they
/// don't share a mounted volume with the API.
#[utoipa::path(
    get,
    path = "/api/v1/internal/packs/{pack_ref}/archive",
    tag = "internal",
    params(
        ("pack_ref" = String, Path, description = "Pack reference identifier")
    ),
    responses(
        (status = 200, description = "Pack archive", content_type = "application/gzip"),
        (status = 206, description = "Requested pack archive byte range", content_type = "application/gzip"),
        (status = 416, description = "Requested range is not satisfiable"),
        (status = 400, description = "Invalid pack reference"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Pack not found"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn download_pack_archive(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
    request_headers: HeaderMap,
) -> Result<axum::response::Response, (StatusCode, String)> {
    validate_pack_archive_ref(&pack_ref)?;
    authorize_pack_archive(&state, &user, &pack_ref).await?;

    let release = PackReleaseRepository::find_active_by_pack_ref(&state.db, &pack_ref)
        .await
        .map_err(|error| {
            warn!(%error, %pack_ref, "Failed to resolve active pack release");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to resolve active pack release".to_string(),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Pack '{}' has no active release", pack_ref),
            )
        })?;
    let size = pack_release_size(&release)?;
    let range = match resolve_range(&request_headers, size) {
        Ok(range) => range,
        Err(message) => return Ok(range_not_satisfiable(size, message)),
    };
    let tarball = read_pack_release_archive(&state, &release, range.bytes).await?;
    debug!(%pack_ref, release_id = release.id, digest = %release.digest, "Streaming active pack release archive");

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/gzip".parse().unwrap());
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{pack_ref}.tar.gz\"")
            .parse()
            .unwrap(),
    );
    headers.insert(
        header::HeaderName::from_static("x-attune-pack-release-id"),
        release.id.to_string().parse().unwrap(),
    );
    headers.insert(
        header::HeaderName::from_static("x-attune-pack-release-sha256"),
        release.digest.parse().unwrap(),
    );
    insert_range_headers(&mut headers, range, size);
    Ok((range_status(range), headers, Body::from_stream(tarball)).into_response())
}

pub(crate) async fn download_pack_release_archive(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(release_id): Path<i64>,
    request_headers: HeaderMap,
) -> Result<axum::response::Response, (StatusCode, String)> {
    if release_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid pack release ID".to_string(),
        ));
    }
    let release = PackReleaseRepository::find_by_id(&state.db, release_id)
        .await
        .map_err(map_pack_archive_repository_error)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Pack release not found".to_string()))?;
    authorize_pack_release_archive(&state, &user, release.id).await?;
    let size = pack_release_size(&release)?;
    let range = match resolve_range(&request_headers, size) {
        Ok(range) => range,
        Err(message) => return Ok(range_not_satisfiable(size, message)),
    };
    let tarball = read_pack_release_archive(&state, &release, range.bytes).await?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/gzip".parse().unwrap());
    headers.insert(
        header::HeaderName::from_static("x-attune-pack-release-id"),
        release.id.to_string().parse().unwrap(),
    );
    headers.insert(
        header::HeaderName::from_static("x-attune-pack-release-sha256"),
        release.digest.parse().unwrap(),
    );
    insert_range_headers(&mut headers, range, size);
    Ok((range_status(range), headers, Body::from_stream(tarball)).into_response())
}

async fn read_pack_release_archive(
    state: &AppState,
    release: &attune_common::models::PackRelease,
    range: Option<ByteRange>,
) -> Result<BlobReader, (StatusCode, String)> {
    let key = ObjectKey::new(release.object_key.clone().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Pack release has no object key".to_string(),
        )
    })?)
    .map_err(map_pack_blob_error)?;
    let provider_version =
        ProviderVersion::from_stored(release.provider_version.clone().ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Pack release has no provider version".to_string(),
            )
        })?)
        .map_err(map_pack_blob_error)?;
    let reader = state
        .blob_store
        .get(&key, &provider_version, range)
        .await
        .map_err(map_pack_blob_error)?;
    let size = pack_release_size(release)?;
    let digest = decode_hex_digest(&release.digest)?;
    Ok(verify_reader(
        reader,
        range.map(|range| range.end - range.start).unwrap_or(size),
        range.is_none().then_some(digest),
    ))
}

fn pack_release_size(
    release: &attune_common::models::PackRelease,
) -> Result<u64, (StatusCode, String)> {
    u64::try_from(release.archive_size).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Pack release has a negative archive size".to_string(),
        )
    })
}

fn map_pack_blob_error(error: BlobStoreError) -> (StatusCode, String) {
    let status = match error {
        BlobStoreError::NotFound => StatusCode::NOT_FOUND,
        BlobStoreError::VersionMismatch => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        format!("Failed to read pack release object: {error}"),
    )
}

/// Stream the staged candidate for a pending pack-install test.
#[utoipa::path(
    get,
    path = "/api/v1/internal/pack-installs/{pack_install_id}/archive",
    tag = "internal",
    params(
        ("pack_install_id" = i64, Path, description = "Pack install tracking ID"),
        ("x-attune-pack-candidate-token" = String, Header, description = "Attempt-scoped candidate access token")
    ),
    responses(
        (status = 200, description = "Candidate pack archive", content_type = "application/gzip"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Candidate not found")
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn download_pack_install_candidate_archive(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_install_id): Path<i64>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let worker_id = pack_candidate_worker_id(&user)?;
    if pack_install_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid pack install ID".to_string(),
        ));
    }
    let install = PackInstallRepository::new(state.db.clone())
        .find_by_id(pack_install_id)
        .await
        .map_err(|error| {
            warn!(%error, pack_install_id, "Failed to load pack install candidate");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to load pack install candidate".to_string(),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "Pack install candidate not found".to_string(),
            )
        })?;
    if !matches!(install.status.as_str(), "pending" | "running") {
        return Err((
            StatusCode::NOT_FOUND,
            "Pack install candidate not found".to_string(),
        ));
    }
    if install.started_at
        + chrono::Duration::seconds(
            attune_common::repositories::pack_install::PACK_INSTALL_ACTIVE_TTL_SECS,
        )
        < chrono::Utc::now()
    {
        return Err((
            StatusCode::NOT_FOUND,
            "Pack install candidate not found".to_string(),
        ));
    }
    if install.assigned_worker_id != Some(worker_id) {
        return Err((
            StatusCode::NOT_FOUND,
            "Pack install candidate not found".to_string(),
        ));
    }
    let expected_hash = install
        .candidate_access_token_hash
        .as_deref()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "Pack install candidate not found".to_string(),
            )
        })?;
    authorize_pack_candidate_token(&headers, expected_hash)?;
    validate_pack_archive_ref(&install.pack_ref).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Pack install candidate has an invalid pack reference".to_string(),
        )
    })?;
    let candidate_dir = std::path::Path::new(&state.config.packs_base_dir)
        .join(format!(".pack-test-{pack_install_id}"));
    if !candidate_dir.is_dir()
        || std::fs::symlink_metadata(&candidate_dir)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err((
            StatusCode::NOT_FOUND,
            "Pack install candidate not found".to_string(),
        ));
    }

    let pack_ref = install.pack_ref;
    let archive_pack_ref = pack_ref.clone();
    let max_total_bytes = state
        .config
        .pack_upload
        .max_extracted_size_bytes()
        .min(attune_common::config::PackUploadConfig::DEFAULT_MAX_EXTRACTED_SIZE_BYTES);
    let max_entries = state
        .config
        .pack_upload
        .max_file_count()
        .min(attune_common::config::PackUploadConfig::DEFAULT_MAX_FILE_COUNT);
    let max_entry_bytes = state
        .config
        .pack_upload
        .max_per_entry_size_bytes()
        .min(attune_common::config::PackUploadConfig::DEFAULT_MAX_PER_ENTRY_SIZE_BYTES);
    let max_archive_bytes = max_total_bytes
        .saturating_add(u64::from(max_entries).saturating_mul(1024))
        .saturating_add(1024 * 1024)
        .min(usize::MAX as u64) as usize;
    let tarball = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        validate_candidate_archive_source(
            &candidate_dir,
            max_entries,
            max_entry_bytes,
            max_total_bytes,
        )?;
        let encoder = GzEncoder::new(
            SizeLimitedWriter::new(max_archive_bytes),
            Compression::fast(),
        );
        let mut tar_builder = tar::Builder::new(encoder);
        tar_builder.follow_symlinks(false);
        tar_builder.append_dir_all(&archive_pack_ref, &candidate_dir)?;
        tar_builder.finish()?;
        Ok(tar_builder.into_inner()?.finish()?.into_inner())
    })
    .await
    .map_err(|error| {
        warn!(%error, pack_install_id, "Candidate pack archive task panicked");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error building candidate archive".to_string(),
        )
    })?
    .map_err(|error| {
        warn!(%error, pack_install_id, "Failed to build pack install candidate archive");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to build candidate archive".to_string(),
        )
    })?;
    let headers = [
        (
            axum::http::header::CONTENT_TYPE,
            "application/gzip".to_string(),
        ),
        (
            axum::http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}.tar.gz\"", pack_ref),
        ),
    ];
    Ok((StatusCode::OK, headers, tarball))
}

fn authorize_pack_candidate_token(
    headers: &HeaderMap,
    expected_hash: &str,
) -> Result<(), (StatusCode, String)> {
    let candidate_access_token = headers
        .get("x-attune-pack-candidate-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                "Pack install candidate token is required".to_string(),
            )
        })?;
    if attune_common::auth::hash_integration_token(candidate_access_token) != expected_hash {
        return Err((
            StatusCode::NOT_FOUND,
            "Pack install candidate not found".to_string(),
        ));
    }
    Ok(())
}

fn validate_pack_archive_ref(pack_ref: &str) -> Result<(), (StatusCode, String)> {
    use std::path::Component;

    let mut components = std::path::Path::new(pack_ref).components();
    if !matches!(components.next(), Some(Component::Normal(component)) if component == pack_ref)
        || components.next().is_some()
        || pack_ref.starts_with('.')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid pack_ref: expected one non-hidden path component".to_string(),
        ));
    }
    attune_common::schema::RefValidator::validate_pack_ref(pack_ref).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid pack_ref: {error}"),
        )
    })
}

fn scoped_metadata_value<'a>(
    user: &'a AuthenticatedUser,
    key: &str,
    token_name: &str,
) -> Result<&'a str, (StatusCode, String)> {
    user.claims
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                format!("{token_name} token is missing its {key} scope"),
            )
        })
}

fn pack_candidate_worker_id(user: &AuthenticatedUser) -> Result<i64, (StatusCode, String)> {
    if user.claims.token_type != TokenType::Worker {
        return Err((
            StatusCode::FORBIDDEN,
            "Pack install candidate archives require a worker token".to_string(),
        ));
    }
    scoped_metadata_value(user, "worker_id", "Worker")?
        .parse::<i64>()
        .ok()
        .filter(|worker_id| *worker_id > 0)
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                "Worker token has an invalid worker_id scope".to_string(),
            )
        })
}

async fn authorize_pack_archive(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    pack_ref: &str,
) -> Result<(), (StatusCode, String)> {
    let allowed = match user.claims.token_type {
        TokenType::Worker => {
            scoped_metadata_value(user, "worker_id", "Worker")?;
            true
        }
        TokenType::Execution => {
            let execution_id = user.execution_id().ok_or_else(|| {
                (
                    StatusCode::FORBIDDEN,
                    "Execution token is missing its execution scope".to_string(),
                )
            })?;
            let execution = ExecutionRepository::find_by_id(&state.db, execution_id)
                .await
                .map_err(map_pack_archive_repository_error)?
                .ok_or_else(pack_archive_scope_forbidden)?;
            pack_ref_from_component_ref(&execution.action_ref) == Some(pack_ref)
        }
        TokenType::Sensor => {
            let sensor_ref = scoped_metadata_value(user, "sensor_ref", "Sensor")?;
            let sensor = SensorRepository::find_by_ref(&state.db, sensor_ref)
                .await
                .map_err(map_pack_archive_repository_error)?
                .ok_or_else(pack_archive_scope_forbidden)?;
            sensor.pack_ref.as_deref() == Some(pack_ref)
        }
        TokenType::Access | TokenType::Refresh => false,
    };

    if allowed {
        Ok(())
    } else {
        Err(pack_archive_scope_forbidden())
    }
}

async fn authorize_pack_release_archive(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    release_id: i64,
) -> Result<(), (StatusCode, String)> {
    let pinned_release = match user.claims.token_type {
        TokenType::Worker => {
            scoped_metadata_value(user, "worker_id", "Worker")?;
            return Ok(());
        }
        TokenType::Execution => {
            let execution_id = user.execution_id().ok_or_else(|| {
                (
                    StatusCode::FORBIDDEN,
                    "Execution token is missing its execution scope".to_string(),
                )
            })?;
            ExecutionRepository::find_by_id(&state.db, execution_id)
                .await
                .map_err(map_pack_archive_repository_error)?
                .and_then(|execution| execution.pack_release)
        }
        TokenType::Sensor => {
            let fence = user.sensor_workload_fence().map_err(|_| {
                (
                    StatusCode::FORBIDDEN,
                    "Sensor token is missing its workload scope".to_string(),
                )
            })?;
            SensorWorkloadRepository::pack_release_for_current_fence(&state.db, fence)
                .await
                .map_err(map_pack_archive_repository_error)?
        }
        TokenType::Access | TokenType::Refresh => None,
    };

    if pinned_pack_release_matches(pinned_release, release_id) {
        Ok(())
    } else {
        Err(pack_archive_scope_forbidden())
    }
}

fn pinned_pack_release_matches(pinned_release: Option<i64>, requested_release: i64) -> bool {
    pinned_release == Some(requested_release)
}

fn pack_ref_from_component_ref(component_ref: &str) -> Option<&str> {
    attune_common::schema::RefValidator::validate_component_ref(component_ref)
        .ok()
        .and_then(|()| component_ref.split_once('.').map(|(pack_ref, _)| pack_ref))
}

fn pack_archive_scope_forbidden() -> (StatusCode, String) {
    (
        StatusCode::FORBIDDEN,
        "Token is not authorized for this pack archive".to_string(),
    )
}

fn map_pack_archive_repository_error(error: attune_common::error::Error) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Failed to authorize pack archive: {error}"),
    )
}

async fn authorize_file_transfer(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    file_path: &str,
    operation: FileOperation,
) -> Result<(), (StatusCode, String)> {
    ValidatedRelativePath::new(file_path).map_err(map_transport_error)?;

    let allowed = match file_authorization_scope(user, operation)? {
        FileAuthorizationScope::Worker => true,
        FileAuthorizationScope::ExecutionMutation(execution_id) => {
            ArtifactVersionRepository::file_path_owned_by_execution(
                &state.db,
                file_path,
                execution_id,
            )
            .await
            .map_err(map_repository_error)?
        }
        FileAuthorizationScope::ExecutionRead => {
            let read_ctx = artifact_read_context_for_user(state, user)
                .await
                .map_err(|error| (StatusCode::FORBIDDEN, error.to_string()))?
                .ok_or_else(|| {
                    (
                        StatusCode::FORBIDDEN,
                        "Execution token has no artifact read context".to_string(),
                    )
                })?;
            ArtifactVersionRepository::file_path_is_readable(&state.db, file_path, &read_ctx)
                .await
                .map_err(map_repository_error)?
        }
        FileAuthorizationScope::Sensor(sensor_ref) => {
            ArtifactVersionRepository::file_path_owned_by_sensor(&state.db, file_path, sensor_ref)
                .await
                .map_err(map_repository_error)?
        }
    };

    if allowed {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            "Token is not authorized for this artifact file path".to_string(),
        ))
    }
}

fn map_transport_error(error: attune_common::error::Error) -> (StatusCode, String) {
    use attune_common::error::Error;
    match error {
        Error::Validation(message) => (StatusCode::BAD_REQUEST, message),
        Error::PermissionDenied(message) => (StatusCode::FORBIDDEN, message),
        Error::Io(message) if message.contains("No such file or directory") => {
            (StatusCode::NOT_FOUND, "File not found".to_string())
        }
        other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
    }
}

fn map_repository_error(error: attune_common::error::Error) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Failed to authorize artifact path: {error}"),
    )
}

fn map_sqlx_error(error: sqlx::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::auth::jwt::Claims;
    use attune_common::blob_store::{sha256, StoredObject};
    use attune_common::config::{BlobStorageConfig, Config};
    use attune_common::models::enums::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, LogStreamBackend, OwnerType,
        RetentionPolicyType,
    };
    use attune_common::repositories::artifact::CreateArtifactInput;
    use attune_common::repositories::storage_maintenance::StorageMaintenanceRepository;
    use attune_common::repositories::Create;
    use attune_common::test_database::TestDatabase;
    use chrono::{Duration, Utc};
    use std::io::Write;

    fn user(token_type: TokenType, metadata: Option<serde_json::Value>) -> AuthenticatedUser {
        AuthenticatedUser {
            claims: Claims {
                sub: "1".to_string(),
                login: "test".to_string(),
                iat: 0,
                exp: i64::MAX,
                token_type,
                scope: None,
                metadata,
            },
        }
    }

    fn assert_status_error<T>(result: Result<T, (StatusCode, String)>, expected: StatusCode) {
        match result {
            Ok(_) => panic!("expected {expected} error"),
            Err((status, _)) => assert_eq!(status, expected),
        }
    }

    #[test]
    fn identical_log_segment_retries_match() {
        let digest = "a".repeat(64);
        assert!(log_segment_retry_matches(&digest, 12, &digest, 12));
    }

    #[test]
    fn conflicting_log_segment_retries_do_not_match() {
        let digest = "a".repeat(64);
        assert!(!log_segment_retry_matches(&digest, 12, &"b".repeat(64), 12));
        assert!(!log_segment_retry_matches(&digest, 12, &digest, 13));
    }

    #[test]
    fn log_segments_commit_only_at_the_next_sequence() {
        assert_eq!(
            log_segment_commit_decision(None, false, 2, 2, "digest", 4).unwrap(),
            LogSegmentCommitDecision::Commit
        );
        assert_eq!(
            log_segment_commit_decision(None, false, 2, 3, "digest", 4)
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            log_segment_commit_decision(None, true, 2, 2, "digest", 4)
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
    }

    #[test]
    fn log_segment_retries_are_idempotent_but_conflicts_are_rejected() {
        assert_eq!(
            log_segment_commit_decision(Some(("digest", 4)), false, 3, 2, "digest", 4,).unwrap(),
            LogSegmentCommitDecision::Retry
        );
        assert_eq!(
            log_segment_commit_decision(Some(("digest", 4)), false, 3, 2, "different", 4,)
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn resolved_shared_file_snapshot_drives_range_read_without_another_lookup() {
        let directory = tempfile::tempdir().unwrap();
        let file_path = "logs/stdout.log";
        std::fs::create_dir(directory.path().join("logs")).unwrap();
        std::fs::write(directory.path().join(file_path), b"abcdef").unwrap();
        let snapshot = SharedFileLogSnapshot {
            file_path: file_path.to_string(),
            size: 6,
            sha256: None,
        };

        let mut reader = stream_shared_file_log(
            directory.path().to_str().unwrap(),
            false,
            snapshot,
            Some(ByteRange::new(1, 4).unwrap()),
        )
        .await
        .unwrap();
        let mut bytes = Vec::new();
        while let Some(chunk) = futures::StreamExt::next(&mut reader).await {
            bytes.extend_from_slice(&chunk.unwrap());
        }

        assert_eq!(bytes, b"bcd");
    }

    #[test]
    fn sealing_is_idempotent_only_for_identical_ready_state() {
        assert!(log_stream_seal_is_complete(
            true,
            true,
            Some(ArtifactBodyState::Ready)
        ));
        assert!(!log_stream_seal_is_complete(
            false,
            true,
            Some(ArtifactBodyState::Ready)
        ));
        assert!(!log_stream_seal_is_complete(
            true,
            true,
            Some(ArtifactBodyState::Pending)
        ));
    }

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn concurrent_log_commit_and_seal_work_with_one_database_connection() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path).expect("test config");
        config.database.max_connections = 1;
        config.database.min_connections = 0;
        let database = TestDatabase::create(&config.database)
            .await
            .expect("test database")
            .with_cleanup_on_drop();
        config.database.schema = Some(database.schema().to_string());
        let directory = tempfile::tempdir().expect("temporary storage");
        config.artifacts_dir = directory
            .path()
            .join("staging")
            .to_string_lossy()
            .into_owned();
        config.storage = BlobStorageConfig::Filesystem {
            root: directory.path().join("objects"),
        };
        let state = Arc::new(AppState::new(database.pool().clone(), config));

        let artifact = ArtifactRepository::create(
            &state.db,
            CreateArtifactInput {
                r#ref: "test.concurrent_log".to_string(),
                scope: OwnerType::System,
                owner: "test".to_string(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::General,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 1,
                name: None,
                description: None,
                content_type: Some("text/plain".to_string()),
                data: None,
            },
        )
        .await
        .expect("artifact");
        let version = ArtifactVersionRepository::create_file_backed(
            &state.db,
            artifact.id,
            &artifact.r#ref,
            "text/plain".to_string(),
            None,
            None,
            Some("test".to_string()),
        )
        .await
        .expect("pending version");
        let stream = LogStreamRepository::create(&state.db, version.id, 1024, 500)
            .await
            .expect("log stream");
        let worker = user(TokenType::Worker, None);

        let out_of_order = commit_log_segment(
            State(state.clone()),
            RequireAuth(worker.clone()),
            Path((version.id, 1)),
            Body::from("wrong order"),
        )
        .await;
        assert_status_error(out_of_order, StatusCode::CONFLICT);

        let commits = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(
                commit_log_segment(
                    State(state.clone()),
                    RequireAuth(worker.clone()),
                    Path((version.id, 0)),
                    Body::from("same bytes"),
                ),
                commit_log_segment(
                    State(state.clone()),
                    RequireAuth(worker.clone()),
                    Path((version.id, 0)),
                    Body::from("same bytes"),
                )
            )
        })
        .await
        .expect("commits must not exhaust the pool");
        let first = commits.0.expect("first commit").into_response().status();
        let second = commits.1.expect("second commit").into_response().status();
        assert!(matches!(
            (first, second),
            (StatusCode::CREATED, StatusCode::OK) | (StatusCode::OK, StatusCode::CREATED)
        ));

        let conflicting_bytes = commit_log_segment(
            State(state.clone()),
            RequireAuth(worker.clone()),
            Path((version.id, 0)),
            Body::from("other bytes"),
        )
        .await;
        assert_status_error(conflicting_bytes, StatusCode::CONFLICT);

        let seals = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(
                seal_log_stream(
                    State(state.clone()),
                    RequireAuth(worker.clone()),
                    Path(version.id),
                    Query(SealLogQuery { truncated: false }),
                ),
                seal_log_stream(
                    State(state.clone()),
                    RequireAuth(worker.clone()),
                    Path(version.id),
                    Query(SealLogQuery { truncated: false }),
                )
            )
        })
        .await
        .expect("seals must not exhaust the pool");
        assert_eq!(
            seals.0.expect("first seal").into_response().status(),
            StatusCode::OK
        );
        assert_eq!(
            seals.1.expect("second seal").into_response().status(),
            StatusCode::OK
        );

        let retry = commit_log_segment(
            State(state.clone()),
            RequireAuth(worker.clone()),
            Path((version.id, 0)),
            Body::from("same bytes"),
        )
        .await
        .expect("identical retry after seal")
        .into_response();
        assert_eq!(retry.status(), StatusCode::OK);

        let after_seal = commit_log_segment(
            State(state.clone()),
            RequireAuth(worker.clone()),
            Path((version.id, 1)),
            Body::from("too late"),
        )
        .await;
        assert_status_error(after_seal, StatusCode::CONFLICT);

        let conflicting_seal = seal_log_stream(
            State(state.clone()),
            RequireAuth(worker),
            Path(version.id),
            Query(SealLogQuery { truncated: true }),
        )
        .await;
        assert_status_error(conflicting_seal, StatusCode::CONFLICT);

        let stored = LogStreamRepository::find_by_artifact_version(&state.db, version.id)
            .await
            .expect("stream lookup")
            .expect("stored stream");
        let segments = LogStreamRepository::segments(&state.db, stream.id)
            .await
            .expect("segments");
        let ready = ArtifactVersionRepository::find_by_id(&state.db, version.id)
            .await
            .expect("version lookup")
            .expect("stored version");
        assert!(stored.sealed);
        assert!(!stored.truncated);
        assert_eq!(stored.next_sequence, 1);
        assert_eq!(segments.len(), 1);
        assert_eq!(ready.body_state, Some(ArtifactBodyState::Ready));
        assert_eq!(ready.size_bytes, Some(10));
    }

    #[test]
    fn pack_archive_refs_are_single_visible_components() {
        for pack_ref in ["core", "my_pack", "my-pack-2"] {
            assert!(validate_pack_archive_ref(pack_ref).is_ok(), "{pack_ref}");
        }
        for pack_ref in [
            "",
            ".pack-test-1",
            "..",
            "../core",
            "core/other",
            "core\\other",
        ] {
            assert!(validate_pack_archive_ref(pack_ref).is_err(), "{pack_ref}");
        }
    }

    #[test]
    fn candidate_archive_writer_enforces_its_heap_limit() {
        let mut writer = SizeLimitedWriter::new(4);
        assert_eq!(writer.write(b"abc").unwrap(), 3);
        assert!(writer.write(b"de").is_err());
        assert_eq!(writer.into_inner(), b"abc");
    }

    #[test]
    fn candidate_archive_source_enforces_uncompressed_limits() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("small"), b"1234").unwrap();
        assert!(validate_candidate_archive_source(root.path(), 1, 4, 4).is_ok());
        assert!(validate_candidate_archive_source(root.path(), 1, 3, 4).is_err());
        assert!(validate_candidate_archive_source(root.path(), 0, 4, 4).is_err());
        assert!(validate_candidate_archive_source(root.path(), 1, 4, 3).is_err());
    }

    #[test]
    fn candidate_archives_require_scoped_worker_tokens() {
        assert_eq!(
            pack_candidate_worker_id(&user(
                TokenType::Worker,
                Some(serde_json::json!({"worker_id": "42"})),
            ))
            .unwrap(),
            42
        );

        for candidate in [
            user(TokenType::Worker, None),
            user(
                TokenType::Execution,
                Some(serde_json::json!({"execution_id": 42})),
            ),
            user(
                TokenType::Sensor,
                Some(serde_json::json!({"sensor_ref": "core.timer"})),
            ),
            user(TokenType::Access, None),
        ] {
            assert_eq!(
                pack_candidate_worker_id(&candidate).unwrap_err().0,
                StatusCode::FORBIDDEN
            );
        }
    }

    #[test]
    fn candidate_archives_require_the_attempt_secret() {
        let secret = "attempt-secret";
        let expected_hash = attune_common::auth::hash_integration_token(secret);
        let mut headers = HeaderMap::new();

        assert_eq!(
            authorize_pack_candidate_token(&headers, &expected_hash)
                .unwrap_err()
                .0,
            StatusCode::FORBIDDEN
        );

        headers.insert(
            "x-attune-pack-candidate-token",
            "wrong-secret".parse().unwrap(),
        );
        assert_eq!(
            authorize_pack_candidate_token(&headers, &expected_hash)
                .unwrap_err()
                .0,
            StatusCode::NOT_FOUND
        );

        headers.insert("x-attune-pack-candidate-token", secret.parse().unwrap());
        assert!(authorize_pack_candidate_token(&headers, &expected_hash).is_ok());
    }

    #[test]
    fn execution_archive_scope_uses_a_valid_component_pack_ref() {
        assert_eq!(pack_ref_from_component_ref("core.echo"), Some("core"));
        assert_eq!(pack_ref_from_component_ref("other.echo"), Some("other"));
        assert_eq!(pack_ref_from_component_ref("core.echo.extra"), None);
        assert_eq!(pack_ref_from_component_ref(".hidden"), None);
    }

    #[test]
    fn release_archive_scope_requires_the_exact_pinned_release() {
        assert!(pinned_pack_release_matches(Some(42), 42));
        assert!(!pinned_pack_release_matches(Some(42), 41));
        assert!(!pinned_pack_release_matches(None, 42));
    }

    #[test]
    fn operation_classes_follow_http_capabilities() {
        assert_eq!(
            file_authorization_scope(&user(TokenType::Worker, None), FileOperation::Mutate)
                .unwrap(),
            FileAuthorizationScope::Worker
        );
        assert_eq!(
            file_authorization_scope(
                &user(
                    TokenType::Execution,
                    Some(serde_json::json!({"execution_id": 42})),
                ),
                FileOperation::Read,
            )
            .unwrap(),
            FileAuthorizationScope::ExecutionRead
        );
        assert_eq!(
            file_authorization_scope(
                &user(
                    TokenType::Execution,
                    Some(serde_json::json!({"execution_id": 42})),
                ),
                FileOperation::Mutate,
            )
            .unwrap(),
            FileAuthorizationScope::ExecutionMutation(42)
        );
        assert_eq!(
            file_authorization_scope(
                &user(
                    TokenType::Sensor,
                    Some(serde_json::json!({"sensor_ref": "core.timer"})),
                ),
                FileOperation::Read,
            )
            .unwrap(),
            FileAuthorizationScope::Sensor("core.timer")
        );
        assert_eq!(
            file_authorization_scope(&user(TokenType::Execution, None), FileOperation::Mutate)
                .unwrap_err()
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            file_authorization_scope(
                &user(
                    TokenType::Execution,
                    Some(serde_json::json!({"execution_id": 43})),
                ),
                FileOperation::Mutate,
            )
            .unwrap(),
            FileAuthorizationScope::ExecutionMutation(43)
        );
    }

    #[test]
    fn completion_accepts_only_the_reserved_body_bytes() {
        let key = ObjectKey::new("artifacts/12/v3").unwrap();
        let digest = sha256(b"completed body");
        let stored = StoredObject {
            key,
            provider_version: ProviderVersion::from_stored("e:version-1").unwrap(),
            size: 14,
            sha256: digest,
        };

        assert!(stored_object_matches(&stored, 14, digest));
        assert!(!stored_object_matches(&stored, 13, digest));
        assert!(!stored_object_matches(&stored, 14, sha256(b"other body")));
    }

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn empty_staged_file_completes_downloads_and_is_not_abandoned() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path).expect("test config");
        let database = TestDatabase::create(&config.database)
            .await
            .expect("test database")
            .with_cleanup_on_drop();
        config.database.schema = Some(database.schema().to_string());
        let directory = tempfile::tempdir().expect("temporary storage");
        config.artifacts_dir = directory
            .path()
            .join("staging")
            .to_string_lossy()
            .into_owned();
        config.storage = BlobStorageConfig::Filesystem {
            root: directory.path().join("objects"),
        };
        let state = Arc::new(AppState::new(database.pool().clone(), config));

        let artifact = ArtifactRepository::create(
            &state.db,
            CreateArtifactInput {
                r#ref: "test.empty_file".to_string(),
                scope: OwnerType::System,
                owner: "test".to_string(),
                r#type: ArtifactType::FileBinary,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::General,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 1,
                name: None,
                description: None,
                content_type: Some("application/octet-stream".to_string()),
                data: None,
            },
        )
        .await
        .expect("artifact");
        let version = ArtifactVersionRepository::create_file_backed(
            &state.db,
            artifact.id,
            &artifact.r#ref,
            "application/octet-stream".to_string(),
            None,
            None,
            Some("test".to_string()),
        )
        .await
        .expect("pending version");
        let file_path = version.file_path.expect("staging path");
        VolumeTransport::new(&state.config.artifacts_dir)
            .write_file(&file_path, b"", Some("application/octet-stream"))
            .await
            .expect("empty staging file");
        let worker = user(TokenType::Worker, None);

        for _ in 0..2 {
            let response = complete_file(
                State(state.clone()),
                RequireAuth(worker.clone()),
                Path(file_path.clone()),
            )
            .await
            .expect("completion")
            .into_response();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["x-attune-size"], "0");
        }

        let ready = ArtifactVersionRepository::find_by_id(&state.db, version.id)
            .await
            .expect("ready lookup")
            .expect("ready version");
        assert_eq!(ready.body_state, Some(ArtifactBodyState::Ready));
        assert_eq!(ready.size_bytes, Some(0));
        assert_eq!(
            ready.sha256.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert!(ready.object_key.is_some());
        assert!(ready.provider_version.is_some());

        let response = download_file(
            State(state.clone()),
            RequireAuth(worker),
            Path(file_path),
            HeaderMap::new(),
        )
        .await
        .expect("download")
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("download body")
            .is_empty());

        let abandoned = StorageMaintenanceRepository::abandoned_pending(
            &state.db,
            Utc::now() + Duration::hours(1),
            10,
        )
        .await
        .expect("abandoned lookup");
        assert!(!abandoned.iter().any(|candidate| candidate.id == version.id));
    }

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn shared_log_seal_stats_hashes_and_reads_the_authoritative_file() {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path).expect("test config");
        let database = TestDatabase::create(&config.database)
            .await
            .expect("test database")
            .with_cleanup_on_drop();
        config.database.schema = Some(database.schema().to_string());
        let directory = tempfile::tempdir().expect("temporary storage");
        config.artifacts_dir = directory
            .path()
            .join("artifacts")
            .to_string_lossy()
            .into_owned();
        config.storage = BlobStorageConfig::Filesystem {
            root: directory.path().join("objects"),
        };
        let state = Arc::new(AppState::new(database.pool().clone(), config));
        let artifact = ArtifactRepository::create(
            &state.db,
            CreateArtifactInput {
                r#ref: "core.echo.stdout.log".to_string(),
                scope: OwnerType::Action,
                owner: "core.echo".to_string(),
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 1,
                name: None,
                description: None,
                content_type: Some("text/plain".to_string()),
                data: None,
            },
        )
        .await
        .expect("artifact");
        let version = ArtifactVersionRepository::create_log_pending(
            &state.db,
            artifact.id,
            &artifact.r#ref,
            LogStreamBackend::SharedFile,
            "text/plain".to_string(),
            Some(42),
            None,
            Some("worker".to_string()),
        )
        .await
        .expect("pending log");
        let stream = LogStreamRepository::create_with_backend(
            &state.db,
            version.id,
            LogStreamBackend::SharedFile,
            1024,
            500,
        )
        .await
        .expect("stream");
        let file_path = version.file_path.as_deref().expect("file path");
        VolumeTransport::new(&state.config.artifacts_dir)
            .write_file(file_path, b"0123456789", Some("text/plain"))
            .await
            .expect("shared log file");
        assert_eq!(
            resolve_shared_file_log_snapshot(&state, &stream)
                .await
                .unwrap()
                .size,
            10
        );
        assert!(!stream.sealed);

        let response = seal_log_stream(
            State(state.clone()),
            RequireAuth(user(TokenType::Worker, None)),
            Path(version.id),
            Query(SealLogQuery { truncated: true }),
        )
        .await
        .expect("seal")
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let sealed = LogStreamRepository::find_by_artifact_version(&state.db, version.id)
            .await
            .unwrap()
            .unwrap();
        let ready = ArtifactVersionRepository::find_by_id(&state.db, version.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sealed.total_bytes, 10);
        assert!(sealed.truncated);
        assert_eq!(ready.size_bytes, Some(10));
        assert_eq!(
            ready.sha256.as_deref(),
            Some(hex_digest(&sha256(b"0123456789")).as_str())
        );
        assert!(ready.object_key.is_none());
        assert!(LogStreamRepository::segments(&state.db, stream.id)
            .await
            .unwrap()
            .is_empty());

        let mut reader = stream_log_stream(&state, stream.id, Some(ByteRange::new(3, 7).unwrap()))
            .await
            .expect("range reader");
        let mut bytes = Vec::new();
        while let Some(chunk) = futures::StreamExt::next(&mut reader).await {
            bytes.extend_from_slice(&chunk.expect("chunk"));
        }
        assert_eq!(bytes, b"3456");
    }
}
