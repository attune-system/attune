//! Sensor Log Endpoints
//!
//! Provides read access to per-sensor rotating log files (stdout/stderr)
//! via the standard artifact system.
//!
//! Sensor log artifacts are auto-registered by the sensor service with refs
//! in the format `sensor.{sensor_ref}.stdout` / `sensor.{sensor_ref}.stderr`.
//! These endpoints resolve a sensor ref to the matching artifact and delegate
//! to the existing artifact download/stream infrastructure.

use std::sync::Arc;

use attune_common::repositories::{
    artifact::{ArtifactRepository, ArtifactVersionRepository},
    log_stream::LogStreamRepository,
    trigger::SensorRepository,
    FindByRef,
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::header,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use attune_common::blob_store::{
    body_from_file, BlobReader, ByteRange, ObjectKey, ProviderVersion,
};
use futures::{stream, StreamExt, TryStreamExt};

use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    middleware::{ApiError, ApiResult},
    routes::triggers::can_access_sensor_api,
    state::AppState,
};

/// Summary of a sensor's available log artifacts.
#[derive(Serialize)]
pub(crate) struct SensorLogSummary {
    sensor_ref: String,
    logs: Vec<SensorLogEntry>,
}

#[derive(Serialize)]
pub(crate) struct SensorLogEntry {
    stream: String,
    artifact_ref: String,
    artifact_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SensorLogQuery {
    tail: Option<usize>,
}

const SENSOR_LOG_TAIL_MAX_BYTES: usize = 1024 * 1024;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sensors/{sensor_ref}/logs", get(list_sensor_logs))
        .route("/sensors/{sensor_ref}/logs/{stream}", get(get_sensor_log))
}

