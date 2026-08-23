//! Internal file transfer endpoints for artifact content distribution.
//!
//! These endpoints allow workers and sensors to upload/download/append
//! raw file content when they do not share a mounted volume with the API.
//!
//! **Authentication**: Requires a valid JWT (Access, Execution, or Worker token).
//!
//! **Path parameter**: `file_path` is the relative path within `artifacts_dir`,
//! matching what is stored in `artifact_version.file_path`.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, head, patch, put},
    Router,
};
use std::sync::Arc;
use tracing::{debug, warn};

use attune_common::artifact_transport::{
    ArtifactFileTransport, ValidatedRelativePath, VolumeTransport,
};
use attune_common::repositories::artifact::ArtifactVersionRepository;
use attune_common::repositories::pack_install::PackInstallRepository;
use attune_common::repositories::{ExecutionRepository, FindById, FindByRef, SensorRepository};

use crate::{
    auth::{jwt::TokenType, middleware::AuthenticatedUser, middleware::RequireAuth},
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

    // Read body with size limit
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");

    let bytes = axum::body::to_bytes(body, max_size as usize)
        .await
        .map_err(|e| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("Request body too large (max {max_size} bytes): {e}"),
            )
        })?;

    VolumeTransport::new(artifacts_dir)
        .write_file(&file_path, &bytes, Some(content_type))
        .await
        .map_err(map_transport_error)?;

    debug!(
        path = %file_path,
        size = bytes.len(),
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
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authorize_file_transfer(&state, &user, &file_path, FileOperation::Read).await?;

    let artifacts_dir = &state.config.artifacts_dir;

    let bytes = VolumeTransport::new(artifacts_dir)
        .read_file(&file_path)
        .await
        .map_err(map_transport_error)?;

    // Guess content type from extension
    let content_type = mime_from_extension(&file_path);

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", content_type.parse().unwrap());
    headers.insert("Content-Length", bytes.len().to_string().parse().unwrap());

    Ok((StatusCode::OK, headers, bytes))
}

/// Append content to an existing file (or create it).
///
/// Used for streaming log writes — workers send periodic chunks.
#[utoipa::path(
    patch,
    path = "/api/v1/internal/files/{file_path}",
    tag = "internal",
    params(
        ("file_path" = String, Path, description = "Relative artifact file path")
    ),
    request_body(content = String, content_type = "application/octet-stream"),
    responses(
        (status = 204, description = "File content appended"),
        (status = 400, description = "Invalid file path"),
        (status = 401, description = "Unauthorized"),
        (status = 413, description = "Payload too large"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn append_to_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
    body: Body,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    authorize_file_transfer(&state, &user, &file_path, FileOperation::Mutate).await?;

    let artifacts_dir = &state.config.artifacts_dir;
    let max_size = state.config.artifacts.max_upload_size;

    let bytes = axum::body::to_bytes(body, max_size as usize)
        .await
        .map_err(|e| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("Request body too large: {e}"),
            )
        })?;

    VolumeTransport::new(artifacts_dir)
        .append_file(&file_path, &bytes)
        .await
        .map_err(map_transport_error)?;

    debug!(
        path = %file_path,
        appended_bytes = bytes.len(),
        "File appended via internal endpoint"
    );

    Ok(StatusCode::NO_CONTENT)
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

    let artifacts_dir = &state.config.artifacts_dir;

    match VolumeTransport::new(artifacts_dir)
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
        .route("/internal/files/{*file_path}", patch(append_to_file))
        .route("/internal/files/{*file_path}", head(check_file))
        .route("/internal/files/{*file_path}", delete(delete_file_handler))
        .route(
            "/internal/packs/{pack_ref}/archive",
            get(download_pack_archive),
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

/// Stream a pack directory as a `.tar.gz` archive.
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
) -> Result<impl IntoResponse, (StatusCode, String)> {
    validate_pack_archive_ref(&pack_ref)?;
    authorize_pack_archive(&state, &user, &pack_ref).await?;

    let packs_base_dir = &state.config.packs_base_dir;
    let pack_dir = std::path::Path::new(packs_base_dir).join(&pack_ref);

    if !matches!(
        std::fs::symlink_metadata(&pack_dir),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink()
    ) {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Pack '{}' not found on this server", pack_ref),
        ));
    }

    debug!(
        "Streaming pack archive for '{}' from {:?}",
        pack_ref, pack_dir
    );

    // Build the tar.gz in memory.
    // Pack directories are typically small (KB-low MB), so this is fine.
    let pack_ref_clone = pack_ref.clone();
    let tarball = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let buf = Vec::new();
        let encoder = GzEncoder::new(buf, Compression::fast());
        let mut tar_builder = tar::Builder::new(encoder);
        tar_builder.follow_symlinks(false);

        // Add all files in the pack directory, rooted at pack_ref
        tar_builder.append_dir_all(&pack_ref_clone, &pack_dir)?;
        tar_builder.finish()?;

        let encoder = tar_builder.into_inner()?;
        encoder.finish()
    })
    .await
    .map_err(|e| {
        warn!("Pack archive task panicked: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error building pack archive".to_string(),
        )
    })?
    .map_err(|e| {
        warn!("Failed to build pack archive for '{}': {}", pack_ref, e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to build pack archive: {}", e),
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

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::auth::jwt::Claims;
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
}
