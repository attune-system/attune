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
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

use crate::{
    auth::{jwt::TokenType, middleware::AuthenticatedUser, middleware::RequireAuth},
    state::AppState,
};

/// Upload or overwrite a file at the given path.
///
/// The request body is the raw file content.
/// Content-Type header is stored alongside the file if needed.
async fn upload_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_internal_transfer_token(&user)?;

    let artifacts_dir = &state.config.artifacts_dir;
    let max_size = state.config.artifacts.max_upload_size;

    // Validate path: no traversal
    if file_path.contains("..") || file_path.starts_with('/') {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid file path: must be relative with no '..' segments".to_string(),
        ));
    }

    let full_path = std::path::Path::new(artifacts_dir).join(&file_path);

    // Ensure parent directory exists
    if let Some(parent) = full_path.parent() {
        attune_common::utils::create_shared_dir_all(parent)
            .await
            .map_err(|e| {
                warn!("Failed to create directory for {file_path}: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to create directory: {e}"),
                )
            })?;
    }

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

    tokio::fs::write(&full_path, &bytes).await.map_err(|e| {
        warn!("Failed to write file {file_path}: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write file: {e}"),
        )
    })?;

    debug!(
        path = %file_path,
        size = bytes.len(),
        content_type = %content_type,
        "File uploaded via internal endpoint"
    );

    Ok(StatusCode::CREATED)
}

/// Download file content at the given path.
async fn download_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_internal_transfer_token(&user)?;

    let artifacts_dir = &state.config.artifacts_dir;

    if file_path.contains("..") || file_path.starts_with('/') {
        return Err((StatusCode::BAD_REQUEST, "Invalid file path".to_string()));
    }

    let full_path = std::path::Path::new(artifacts_dir).join(&file_path);

    if !full_path.exists() {
        return Err((StatusCode::NOT_FOUND, "File not found".to_string()));
    }

    let bytes = tokio::fs::read(&full_path).await.map_err(|e| {
        warn!("Failed to read file {file_path}: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read file: {e}"),
        )
    })?;

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
async fn append_to_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
    body: Body,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_internal_transfer_token(&user)?;

    let artifacts_dir = &state.config.artifacts_dir;
    let max_size = state.config.artifacts.max_upload_size;

    if file_path.contains("..") || file_path.starts_with('/') {
        return Err((StatusCode::BAD_REQUEST, "Invalid file path".to_string()));
    }

    let full_path = std::path::Path::new(artifacts_dir).join(&file_path);

    if let Some(parent) = full_path.parent() {
        attune_common::utils::create_shared_dir_all(parent)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to create directory: {e}"),
                )
            })?;
    }

    let bytes = axum::body::to_bytes(body, max_size as usize)
        .await
        .map_err(|e| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("Request body too large: {e}"),
            )
        })?;

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&full_path)
        .await
        .map_err(|e| {
            warn!("Failed to open file for append {file_path}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to open file: {e}"),
            )
        })?;

    file.write_all(&bytes).await.map_err(|e| {
        warn!("Failed to append to file {file_path}: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to append: {e}"),
        )
    })?;

    debug!(
        path = %file_path,
        appended_bytes = bytes.len(),
        "File appended via internal endpoint"
    );

    Ok(StatusCode::NO_CONTENT)
}

/// Check file existence and return size via HEAD request.
async fn check_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    require_internal_transfer_token_head(&user)?;

    let artifacts_dir = &state.config.artifacts_dir;

    if file_path.contains("..") || file_path.starts_with('/') {
        return Err(StatusCode::BAD_REQUEST);
    }

    let full_path = std::path::Path::new(artifacts_dir).join(&file_path);

    match tokio::fs::metadata(&full_path).await {
        Ok(meta) => {
            let mut headers = HeaderMap::new();
            headers.insert("Content-Length", meta.len().to_string().parse().unwrap());
            let content_type = mime_from_extension(&file_path);
            headers.insert("Content-Type", content_type.parse().unwrap());
            Ok((StatusCode::OK, headers))
        }
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

/// Delete a file. Returns 204 on success, 404 if not found.
async fn delete_file(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(file_path): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_internal_transfer_token(&user)?;

    let artifacts_dir = &state.config.artifacts_dir;

    if file_path.contains("..") || file_path.starts_with('/') {
        return Err((StatusCode::BAD_REQUEST, "Invalid file path".to_string()));
    }

    let full_path = std::path::Path::new(artifacts_dir).join(&file_path);

    match tokio::fs::remove_file(&full_path).await {
        Ok(()) => {
            debug!(path = %file_path, "File deleted via internal endpoint");
            // Clean up empty parent directories
            if let Some(parent) = full_path.parent() {
                let _ = cleanup_empty_dirs(parent, artifacts_dir).await;
            }
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err((StatusCode::NOT_FOUND, "File not found".to_string()))
        }
        Err(e) => {
            warn!("Failed to delete file {file_path}: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to delete: {e}"),
            ))
        }
    }
}

/// Clean up empty parent directories up to (but not including) the base dir.
async fn cleanup_empty_dirs(dir: &std::path::Path, base: &str) -> std::io::Result<()> {
    let base_path = std::path::Path::new(base);
    let mut current = dir.to_path_buf();
    while current != base_path && current.starts_with(base_path) {
        match tokio::fs::remove_dir(&current).await {
            Ok(()) => {
                debug!("Removed empty directory: {}", current.display());
            }
            Err(_) => break, // Not empty or other error
        }
        match current.parent() {
            Some(p) => current = p.to_path_buf(),
            None => break,
        }
    }
    Ok(())
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
async fn delete_file_handler(
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
async fn download_pack_archive(
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

fn require_internal_transfer_token_head(user: &AuthenticatedUser) -> Result<(), StatusCode> {
    require_internal_transfer_token(user).map_err(|(status, _)| status)
}