/// List available log streams for a sensor.
#[utoipa::path(
    get,
    path = "/api/v1/sensors/{sensor_ref}/logs",
    tag = "sensors",
    params(
        ("sensor_ref" = String, Path, description = "Sensor reference (e.g., core.timer)")
    ),
    responses(
        (status = 200, description = "Sensor log summary"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn list_sensor_logs(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(sensor_ref): Path<String>,
) -> ApiResult<Json<SensorLogSummary>> {
    ensure_visible_sensor(&state, &user, &sensor_ref).await?;

    let mut logs = Vec::new();

    for stream in &["stdout", "stderr"] {
        let artifact_ref = format!("sensor.{}.{}", sensor_ref, stream);
        let artifact_id = ArtifactRepository::find_by_ref(&state.db, &artifact_ref)
            .await?
            .map(|artifact| artifact.id);
        logs.push(SensorLogEntry {
            stream: stream.to_string(),
            artifact_ref,
            artifact_id,
        });
    }

    Ok(Json(SensorLogSummary { sensor_ref, logs }))
}

/// Download a specific sensor log stream.
///
/// Resolves the sensor ref + stream to a log file on disk and serves
/// the content as plain text.
#[utoipa::path(
    get,
    path = "/api/v1/sensors/{sensor_ref}/logs/{stream}",
    tag = "sensors",
    params(
        ("sensor_ref" = String, Path, description = "Sensor reference (e.g., core.timer)"),
        ("stream" = String, Path, description = "Log stream: stdout or stderr")
    ),
    responses(
        (status = 200, description = "Log file content", content_type = "text/plain"),
        (status = 404, description = "Sensor log not found"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn get_sensor_log(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((sensor_ref, stream)): Path<(String, String)>,
    Query(query): Query<SensorLogQuery>,
) -> ApiResult<impl IntoResponse> {
    if stream != "stdout" && stream != "stderr" {
        return Err(ApiError::ValidationError(
            "stream must be 'stdout' or 'stderr'".into(),
        ));
    }

    ensure_visible_sensor(&state, &user, &sensor_ref).await?;

    let artifact_ref = format!("sensor.{}.{}", sensor_ref, stream);

    // Verify artifact exists in DB
    let artifact = ArtifactRepository::find_by_ref(&state.db, &artifact_ref)
        .await
        .map_err(|e| ApiError::DatabaseError(format!("DB error: {}", e)))?
        .ok_or_else(|| ApiError::NotFound("Sensor log not found".to_string()))?;

    debug!(
        "Resolved sensor log '{}' to artifact id={}",
        artifact_ref, artifact.id
    );

    if let Some(tail) = query.tail.filter(|tail| *tail > 0) {
        let content = read_sensor_log_tail(&state, artifact.id, &sensor_ref, &stream, tail).await?;
        return Ok((
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            Body::from(content),
        )
            .into_response());
    }

    let reader = stream_sensor_log(&state, artifact.id, &sensor_ref, &stream).await?;
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        Body::from_stream(reader),
    )
        .into_response())
}

async fn ensure_visible_sensor(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    sensor_ref: &str,
) -> ApiResult<()> {
    let sensor = SensorRepository::find_by_ref(&state.db, sensor_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Sensor '{}' not found", sensor_ref)))?;
    if !can_access_sensor_api(state, user, &sensor).await? {
        return Err(ApiError::NotFound(format!(
            "Sensor '{}' not found",
            sensor_ref
        )));
    }
    Ok(())
}

async fn stream_sensor_log(
    state: &AppState,
    artifact_id: i64,
    sensor_ref: &str,
    stream: &str,
) -> ApiResult<BlobReader> {
    let mut versions = ArtifactVersionRepository::list_by_artifact(&state.db, artifact_id).await?;
    versions.retain(|version| version.file_path.is_some());
    versions.reverse();

    let mut readers = Vec::new();
    for version in versions {
        if let Some(log_stream) =
            LogStreamRepository::find_by_artifact_version(&state.db, version.id).await?
        {
            readers.push(
                super::internal_files::stream_log_stream(state, log_stream.id, None)
                    .await
                    .map_err(|error| ApiError::InternalServerError(error.to_string()))?,
            );
        } else if let Some(file_path) = version.file_path {
            let path = std::path::Path::new(&state.config.artifacts_dir).join(file_path);
            match body_from_file(&path).await {
                Ok(reader) => readers.push(reader),
                Err(error) => {
                    warn!(path = %path.display(), %error, "Skipping unreadable sensor log")
                }
            }
        }
    }

    if readers.is_empty() {
        let path = legacy_sensor_log_path(&state.config.artifacts_dir, sensor_ref, stream);
        if let Ok(reader) = body_from_file(&path).await {
            readers.push(reader);
        }
    }

    Ok(stream::iter(readers).flatten().boxed())
}

async fn read_sensor_log_tail(
    state: &AppState,
    artifact_id: i64,
    sensor_ref: &str,
    stream_name: &str,
    tail: usize,
) -> ApiResult<String> {
    let versions = ArtifactVersionRepository::list_by_artifact(&state.db, artifact_id).await?;
    let mut newest_chunks = Vec::new();
    let mut bytes = 0_usize;
    let mut newlines = 0_usize;

    for version in versions {
        if bytes >= SENSOR_LOG_TAIL_MAX_BYTES || newlines > tail {
            break;
        }
        let remaining = SENSOR_LOG_TAIL_MAX_BYTES - bytes;
        let chunk = if let Some(log_stream) =
            LogStreamRepository::find_by_artifact_version(&state.db, version.id).await?
        {
            read_log_stream_tail(state, log_stream.id, remaining, tail - newlines.min(tail)).await?
        } else if let Some(file_path) = version.file_path {
            read_file_tail(
                &std::path::Path::new(&state.config.artifacts_dir).join(file_path),
                remaining,
            )
            .await?
        } else {
            Vec::new()
        };
        bytes += chunk.len();
        newlines += chunk.iter().filter(|byte| **byte == b'\n').count();
        if !chunk.is_empty() {
            newest_chunks.push(chunk);
        }
    }

    if newest_chunks.is_empty() {
        newest_chunks.push(
            read_file_tail(
                &legacy_sensor_log_path(&state.config.artifacts_dir, sensor_ref, stream_name),
                SENSOR_LOG_TAIL_MAX_BYTES,
            )
            .await?,
        );
    }
    newest_chunks.reverse();
    let content = newest_chunks.concat();
    let text = String::from_utf8_lossy(&content);
    let lines = text.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(tail);
    let mut result = lines[start..].join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    Ok(result)
}

async fn read_log_stream_tail(
    state: &AppState,
    stream_id: i64,
    max_bytes: usize,
    tail: usize,
) -> ApiResult<Vec<u8>> {
    let mut segments = LogStreamRepository::segments(&state.db, stream_id).await?;
    let mut newest_chunks = Vec::new();
    let mut bytes = 0_usize;
    let mut newlines = 0_usize;
    while let Some(segment) = segments.pop() {
        if bytes >= max_bytes || newlines > tail {
            break;
        }
        let size = u64::try_from(segment.size_bytes)
            .map_err(|_| ApiError::InternalServerError("Invalid log segment size".into()))?;
        let take = size.min((max_bytes - bytes) as u64);
        let key = ObjectKey::new(segment.object_key)
            .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
        let version = ProviderVersion::from_stored(segment.provider_version)
            .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
        let range = (take < size)
            .then(|| ByteRange::new(size - take, size))
            .transpose()
            .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
        let chunk = state
            .blob_store
            .get(&key, &version, range)
            .await
            .map_err(|error| ApiError::InternalServerError(error.to_string()))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| ApiError::InternalServerError(error.to_string()))?
            .concat();
        bytes += chunk.len();
        newlines += chunk.iter().filter(|byte| **byte == b'\n').count();
        newest_chunks.push(chunk);
    }
    newest_chunks.reverse();
    Ok(newest_chunks.concat())
}

async fn read_file_tail(path: &std::path::Path, max_bytes: usize) -> ApiResult<Vec<u8>> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                warn!(path = %path.display(), %error, "Skipping unreadable sensor log");
            }
            return Ok(Vec::new());
        }
        Err(error) => return Err(ApiError::InternalServerError(error.to_string())),
    };
    let size = file
        .metadata()
        .await
        .map_err(|error| ApiError::InternalServerError(error.to_string()))?
        .len();
    let start = size.saturating_sub(max_bytes as u64);
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
    let mut bytes = Vec::with_capacity((size - start) as usize);
    file.read_to_end(&mut bytes)
        .await
        .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
    Ok(bytes)
}

fn legacy_sensor_log_path(
    artifacts_dir: &str,
    sensor_ref: &str,
    stream: &str,
) -> std::path::PathBuf {
    std::path::Path::new(artifacts_dir)
        .join("sensors")
        .join(sensor_ref)
        .join(format!("{stream}.log"))
}
