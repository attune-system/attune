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
    require_internal_transfer_token(&user)?;

    // Validate pack_ref: no path traversal
    if pack_ref.contains("..") || pack_ref.contains('/') || pack_ref.contains('\\') {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid pack_ref: path traversal not allowed".to_string(),
        ));
    }

    let packs_base_dir = &state.config.packs_base_dir;
    let pack_dir = std::path::Path::new(packs_base_dir).join(&pack_ref);

    if !pack_dir.is_dir() {
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

fn require_internal_transfer_token(user: &AuthenticatedUser) -> Result<(), (StatusCode, String)> {
    match user.claims.token_type {
        TokenType::Execution | TokenType::Sensor | TokenType::Worker => Ok(()),
        TokenType::Access | TokenType::Refresh => Err((
            StatusCode::FORBIDDEN,
            "Internal file transfer endpoints require execution, sensor, or worker tokens"
                .to_string(),
        )),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::auth::jwt::Claims;

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
    fn internal_transfer_token_types_match_contract() {
        for token_type in [TokenType::Execution, TokenType::Sensor, TokenType::Worker] {
            assert!(require_internal_transfer_token(&user(token_type, None)).is_ok());
        }
        for token_type in [TokenType::Access, TokenType::Refresh] {
            assert_eq!(
                require_internal_transfer_token(&user(token_type, None))
                    .unwrap_err()
                    .0,
                StatusCode::FORBIDDEN
            );
        }
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
