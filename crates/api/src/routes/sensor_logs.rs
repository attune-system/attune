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
    trigger::SensorRepository,
    FindByRef,
};
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    middleware::{ApiError, ApiResult},
    routes::triggers::can_access_sensor_api,
    state::AppState,
};

/// Summary of a sensor's available log artifacts.
#[derive(Serialize)]
struct SensorLogSummary {
    sensor_ref: String,
    logs: Vec<SensorLogEntry>,
}

#[derive(Serialize)]
struct SensorLogEntry {
    stream: String,
    artifact_ref: String,
}

#[derive(Debug, Deserialize)]
struct SensorLogQuery {
    tail: Option<usize>,
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sensors/{sensor_ref}/logs", get(list_sensor_logs))
        .route("/sensors/{sensor_ref}/logs/{stream}", get(get_sensor_log))
}

/// List available log streams for a sensor.
#[utoipa::path(
    get,
    path = "/api/v1/sensors/{sensor_ref}/logs",
    params(
        ("sensor_ref" = String, Path, description = "Sensor reference (e.g., core.timer)")
    ),
    responses(
        (status = 200, description = "Sensor log summary"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = []))
)]
async fn list_sensor_logs(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(sensor_ref): Path<String>,
) -> ApiResult<Json<SensorLogSummary>> {
    ensure_visible_sensor(&state, &user, &sensor_ref).await?;

    let mut logs = Vec::new();

    for stream in &["stdout", "stderr"] {
        logs.push(SensorLogEntry {
            stream: stream.to_string(),
            artifact_ref: format!("sensor.{}.{}", sensor_ref, stream),
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
async fn get_sensor_log(
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

    let mut content = read_sensor_log_content(
        &state.config.artifacts_dir,
        artifact.id,
        &sensor_ref,
        &stream,
        query.tail,
        &state.db,
    )
    .await?;

    if let Some(tail) = query.tail.filter(|tail| *tail > 0) {
        let lines = content.lines().collect::<Vec<_>>();
        if lines.len() > tail {
            content = lines[lines.len() - tail..].join("\n");
            content.push('\n');
        }
    }

    Ok((
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        content,
    ))
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

async fn read_sensor_log_content(
    artifacts_dir: &str,
    artifact_id: i64,
    sensor_ref: &str,
    stream: &str,
    tail: Option<usize>,
    pool: &sqlx::PgPool,
) -> ApiResult<String> {
    let mut versions = ArtifactVersionRepository::list_by_artifact(pool, artifact_id).await?;
    versions.retain(|version| version.file_path.is_some());

    if !versions.is_empty() {
        let mut chunks = Vec::new();
        if tail.filter(|tail| *tail > 0).is_some() {
            let mut line_count = 0usize;
            for version in versions.iter() {
                if let Some(content) =
                    read_log_file_path(artifacts_dir, version.file_path.as_deref().unwrap()).await?
                {
                    line_count += content.lines().count();
                    chunks.push(content);
                    if line_count >= tail.unwrap_or(0) {
                        break;
                    }
                }
            }
            chunks.reverse();
        } else {
            versions.reverse();
            for version in versions {
                if let Some(content) =
                    read_log_file_path(artifacts_dir, version.file_path.as_deref().unwrap()).await?
                {
                    chunks.push(content);
                }
            }
        }

        if !chunks.is_empty() {
            return Ok(join_log_chunks(chunks));
        }
    }

    let legacy_path = std::path::Path::new(artifacts_dir)
        .join("sensors")
        .join(sensor_ref)
        .join(format!("{}.log", stream));

    match tokio::fs::read(&legacy_path).await {
        Ok(content) => Ok(String::from_utf8_lossy(&content).into_owned()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                warn!(
                    "Skipping unreadable legacy sensor log '{}': {}",
                    legacy_path.display(),
                    e
                );
            }
            Ok(String::new())
        }
        Err(e) => Err(ApiError::InternalServerError(format!(
            "Failed to read sensor log: {}",
            e
        ))),
    }
}

async fn read_log_file_path(artifacts_dir: &str, file_path: &str) -> ApiResult<Option<String>> {
    let path = std::path::Path::new(artifacts_dir).join(file_path);
    match tokio::fs::read(&path).await {
        Ok(content) => Ok(Some(String::from_utf8_lossy(&content).into_owned())),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                warn!("Skipping unreadable sensor log '{}': {}", path.display(), e);
            }
            Ok(None)
        }
        Err(e) => Err(ApiError::InternalServerError(format!(
            "Failed to read sensor log '{}': {}",
            file_path, e
        ))),
    }
}

fn join_log_chunks(chunks: Vec<String>) -> String {
    let mut content = String::new();
    for chunk in chunks {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&chunk);
    }
    content
}
