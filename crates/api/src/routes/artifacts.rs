//! Artifact management API routes
//!
//! Provides endpoints for:
//! - CRUD operations on artifacts (metadata + data)
//! - File-backed version creation (execution writes file to shared volume)
//! - File upload (binary) and download for file-type artifacts
//! - JSON content versioning for structured artifacts
//! - Progress append for progress-type artifacts (streaming updates)
//! - Listing artifacts by execution
//! - Version history and retrieval
//! - Upsert-and-upload: create-or-reuse an artifact by ref and upload a version in one call
//! - Upsert-and-allocate: create-or-reuse an artifact by ref and allocate a file-backed version path in one call
//! - SSE streaming for file-backed artifacts (live tail while execution is running)

use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Json, Router,
};
use futures::stream::Stream;
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    time::{sleep, Duration, Instant},
};
use tracing::{debug, warn};

use attune_common::audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome};
use attune_common::models::enums::{
    ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
};
use attune_common::repositories::{
    action::ActionRepository,
    artifact::{
        classify_artifact, default_content_type_for_artifact, is_file_backed_type,
        is_runtime_log_artifact_ref, ref_to_dir_path, ArtifactRepository, ArtifactSearchFilters,
        ArtifactVersionRepository, CreateArtifactInput, CreateArtifactVersionInput,
        UpdateArtifactInput,
    },
    execution::ExecutionRepository,
    trigger::SensorRepository,
    Create, Delete, FindById, FindByRef, Patch, Update,
};

use crate::{
    auth::{jwt::TokenType, middleware::AuthenticatedUser, middleware::RequireAuth},
    authz::{
        execution_has_standard_access, execution_standard_pack_refs, AuthorizationCheck,
        AuthorizationService,
    },
    dto::{
        artifact::{
            AllocateFileVersionByRefRequest, AppendProgressRequest, ArtifactJsonPatch,
            ArtifactQueryParams, ArtifactResponse, ArtifactStringPatch, ArtifactSummary,
            ArtifactVersionResponse, ArtifactVersionSummary, CreateArtifactRequest,
            CreateFileVersionRequest, CreateVersionJsonRequest, SetDataRequest,
            UpdateArtifactRequest,
        },
        common::{PaginatedResponse, PaginationParams},
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::rbac::{Action, AuthorizationContext, Resource};

// ============================================================================
// Artifact CRUD
// ============================================================================

struct ArtifactRetentionDefaults<'a> {
    artifact_ref: &'a str,
    scope: OwnerType,
    owner: &'a str,
    execution: Option<i64>,
    requested_policy: Option<RetentionPolicyType>,
    requested_limit: Option<i32>,
    fallback_limit: i32,
}

async fn resolve_artifact_retention(
    state: &AppState,
    user: &AuthenticatedUser,
    defaults: ArtifactRetentionDefaults<'_>,
) -> ApiResult<(RetentionPolicyType, i32)> {
    if let Some(limit) = defaults.requested_limit {
        if limit <= 0 {
            return Err(ApiError::BadRequest(
                "retention_limit must be greater than zero".to_string(),
            ));
        }
    }

    if is_runtime_log_artifact_ref(defaults.artifact_ref) {
        return Ok((
            defaults
                .requested_policy
                .unwrap_or(RetentionPolicyType::Versions),
            defaults.requested_limit.unwrap_or(defaults.fallback_limit),
        ));
    }

    let execution_id = defaults.execution.or_else(|| user.execution_id());
    let mut default_policy = None;
    let mut default_limit = None;

    if let Some(execution_id) = execution_id {
        if let Some(execution) = ExecutionRepository::find_by_id(&state.db, execution_id).await? {
            default_policy = execution.artifact_retention_policy;
            default_limit = execution.artifact_retention_limit;
        }
    }

    if default_policy.is_none() || default_limit.is_none() {
        match defaults.scope {
            OwnerType::Action => {
                if let Some(action) =
                    ActionRepository::find_by_ref(&state.db, defaults.owner).await?
                {
                    default_policy = default_policy.or(action.artifact_retention_policy);
                    default_limit = default_limit.or(action.artifact_retention_limit);
                }
            }
            OwnerType::Sensor => {
                if let Some(sensor) =
                    SensorRepository::find_by_ref(&state.db, defaults.owner).await?
                {
                    default_policy = default_policy.or(sensor.artifact_retention_policy);
                    default_limit = default_limit.or(sensor.artifact_retention_limit);
                }
            }
            _ => {}
        }
    }

    Ok((
        defaults
            .requested_policy
            .or(default_policy)
            .unwrap_or(RetentionPolicyType::Versions),
        defaults
            .requested_limit
            .or(default_limit)
            .unwrap_or(defaults.fallback_limit),
    ))
}

fn default_visibility_for_artifact(
    artifact_type: ArtifactType,
    classification: ArtifactClassification,
) -> ArtifactVisibility {
    if classification == ArtifactClassification::RuntimeLog {
        ArtifactVisibility::Private
    } else if artifact_type == ArtifactType::Progress {
        ArtifactVisibility::Public
    } else {
        ArtifactVisibility::Private
    }
}

fn validate_runtime_log_visibility(
    classification: ArtifactClassification,
    visibility: ArtifactVisibility,
) -> ApiResult<()> {
    if classification == ArtifactClassification::RuntimeLog
        && visibility == ArtifactVisibility::Public
    {
        return Err(ApiError::BadRequest(
            "runtime_log artifacts must remain private".to_string(),
        ));
    }
    Ok(())
}

/// List artifacts with pagination and optional filters
#[utoipa::path(
    get,
    path = "/api/v1/artifacts",
    tag = "artifacts",
    params(ArtifactQueryParams),
    responses(
        (status = 200, description = "List of artifacts", body = PaginatedResponse<ArtifactSummary>),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_artifacts(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ArtifactQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let filters = ArtifactSearchFilters {
        scope: query.scope,
        owner: query.owner.clone(),
        r#type: query.r#type,
        visibility: query.visibility,
        classification: query.classification,
        execution: query.execution,
        name_contains: query.name.clone(),
        limit: query.limit(),
        offset: query.offset(),
    };

    let result = ArtifactRepository::search(&state.db, &filters).await?;
    let rows = filter_artifacts_for_read(&state, &user, result.rows).await?;

    let items: Vec<ArtifactSummary> = rows.into_iter().map(ArtifactSummary::from).collect();

    let pagination = PaginationParams {
        page: query.page,
        page_size: query.per_page,
    };

    let response = PaginatedResponse::new(items, &pagination, result.total as u64);
    Ok((StatusCode::OK, Json(response)))
}

/// Get a single artifact by ID
#[utoipa::path(
    get,
    path = "/api/v1/artifacts/{id}",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID")),
    responses(
        (status = 200, description = "Artifact details", body = inline(ApiResponse<ArtifactResponse>)),
        (status = 404, description = "Artifact not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_artifact(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Read, &artifact)
        .await
        .map_err(|_| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(ArtifactResponse::from(artifact))),
    ))
}

/// Get a single artifact by ref
#[utoipa::path(
    get,
    path = "/api/v1/artifacts/ref/{ref}",
    tag = "artifacts",
    params(("ref" = String, Path, description = "Artifact reference")),
    responses(
        (status = 200, description = "Artifact details", body = inline(ApiResponse<ArtifactResponse>)),
        (status = 404, description = "Artifact not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_artifact_by_ref(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(artifact_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let artifact = ArtifactRepository::find_by_ref(&state.db, &artifact_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact '{}' not found", artifact_ref)))?;

    authorize_artifact_action(&state, &user, Action::Read, &artifact)
        .await
        .map_err(|_| ApiError::NotFound(format!("Artifact '{}' not found", artifact_ref)))?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(ArtifactResponse::from(artifact))),
    ))
}

/// Create a new artifact
#[utoipa::path(
    post,
    path = "/api/v1/artifacts",
    tag = "artifacts",
    request_body = CreateArtifactRequest,
    responses(
        (status = 201, description = "Artifact created", body = inline(ApiResponse<ArtifactResponse>)),
        (status = 400, description = "Validation error"),
        (status = 409, description = "Artifact with same ref already exists"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_artifact(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateArtifactRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate ref is not empty
    if request.r#ref.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Artifact ref must not be empty".to_string(),
        ));
    }

    // Check for duplicate ref
    if ArtifactRepository::find_by_ref(&state.db, &request.r#ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Artifact with ref '{}' already exists",
            request.r#ref
        )));
    }

    let classification = classify_artifact(&request.r#ref, request.r#type);
    let visibility = request
        .visibility
        .unwrap_or_else(|| default_visibility_for_artifact(request.r#type, classification));
    validate_runtime_log_visibility(classification, visibility)?;

    authorize_artifact_create(
        &state,
        &user,
        &request.r#ref,
        request.scope,
        &request.owner,
        visibility,
    )
    .await?;

    let (retention_policy, retention_limit) = resolve_artifact_retention(
        &state,
        &user,
        ArtifactRetentionDefaults {
            artifact_ref: &request.r#ref,
            scope: request.scope,
            owner: &request.owner,
            execution: None,
            requested_policy: request.retention_policy,
            requested_limit: request.retention_limit,
            fallback_limit: 5,
        },
    )
    .await?;

    let input = CreateArtifactInput {
        r#ref: request.r#ref,
        scope: request.scope,
        owner: request.owner,
        r#type: request.r#type,
        visibility,
        classification,
        retention_policy,
        retention_limit,
        name: request.name,
        description: request.description,
        content_type: request.content_type,
        data: request.data,
    };

    let artifact = ArtifactRepository::create(&state.db, input).await?;

    emit_artifact_audit(
        &state,
        &user,
        event_type::artifact::CREATED,
        AuditOutcome::Success,
        &artifact,
        serde_json::json!({
            "type": artifact.r#type,
            "scope": artifact.scope,
            "owner": artifact.owner,
            "visibility": artifact.visibility,
        }),
    );

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            ArtifactResponse::from(artifact),
            "Artifact created successfully",
        )),
    ))
}

/// Update an existing artifact
#[utoipa::path(
    put,
    path = "/api/v1/artifacts/{id}",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID")),
    request_body = UpdateArtifactRequest,
    responses(
        (status = 200, description = "Artifact updated", body = inline(ApiResponse<ArtifactResponse>)),
        (status = 404, description = "Artifact not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_artifact(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<UpdateArtifactRequest>,
) -> ApiResult<impl IntoResponse> {
    // Verify artifact exists
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Update, &artifact).await?;
    if let Some(visibility) = request.visibility {
        validate_runtime_log_visibility(artifact.classification, visibility)?;
    }

    let data_changed = request.data.is_some();
    let input = UpdateArtifactInput {
        r#ref: None, // Ref is immutable after creation
        scope: request.scope,
        owner: request.owner,
        r#type: request.r#type,
        visibility: request.visibility,
        classification: None,
        retention_policy: request.retention_policy,
        retention_limit: request.retention_limit,
        name: request.name.map(|patch| match patch {
            ArtifactStringPatch::Set(value) => Patch::Set(value),
            ArtifactStringPatch::Clear => Patch::Clear,
        }),
        description: request.description.map(|patch| match patch {
            ArtifactStringPatch::Set(value) => Patch::Set(value),
            ArtifactStringPatch::Clear => Patch::Clear,
        }),
        content_type: request.content_type.map(|patch| match patch {
            ArtifactStringPatch::Set(value) => Patch::Set(value),
            ArtifactStringPatch::Clear => Patch::Clear,
        }),
        size_bytes: None, // Managed by version creation trigger
        data: request.data.map(|patch| match patch {
            ArtifactJsonPatch::Set(value) => Patch::Set(value),
            ArtifactJsonPatch::Clear => Patch::Clear,
        }),
    };

    let updated = ArtifactRepository::update(&state.db, id, input).await?;

    emit_artifact_audit(
        &state,
        &user,
        event_type::artifact::UPDATED,
        AuditOutcome::Success,
        &updated,
        serde_json::json!({
            "type": updated.r#type,
            "scope": updated.scope,
            "owner": updated.owner,
            "visibility": updated.visibility,
            "data_changed": data_changed,
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            ArtifactResponse::from(updated),
            "Artifact updated successfully",
        )),
    ))
}

/// Delete an artifact (cascades to all versions, including disk files)
#[utoipa::path(
    delete,
    path = "/api/v1/artifacts/{id}",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID")),
    responses(
        (status = 200, description = "Artifact deleted", body = SuccessResponse),
        (status = 404, description = "Artifact not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_artifact(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Delete, &artifact).await?;

    // Before deleting DB rows, clean up any file-backed versions on disk
    let file_versions =
        ArtifactVersionRepository::find_file_versions_by_artifact(&state.db, id).await?;
    if !file_versions.is_empty() {
        let artifacts_dir = &state.config.artifacts_dir;
        cleanup_version_files(artifacts_dir, &file_versions);
        // Also try to remove the artifact's parent directory if it's now empty
        let ref_dir = ref_to_dir_path(&artifact.r#ref);
        let full_ref_dir = std::path::Path::new(artifacts_dir).join(&ref_dir);
        cleanup_empty_parents(&full_ref_dir, artifacts_dir);
    }

    let deleted = ArtifactRepository::delete(&state.db, id).await?;
    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Artifact with ID {} not found",
            id
        )));
    }

    emit_artifact_audit(
        &state,
        &user,
        event_type::artifact::DELETED,
        AuditOutcome::Success,
        &artifact,
        serde_json::json!({
            "type": artifact.r#type,
            "scope": artifact.scope,
            "owner": artifact.owner,
            "visibility": artifact.visibility,
            "file_versions_deleted": file_versions.len(),
        }),
    );

    Ok((
        StatusCode::OK,
        Json(SuccessResponse::new("Artifact deleted successfully")),
    ))
}

// ============================================================================
// Artifacts by Execution
// ============================================================================

/// List all artifacts for a given execution
#[utoipa::path(
    get,
    path = "/api/v1/executions/{execution_id}/artifacts",
    tag = "artifacts",
    params(("execution_id" = i64, Path, description = "Execution ID")),
    responses(
        (status = 200, description = "List of artifacts for execution", body = inline(ApiResponse<Vec<ArtifactSummary>>)),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_artifacts_by_execution(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(execution_id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let mut artifacts = ArtifactRepository::find_by_execution(&state.db, execution_id).await?;
    artifacts = filter_artifacts_for_read(&state, &user, artifacts).await?;
    let items: Vec<ArtifactSummary> = artifacts.into_iter().map(ArtifactSummary::from).collect();

    Ok((StatusCode::OK, Json(ApiResponse::new(items))))
}

// ============================================================================
// Progress Artifacts
// ============================================================================

/// Append an entry to a progress-type artifact's data array.
///
/// The entry is atomically appended to `artifact.data` (initialized as `[]` if null).
/// This is the primary mechanism for actions to stream progress updates.
#[utoipa::path(
    post,
    path = "/api/v1/artifacts/{id}/progress",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID (must be progress type)")),
    request_body = AppendProgressRequest,
    responses(
        (status = 200, description = "Entry appended", body = inline(ApiResponse<ArtifactResponse>)),
        (status = 400, description = "Artifact is not a progress type"),
        (status = 404, description = "Artifact not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn append_progress(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<AppendProgressRequest>,
) -> ApiResult<impl IntoResponse> {
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Update, &artifact).await?;

    if artifact.r#type != ArtifactType::Progress {
        return Err(ApiError::BadRequest(format!(
            "Artifact '{}' is type {:?}, not progress. Use version endpoints for file artifacts.",
            artifact.r#ref, artifact.r#type
        )));
    }

    let updated = ArtifactRepository::append_progress(&state.db, id, &request.entry).await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            ArtifactResponse::from(updated),
            "Progress entry appended",
        )),
    ))
}

/// Set the full data payload on an artifact (replaces existing data).
///
/// Useful for resetting progress, updating metadata, or setting structured content.
#[utoipa::path(
    put,
    path = "/api/v1/artifacts/{id}/data",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID")),
    request_body = SetDataRequest,
    responses(
        (status = 200, description = "Data set", body = inline(ApiResponse<ArtifactResponse>)),
        (status = 404, description = "Artifact not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn set_artifact_data(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<SetDataRequest>,
) -> ApiResult<impl IntoResponse> {
    // Verify exists
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Update, &artifact).await?;

    let updated = ArtifactRepository::set_data(&state.db, id, &request.data).await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            ArtifactResponse::from(updated),
            "Artifact data updated",
        )),
    ))
}

// ============================================================================
// Version Management
// ============================================================================

/// List all versions for an artifact (without binary content)
#[utoipa::path(
    get,
    path = "/api/v1/artifacts/{id}/versions",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID")),
    responses(
        (status = 200, description = "List of versions", body = inline(ApiResponse<Vec<ArtifactVersionSummary>>)),
        (status = 404, description = "Artifact not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_versions(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    // Verify artifact exists
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Read, &artifact)
        .await
        .map_err(|_| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    let versions = ArtifactVersionRepository::list_by_artifact(&state.db, id).await?;
    let items: Vec<ArtifactVersionSummary> = versions
        .into_iter()
        .map(ArtifactVersionSummary::from)
        .collect();

    Ok((StatusCode::OK, Json(ApiResponse::new(items))))
}

/// Get a specific version's metadata and JSON content (no binary)
#[utoipa::path(
    get,
    path = "/api/v1/artifacts/{id}/versions/{version}",
    tag = "artifacts",
    params(
        ("id" = i64, Path, description = "Artifact ID"),
        ("version" = i32, Path, description = "Version number"),
    ),
    responses(
        (status = 200, description = "Version details", body = inline(ApiResponse<ArtifactVersionResponse>)),
        (status = 404, description = "Artifact or version not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_version(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((id, version)): Path<(i64, i32)>,
) -> ApiResult<impl IntoResponse> {
    // Verify artifact exists
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Read, &artifact)
        .await
        .map_err(|_| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    let ver = ArtifactVersionRepository::find_by_version(&state.db, id, version)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Version {} not found for artifact {}", version, id))
        })?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(ArtifactVersionResponse::from(ver))),
    ))
}

/// Get the latest version's metadata and JSON content
#[utoipa::path(
    get,
    path = "/api/v1/artifacts/{id}/versions/latest",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID")),
    responses(
        (status = 200, description = "Latest version", body = inline(ApiResponse<ArtifactVersionResponse>)),
        (status = 404, description = "Artifact not found or no versions"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_latest_version(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Read, &artifact)
        .await
        .map_err(|_| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    let ver = ArtifactVersionRepository::find_latest(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("No versions found for artifact {}", id)))?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(ArtifactVersionResponse::from(ver))),
    ))
}

/// Create a new version with JSON content
#[utoipa::path(
    post,
    path = "/api/v1/artifacts/{id}/versions",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID")),
    request_body = CreateVersionJsonRequest,
    responses(
        (status = 201, description = "Version created", body = inline(ApiResponse<ArtifactVersionResponse>)),
        (status = 404, description = "Artifact not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_version_json(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<CreateVersionJsonRequest>,
) -> ApiResult<impl IntoResponse> {
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Update, &artifact).await?;

    let input = CreateArtifactVersionInput {
        artifact: id,
        execution: request.execution,
        content_type: Some(
            request
                .content_type
                .unwrap_or_else(|| "application/json".to_string()),
        ),
        content: None,
        content_json: Some(request.content),
        file_path: None,
        meta: request.meta,
        created_by: request.created_by,
    };

    let version = ArtifactVersionRepository::create(&state.db, input).await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            ArtifactVersionResponse::from(version),
            "Version created successfully",
        )),
    ))
}

/// Create a new file-backed version (no file content in request).
///
/// This endpoint allocates a version number and computes a `file_path` on the
/// shared artifact volume. The caller (execution process) is expected to write
/// the file content directly to `$ATTUNE_ARTIFACTS_DIR/{file_path}` after
/// receiving the response. The worker finalizes `size_bytes` after execution.
///
/// Only applicable to file-type artifacts (FileBinary, FileDatatable, FileText, Log).
#[utoipa::path(
    post,
    path = "/api/v1/artifacts/{id}/versions/file",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID")),
    request_body = CreateFileVersionRequest,
    responses(
        (status = 201, description = "File version allocated", body = inline(ApiResponse<ArtifactVersionResponse>)),
        (status = 400, description = "Artifact type is not file-based"),
        (status = 404, description = "Artifact not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_version_file(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(request): Json<CreateFileVersionRequest>,
) -> ApiResult<impl IntoResponse> {
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Update, &artifact).await?;

    // Validate this is a file-type artifact
    if !is_file_backed_type(artifact.r#type) {
        return Err(ApiError::BadRequest(format!(
            "Artifact '{}' is type {:?}, which does not support file-backed versions. \
             Use POST /versions for JSON or POST /versions/upload for DB-stored files.",
            artifact.r#ref, artifact.r#type,
        )));
    }

    let content_type = request
        .content_type
        .unwrap_or_else(|| default_content_type_for_artifact(artifact.r#type));
    let execution = request.execution.or_else(|| user.execution_id());

    let version = ArtifactVersionRepository::create_file_backed(
        &state.db,
        id,
        &artifact.r#ref,
        content_type.clone(),
        execution,
        request.meta,
        request.created_by,
    )
    .await?;
    let file_path = version.file_path.clone().ok_or_else(|| {
        ApiError::InternalServerError(format!(
            "Allocated file-backed version {} is missing file_path",
            version.id
        ))
    })?;

    // Create the parent directory on disk with group-writable permissions
    // so both API (UID 1000) and workers (root + GID 1000) can write.
    let artifacts_dir = &state.config.artifacts_dir;
    let full_path = std::path::Path::new(artifacts_dir).join(&file_path);
    if let Some(parent) = full_path.parent() {
        attune_common::utils::create_shared_dir_all(parent)
            .await
            .map_err(|e| {
                ApiError::InternalServerError(format!(
                    "Failed to create artifact directory '{}': {}",
                    parent.display(),
                    e,
                ))
            })?;
    }

    let response = ArtifactVersionResponse::from(version);

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            response,
            "File version allocated — write content to $ATTUNE_ARTIFACTS_DIR/<file_path>",
        )),
    ))
}

/// Upload a binary file as a new version (multipart/form-data)
///
/// The file is sent as a multipart form field named `file`. Optional fields:
/// - `content_type`: MIME type override (auto-detected from filename if omitted)
/// - `meta`: JSON metadata string
/// - `created_by`: Creator identifier
#[utoipa::path(
    post,
    path = "/api/v1/artifacts/{id}/versions/upload",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID")),
    request_body(content = String, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "File version created", body = inline(ApiResponse<ArtifactVersionResponse>)),
        (status = 400, description = "Missing file field"),
        (status = 404, description = "Artifact not found"),
        (status = 413, description = "File too large"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn upload_version(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> ApiResult<impl IntoResponse> {
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Update, &artifact).await?;

    let mut file_data: Option<Vec<u8>> = None;
    let mut content_type: Option<String> = None;
    let mut meta: Option<serde_json::Value> = None;
    let mut created_by: Option<String> = None;
    let mut file_content_type: Option<String> = None;

    // 50 MB limit
    const MAX_FILE_SIZE: usize = 50 * 1024 * 1024;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Multipart error: {}", e)))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                // Capture content type from the multipart field itself
                file_content_type = field.content_type().map(|s| s.to_string());

                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("Failed to read file: {}", e)))?;

                if bytes.len() > MAX_FILE_SIZE {
                    return Err(ApiError::BadRequest(format!(
                        "File exceeds maximum size of {} bytes",
                        MAX_FILE_SIZE
                    )));
                }

                file_data = Some(bytes.to_vec());
            }
            "content_type" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("Failed to read field: {}", e)))?;
                if !text.is_empty() {
                    content_type = Some(text);
                }
            }
            "meta" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("Failed to read field: {}", e)))?;
                if !text.is_empty() {
                    meta =
                        Some(serde_json::from_str(&text).map_err(|e| {
                            ApiError::BadRequest(format!("Invalid meta JSON: {}", e))
                        })?);
                }
            }
            "created_by" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("Failed to read field: {}", e)))?;
                if !text.is_empty() {
                    created_by = Some(text);
                }
            }
            _ => {
                // Skip unknown fields
            }
        }
    }

    let file_bytes = file_data.ok_or_else(|| {
        ApiError::BadRequest("Missing required 'file' field in multipart upload".to_string())
    })?;

    // Resolve content type: explicit > multipart header > fallback
    let resolved_ct = content_type
        .or(file_content_type)
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let input = CreateArtifactVersionInput {
        artifact: id,
        execution: None,
        content_type: Some(resolved_ct),
        content: Some(file_bytes),
        content_json: None,
        file_path: None,
        meta,
        created_by,
    };

    let version = ArtifactVersionRepository::create(&state.db, input).await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            ArtifactVersionResponse::from(version),
            "File version uploaded successfully",
        )),
    ))
}

/// Download the binary content of a specific version.
///
/// For file-backed versions, reads from the shared artifact volume on disk.
/// For DB-stored versions, reads from the BYTEA/JSON content column.
#[utoipa::path(
    get,
    path = "/api/v1/artifacts/{id}/versions/{version}/download",
    tag = "artifacts",
    params(
        ("id" = i64, Path, description = "Artifact ID"),
        ("version" = i32, Path, description = "Version number"),
    ),
    responses(
        (status = 200, description = "Binary file content", content_type = "application/octet-stream"),
        (status = 404, description = "Artifact, version, or content not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn download_version(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((id, version)): Path<(i64, i32)>,
) -> ApiResult<impl IntoResponse> {
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Read, &artifact)
        .await
        .map_err(|_| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    // First try without content (cheaper query) to check for file_path
    let ver = ArtifactVersionRepository::find_by_version(&state.db, id, version)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Version {} not found for artifact {}", version, id))
        })?;

    emit_artifact_audit(
        &state,
        &user,
        event_type::artifact::DOWNLOADED,
        AuditOutcome::Success,
        &artifact,
        serde_json::json!({
            "version": version,
            "file_backed": ver.file_path.is_some(),
            "content_type": ver.content_type.as_deref(),
            "size_bytes": ver.size_bytes,
        }),
    );

    // File-backed version: read from disk
    if let Some(ref file_path) = ver.file_path {
        return serve_file_from_disk(
            &state.config.artifacts_dir,
            file_path,
            &artifact.r#ref,
            version,
            ver.content_type.as_deref(),
        )
        .await;
    }

    // DB-stored version: need to fetch with content
    let ver = ArtifactVersionRepository::find_by_version_with_content(&state.db, id, version)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Version {} not found for artifact {}", version, id))
        })?;

    serve_db_content(&artifact.r#ref, version, &ver)
}

/// Download the latest version's content
#[utoipa::path(
    get,
    path = "/api/v1/artifacts/{id}/download",
    tag = "artifacts",
    params(("id" = i64, Path, description = "Artifact ID")),
    responses(
        (status = 200, description = "Binary file content of latest version", content_type = "application/octet-stream"),
        (status = 404, description = "Artifact not found or no versions"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn download_latest(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Read, &artifact)
        .await
        .map_err(|_| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    // First try without content (cheaper query) to check for file_path.
    // Sensor log artifacts may exist before a stream has emitted any bytes
    // (especially stderr), so no-version logs are served as empty text instead
    // of a broken download.
    let Some(ver) = ArtifactVersionRepository::find_latest(&state.db, id).await? else {
        if let Some((sensor_ref, stream)) = sensor_log_artifact_parts(&artifact.r#ref) {
            return serve_sensor_log_legacy_or_empty(
                &state.config.artifacts_dir,
                &artifact.r#ref,
                &sensor_ref,
                stream,
            )
            .await;
        }
        return Err(ApiError::NotFound(format!(
            "No versions found for artifact {}",
            id
        )));
    };

    let version = ver.version;

    emit_artifact_audit(
        &state,
        &user,
        event_type::artifact::DOWNLOADED,
        AuditOutcome::Success,
        &artifact,
        serde_json::json!({
            "version": version,
            "file_backed": ver.file_path.is_some(),
            "content_type": ver.content_type.as_deref(),
            "size_bytes": ver.size_bytes,
        }),
    );

    // File-backed version: read from disk
    if let Some(ref file_path) = ver.file_path {
        if sensor_log_artifact_parts(&artifact.r#ref).is_some() {
            match serve_file_from_disk(
                &state.config.artifacts_dir,
                file_path,
                &artifact.r#ref,
                version,
                ver.content_type.as_deref(),
            )
            .await
            {
                Ok(response) => return Ok(response),
                Err(ApiError::NotFound(_)) => return serve_empty_sensor_log(&artifact.r#ref),
                Err(ApiError::Forbidden(_)) => return serve_empty_sensor_log(&artifact.r#ref),
                Err(err) => return Err(err),
            }
        }

        return serve_file_from_disk(
            &state.config.artifacts_dir,
            file_path,
            &artifact.r#ref,
            version,
            ver.content_type.as_deref(),
        )
        .await;
    }

    // DB-stored version: need to fetch with content
    let ver = ArtifactVersionRepository::find_latest_with_content(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("No versions found for artifact {}", id)))?;

    serve_db_content(&artifact.r#ref, ver.version, &ver)
}

/// Delete a specific version by version number (including disk file if file-backed)
#[utoipa::path(
    delete,
    path = "/api/v1/artifacts/{id}/versions/{version}",
    tag = "artifacts",
    params(
        ("id" = i64, Path, description = "Artifact ID"),
        ("version" = i32, Path, description = "Version number"),
    ),
    responses(
        (status = 200, description = "Version deleted", body = SuccessResponse),
        (status = 404, description = "Artifact or version not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_version(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((id, version)): Path<(i64, i32)>,
) -> ApiResult<impl IntoResponse> {
    // Verify artifact exists
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Delete, &artifact).await?;

    // Find the version by artifact + version number
    let ver = ArtifactVersionRepository::find_by_version(&state.db, id, version)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Version {} not found for artifact {}", version, id))
        })?;

    // Clean up disk file if file-backed
    if let Some(ref file_path) = ver.file_path {
        let artifacts_dir = &state.config.artifacts_dir;
        let full_path = std::path::Path::new(artifacts_dir).join(file_path);
        if full_path.exists() {
            if let Err(e) = tokio::fs::remove_file(&full_path).await {
                warn!(
                    "Failed to delete artifact file '{}': {}. DB row will still be deleted.",
                    full_path.display(),
                    e
                );
            }
        }
        // Try to clean up empty parent directories
        let ref_dir = ref_to_dir_path(&artifact.r#ref);
        let full_ref_dir = std::path::Path::new(artifacts_dir).join(&ref_dir);
        cleanup_empty_parents(&full_ref_dir, artifacts_dir);
    }

    ArtifactVersionRepository::delete(&state.db, ver.id).await?;

    Ok((
        StatusCode::OK,
        Json(SuccessResponse::new("Version deleted successfully")),
    ))
}

// ============================================================================
// Upsert-and-upload by ref
// ============================================================================

/// Upload a file version to an artifact identified by ref, creating the artifact if it does not
/// already exist.
///
/// This is the recommended way for actions to produce versioned file artifacts. The caller
/// provides the artifact ref and file content in a single multipart request. The server:
///
/// 1. Looks up the artifact by `ref`.
/// 2. If not found, creates it using the metadata fields in the multipart body.
/// 3. If found, optionally updates the `execution` link to the current execution.
/// 4. Uploads the file bytes as a new version (version number is auto-assigned).
///
/// **Multipart fields:**
/// - `file` (required) — the binary file content
/// - `ref` (required for creation) — artifact reference (ignored if artifact already exists)
/// - `scope` — owner scope: `system`, `pack`, `action`, `sensor`, `rule` (default: `action`)
/// - `owner` — owner identifier (default: empty string)
/// - `type` — artifact type: `file_text`, `file_image`, etc. (default: `file_text`)
/// - `visibility` — `public` or `private` (default: type-aware server default)
/// - `name` — human-readable name
/// - `description` — optional description
/// - `content_type` — MIME type (default: auto-detected from multipart or `application/octet-stream`)
/// - `execution` — execution ID to link this artifact to (updates existing artifacts too)
/// - `retention_policy` — `versions`, `days`, `hours`, `minutes` (default: `versions`)
/// - `retention_limit` — limit value (default: `10`)
/// - `created_by` — who created this version
/// - `meta` — JSON metadata for this version
#[utoipa::path(
    post,
    path = "/api/v1/artifacts/ref/{ref}/versions/upload",
    tag = "artifacts",
    params(("ref" = String, Path, description = "Artifact reference (created if not found)")),
    request_body(content = String, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Version created (artifact may have been created too)", body = inline(ApiResponse<ArtifactVersionResponse>)),
        (status = 400, description = "Missing file field or invalid metadata"),
        (status = 413, description = "File too large"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn upload_version_by_ref(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(artifact_ref): Path<String>,
    mut multipart: Multipart,
) -> ApiResult<impl IntoResponse> {
    // 50 MB limit
    const MAX_FILE_SIZE: usize = 50 * 1024 * 1024;

    // Collect all multipart fields
    let mut file_data: Option<Vec<u8>> = None;
    let mut file_content_type: Option<String> = None;
    let mut content_type_field: Option<String> = None;
    let mut meta: Option<serde_json::Value> = None;
    let mut created_by: Option<String> = None;

    // Artifact-creation metadata (used only when creating a new artifact)
    let mut scope: Option<String> = None;
    let mut owner: Option<String> = None;
    let mut artifact_type: Option<String> = None;
    let mut visibility: Option<String> = None;
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut execution: Option<String> = None;
    let mut retention_policy: Option<String> = None;
    let mut retention_limit: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Multipart error: {}", e)))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "file" => {
                file_content_type = field.content_type().map(|s| s.to_string());
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("Failed to read file: {}", e)))?;
                if bytes.len() > MAX_FILE_SIZE {
                    return Err(ApiError::BadRequest(format!(
                        "File exceeds maximum size of {} bytes",
                        MAX_FILE_SIZE
                    )));
                }
                file_data = Some(bytes.to_vec());
            }
            "content_type" => {
                let t = field.text().await.unwrap_or_default();
                if !t.is_empty() {
                    content_type_field = Some(t);
                }
            }
            "meta" => {
                let t = field.text().await.unwrap_or_default();
                if !t.is_empty() {
                    meta =
                        Some(serde_json::from_str(&t).map_err(|e| {
                            ApiError::BadRequest(format!("Invalid meta JSON: {}", e))
                        })?);
                }
            }
            "created_by" => {
                let t = field.text().await.unwrap_or_default();
                if !t.is_empty() {
                    created_by = Some(t);
                }
            }
            "scope" => {
                scope = Some(field.text().await.unwrap_or_default());
            }
            "owner" => {
                owner = Some(field.text().await.unwrap_or_default());
            }
            "type" => {
                artifact_type = Some(field.text().await.unwrap_or_default());
            }
            "visibility" => {
                visibility = Some(field.text().await.unwrap_or_default());
            }
            "name" => {
                name = Some(field.text().await.unwrap_or_default());
            }
            "description" => {
                description = Some(field.text().await.unwrap_or_default());
            }
            "execution" => {
                execution = Some(field.text().await.unwrap_or_default());
            }
            "retention_policy" => {
                retention_policy = Some(field.text().await.unwrap_or_default());
            }
            "retention_limit" => {
                retention_limit = Some(field.text().await.unwrap_or_default());
            }
            _ => { /* skip unknown fields */ }
        }
    }

    let file_bytes = file_data.ok_or_else(|| {
        ApiError::BadRequest("Missing required 'file' field in multipart upload".to_string())
    })?;

    // Parse execution ID
    let execution_id: Option<i64> = match &execution {
        Some(s) if !s.is_empty() => Some(
            s.parse::<i64>()
                .map_err(|_| ApiError::BadRequest(format!("Invalid execution ID: '{}'", s)))?,
        ),
        _ => user.execution_id(),
    };

    // Upsert: find existing artifact or create a new one
    let artifact = match ArtifactRepository::find_by_ref(&state.db, &artifact_ref).await? {
        Some(existing) => {
            authorize_artifact_action(&state, &user, Action::Update, &existing).await?;
            existing
        }
        None => {
            // Parse artifact type
            let a_type: ArtifactType = match &artifact_type {
                Some(t) => serde_json::from_value(serde_json::Value::String(t.clone()))
                    .map_err(|_| ApiError::BadRequest(format!("Invalid artifact type: '{}'", t)))?,
                None => ArtifactType::FileText,
            };

            // Parse scope
            let a_scope: OwnerType = match &scope {
                Some(s) if !s.is_empty() => {
                    serde_json::from_value(serde_json::Value::String(s.clone()))
                        .map_err(|_| ApiError::BadRequest(format!("Invalid scope: '{}'", s)))?
                }
                _ => OwnerType::Action,
            };

            // Parse visibility with type-aware default
            let classification = classify_artifact(&artifact_ref, a_type);
            let a_visibility: ArtifactVisibility = match &visibility {
                Some(v) if !v.is_empty() => {
                    serde_json::from_value(serde_json::Value::String(v.clone()))
                        .map_err(|_| ApiError::BadRequest(format!("Invalid visibility: '{}'", v)))?
                }
                _ => default_visibility_for_artifact(a_type, classification),
            };
            validate_runtime_log_visibility(classification, a_visibility)?;

            authorize_artifact_create(
                &state,
                &user,
                &artifact_ref,
                a_scope,
                owner.as_deref().unwrap_or_default(),
                a_visibility,
            )
            .await?;

            let requested_retention_policy: Option<RetentionPolicyType> = match &retention_policy {
                Some(rp) if !rp.is_empty() => Some(
                    serde_json::from_value(serde_json::Value::String(rp.clone())).map_err(
                        |_| ApiError::BadRequest(format!("Invalid retention_policy: '{}'", rp)),
                    )?,
                ),
                _ => None,
            };
            let requested_retention_limit: Option<i32> = match &retention_limit {
                Some(rl) if !rl.is_empty() => Some(rl.parse::<i32>().map_err(|_| {
                    ApiError::BadRequest(format!("Invalid retention_limit: '{}'", rl))
                })?),
                _ => None,
            };
            let (a_retention_policy, a_retention_limit) = resolve_artifact_retention(
                &state,
                &user,
                ArtifactRetentionDefaults {
                    artifact_ref: &artifact_ref,
                    scope: a_scope,
                    owner: owner.as_deref().unwrap_or_default(),
                    execution: execution_id,
                    requested_policy: requested_retention_policy,
                    requested_limit: requested_retention_limit,
                    fallback_limit: 10,
                },
            )
            .await?;

            let create_input = CreateArtifactInput {
                r#ref: artifact_ref.clone(),
                scope: a_scope,
                owner: owner.unwrap_or_default(),
                r#type: a_type,
                visibility: a_visibility,
                classification,
                retention_policy: a_retention_policy,
                retention_limit: a_retention_limit,
                name: name.filter(|s| !s.is_empty()),
                description: description.filter(|s| !s.is_empty()),
                content_type: content_type_field
                    .clone()
                    .or_else(|| file_content_type.clone()),
                data: None,
            };

            ArtifactRepository::create(&state.db, create_input).await?
        }
    };

    // Resolve content type: explicit field > multipart header > fallback
    let resolved_ct = content_type_field
        .or(file_content_type)
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let version_input = CreateArtifactVersionInput {
        artifact: artifact.id,
        execution: execution_id,
        content_type: Some(resolved_ct),
        content: Some(file_bytes),
        content_json: None,
        file_path: None,
        meta,
        created_by,
    };

    let version = ArtifactVersionRepository::create(&state.db, version_input).await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            ArtifactVersionResponse::from(version),
            "Version uploaded successfully",
        )),
    ))
}

/// Upsert an artifact by ref and allocate a file-backed version in one call.
///
/// If the artifact doesn't exist, it is created using the supplied metadata.
/// If it already exists, the execution link is updated (if provided).
/// Then a new file-backed version is allocated and the `file_path` is returned.
///
/// The caller writes the file to `$ATTUNE_ARTIFACTS_DIR/{file_path}` on the
/// shared volume — no HTTP upload needed.
#[utoipa::path(
    post,
    path = "/api/v1/artifacts/ref/{ref}/versions/file",
    tag = "artifacts",
    params(
        ("ref" = String, Path, description = "Artifact reference (e.g. 'mypack.build_log')")
    ),
    request_body = AllocateFileVersionByRefRequest,
    responses(
        (status = 201, description = "File version allocated", body = inline(ApiResponse<ArtifactVersionResponse>)),
        (status = 400, description = "Invalid request (non-file-backed artifact type)"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn allocate_file_version_by_ref(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(artifact_ref): Path<String>,
    Json(request): Json<AllocateFileVersionByRefRequest>,
) -> ApiResult<impl IntoResponse> {
    let execution = request.execution.or_else(|| user.execution_id());

    // Upsert: find existing artifact or create a new one
    let artifact = match ArtifactRepository::find_by_ref(&state.db, &artifact_ref).await? {
        Some(existing) => {
            authorize_artifact_action(&state, &user, Action::Update, &existing).await?;
            existing
        }
        None => {
            // Parse artifact type (default to FileText)
            let a_type = request.r#type.unwrap_or(ArtifactType::FileText);

            // Validate it's a file-backed type
            if !is_file_backed_type(a_type) {
                return Err(ApiError::BadRequest(format!(
                    "Artifact type {:?} is not file-backed. \
                     Use POST /artifacts/ref/{{ref}}/versions/upload for DB-stored artifacts.",
                    a_type,
                )));
            }

            let a_scope = request.scope.unwrap_or(OwnerType::Action);
            let classification = classify_artifact(&artifact_ref, a_type);
            let a_visibility = request
                .visibility
                .unwrap_or_else(|| default_visibility_for_artifact(a_type, classification));
            validate_runtime_log_visibility(classification, a_visibility)?;
            let owner_ref = request.owner.as_deref().unwrap_or_default();
            let (a_retention_policy, a_retention_limit) = resolve_artifact_retention(
                &state,
                &user,
                ArtifactRetentionDefaults {
                    artifact_ref: &artifact_ref,
                    scope: a_scope,
                    owner: owner_ref,
                    execution,
                    requested_policy: request.retention_policy,
                    requested_limit: request.retention_limit,
                    fallback_limit: 10,
                },
            )
            .await?;

            authorize_artifact_create(
                &state,
                &user,
                &artifact_ref,
                a_scope,
                owner_ref,
                a_visibility,
            )
            .await?;

            let create_input = CreateArtifactInput {
                r#ref: artifact_ref.clone(),
                scope: a_scope,
                owner: request.owner.unwrap_or_default(),
                r#type: a_type,
                visibility: a_visibility,
                classification,
                retention_policy: a_retention_policy,
                retention_limit: a_retention_limit,
                name: request.name,
                description: request.description,
                content_type: request.content_type.clone(),
                data: None,
            };

            ArtifactRepository::create(&state.db, create_input).await?
        }
    };

    // Validate the existing artifact is file-backed
    if !is_file_backed_type(artifact.r#type) {
        return Err(ApiError::BadRequest(format!(
            "Artifact '{}' is type {:?}, which does not support file-backed versions.",
            artifact.r#ref, artifact.r#type,
        )));
    }

    let content_type = request
        .content_type
        .unwrap_or_else(|| default_content_type_for_artifact(artifact.r#type));

    let version = ArtifactVersionRepository::create_file_backed(
        &state.db,
        artifact.id,
        &artifact.r#ref,
        content_type.clone(),
        execution,
        request.meta,
        request.created_by,
    )
    .await?;
    let file_path = version.file_path.clone().ok_or_else(|| {
        ApiError::InternalServerError(format!(
            "Allocated file-backed version {} is missing file_path",
            version.id
        ))
    })?;

    // Create the parent directory on disk with group-writable permissions
    let artifacts_dir = &state.config.artifacts_dir;
    let full_path = std::path::Path::new(artifacts_dir).join(&file_path);
    if let Some(parent) = full_path.parent() {
        attune_common::utils::create_shared_dir_all(parent)
            .await
            .map_err(|e| {
                ApiError::InternalServerError(format!(
                    "Failed to create artifact directory '{}': {}",
                    parent.display(),
                    e,
                ))
            })?;
    }

    let response = ArtifactVersionResponse::from(version);

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            response,
            "File version allocated — write content to $ATTUNE_ARTIFACTS_DIR/<file_path>",
        )),
    ))
}

// ============================================================================
// Helpers
// ============================================================================

/// Derive the parent pack ref for an artifact whose scope is Pack/Action/Sensor.
///
/// - `OwnerType::Pack`: owner _is_ the pack ref.
/// - `OwnerType::Action`/`OwnerType::Sensor`: owner is `<pack_ref>.<entity_name>`,
///   so the pack is the segment before the first `.`.
/// - Any other scope (Identity, System, Rule, …) returns `None`.
fn derive_pack_ref(scope: OwnerType, owner: &str) -> Option<&str> {
    match scope {
        OwnerType::Pack => {
            if owner.is_empty() {
                None
            } else {
                Some(owner)
            }
        }
        OwnerType::Action | OwnerType::Sensor => {
            // Require `<pack>.<entity>` form: at least one dot, non-empty pack
            // segment. A dotless owner like `"action"` is malformed and must
            // not be silently treated as a pack ref.
            let mut parts = owner.splitn(2, '.');
            let pack = parts.next()?;
            parts.next()?;
            if pack.is_empty() {
                None
            } else {
                Some(pack)
            }
        }
        _ => None,
    }
}

/// True when `scope` is one for which `derive_pack_ref` is expected to yield
/// a pack — i.e., a `None` return for these scopes indicates a malformed
/// owner ref rather than a scope that simply has no pack.
fn scope_requires_pack(scope: OwnerType) -> bool {
    matches!(
        scope,
        OwnerType::Pack | OwnerType::Action | OwnerType::Sensor
    )
}

/// Read the executing action ref out of an Execution-token's metadata.
fn execution_token_action_ref(user: &AuthenticatedUser) -> Option<&str> {
    user.claims
        .metadata
        .as_ref()
        .and_then(|m| m.get("action_ref"))
        .and_then(|v| v.as_str())
}

/// Derive the pack ref from an Execution token's `action_ref` metadata.
/// Returns `None` if the token has no `action_ref`, or if the `action_ref`
/// is malformed (empty or with an empty leading segment).
fn execution_token_pack_ref(user: &AuthenticatedUser) -> Option<&str> {
    execution_token_action_ref(user)
        .and_then(|s| s.split('.').next())
        .filter(|s| !s.is_empty())
}

/// Reject Execution tokens that try to mutate an artifact owned by a different
/// pack than the executing action's own pack. Identity-scoped artifacts are
/// not subject to this guard (they fall through to the standard owner check).
fn execution_token_cross_pack_guard(
    user: &AuthenticatedUser,
    artifact: &attune_common::models::artifact::Artifact,
) -> Result<(), ApiError> {
    if user.claims.token_type != TokenType::Execution {
        return Ok(());
    }
    let artifact_pack = match derive_pack_ref(artifact.scope, &artifact.owner) {
        Some(p) => p,
        None => {
            // Pack-derivable scope (Pack/Action/Sensor) with a malformed owner
            // — refuse rather than silently bypass the cross-pack guard.
            if scope_requires_pack(artifact.scope) {
                return Err(ApiError::Forbidden(
                    "Artifact has malformed pack-scoped owner; mutation refused".to_string(),
                ));
            }
            // Identity/system-scoped artifact — no pack to compare against.
            return Ok(());
        }
    };
    let token_pack = execution_token_pack_ref(user).ok_or_else(|| {
        ApiError::Forbidden(
            "Execution token missing or malformed action_ref; cross-pack artifact mutation refused"
                .to_string(),
        )
    })?;
    if token_pack != artifact_pack
        && !(execution_has_standard_access(user)
            && execution_standard_pack_refs(user)
                .iter()
                .any(|pack_ref| pack_ref == artifact_pack))
    {
        return Err(ApiError::Forbidden(format!(
            "Execution token from pack '{}' cannot mutate artifact owned by pack '{}'",
            token_pack, artifact_pack
        )));
    }
    Ok(())
}

/// Resolve a read/write of `artifact` against the calling user.
///
/// Visibility × scope policy:
///
/// - `visibility = public`: any identity holding `artifacts:<action>` (subject
///   to that grant's own constraints) is allowed.
/// - `visibility = private`:
///   - **Constrained** `artifacts:<action>` grants — i.e., grants whose
///     `constraints` field is set — apply normally. This lets operators
///     explicitly delegate access (e.g. `owner_refs: ["pack_x"]`).
///   - **Unconstrained** `artifacts:<action>` grants do *not* unlock private
///     artifacts on their own; they only cover public artifacts.
///   - Identity-scoped private artifacts: only the owning identity may act.
///   - Pack/Action/Sensor-scoped private artifacts: a `packs:<read|configure>`
///     grant covering the derived pack ref also unlocks the artifact.
///
/// Execution-token writes are additionally subject to the cross-pack guard:
/// the token's `action_ref` must live in the same pack as the artifact.
async fn authorize_artifact_action(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: Action,
    artifact: &attune_common::models::artifact::Artifact,
) -> Result<(), ApiError> {
    // Sensor / Refresh tokens are not subject to identity-RBAC artifact checks
    // (they have dedicated scope checks at their entry points).
    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return Ok(());
    }

    // Cross-pack write guard for execution tokens (writes only).
    if action != Action::Read {
        execution_token_cross_pack_guard(user, artifact)?;
    }

    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let authz = AuthorizationService::new(state.db.clone());

    let ctx = artifact_authorization_context(identity_id, artifact);

    // Public artifacts: any matching `artifacts:<action>` grant suffices.
    if artifact.visibility == ArtifactVisibility::Public {
        return authz
            .authorize(
                user,
                AuthorizationCheck {
                    resource: Resource::Artifacts,
                    action,
                    context: ctx,
                },
            )
            .await;
    }

    // Private artifacts: load the caller's effective grants once and try the
    // permitted paths in order.
    let grants = authz.effective_grants(user).await?;
    let denied = || {
        ApiError::Forbidden(format!(
            "Insufficient permissions: artifacts:{}",
            action_name_lower(action)
        ))
    };

    // (a) Constrained `artifacts:<action>` grant. Only grants with explicit
    //     `constraints` count — an unconstrained grant is treated as a
    //     "public-only" baseline and does not unlock private artifacts.
    let constrained_match = grants.iter().any(|g| {
        g.resource == Resource::Artifacts
            && g.actions.contains(&action)
            && g.constraints.is_some()
            && g.allows(Resource::Artifacts, action, &ctx)
    });
    if constrained_match {
        return Ok(());
    }

    // (b) Identity-scoped private artifact: owner-only.
    if artifact.scope == OwnerType::Identity {
        if let Ok(owner_id) = artifact.owner.parse::<i64>() {
            if owner_id == identity_id {
                return Ok(());
            }
        }
        return Err(denied());
    }

    // (c) Pack/Action/Sensor-scoped: defer to parent-pack permissions. Reads
    //     require `packs:read`; writes require `packs:configure`.
    if let Some(pack_ref) = derive_pack_ref(artifact.scope, &artifact.owner) {
        let pack_action = match action {
            Action::Read => Action::Read,
            _ => Action::Configure,
        };
        let mut pack_ctx = AuthorizationContext::new(identity_id);
        pack_ctx.pack_ref = Some(pack_ref.to_string());
        pack_ctx.target_ref = Some(pack_ref.to_string());
        if grants
            .iter()
            .any(|g| g.allows(Resource::Packs, pack_action, &pack_ctx))
        {
            return Ok(());
        }
    }

    Err(denied())
}

async fn authorize_artifact_create(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    artifact_ref: &str,
    scope: OwnerType,
    owner: &str,
    visibility: ArtifactVisibility,
) -> Result<(), ApiError> {
    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return Ok(());
    }

    // Execution-token cross-pack guard for new artifacts. Standard access can
    // create artifacts scoped to the executing action/pack or workflow
    // action/pack; otherwise explicit permission-set grants are required and
    // still cannot cross the token's standard pack boundary.
    if user.claims.token_type == TokenType::Execution {
        if let Some(target_pack) = derive_pack_ref(scope, owner) {
            let token_pack = execution_token_pack_ref(user).ok_or_else(|| {
                ApiError::Forbidden(
                    "Execution token missing or malformed action_ref; cannot create pack-scoped artifact"
                        .to_string(),
                )
            })?;
            if token_pack != target_pack
                && !(execution_has_standard_access(user)
                    && execution_standard_pack_refs(user)
                        .iter()
                        .any(|pack_ref| pack_ref == target_pack))
            {
                return Err(ApiError::Forbidden(format!(
                    "Execution token from pack '{}' cannot create artifact owned by pack '{}'",
                    token_pack, target_pack
                )));
            }
        } else if scope_requires_pack(scope) {
            // Pack-derivable scope with malformed owner — refuse rather than
            // fall through to the standard RBAC path.
            return Err(ApiError::Forbidden(
                "Artifact owner is malformed for pack-scoped create; refused".to_string(),
            ));
        }
    }

    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let authz = AuthorizationService::new(state.db.clone());
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_ref = Some(artifact_ref.to_string());
    ctx.owner_type = Some(scope);
    ctx.owner_ref = Some(owner.to_string());
    ctx.visibility = Some(visibility);
    if let Some(pack_ref) = derive_pack_ref(scope, owner) {
        ctx.pack_ref = Some(pack_ref.to_string());
    }
    if scope == OwnerType::Identity {
        if let Ok(owner_id) = owner.parse::<i64>() {
            ctx.owner_identity_id = Some(owner_id);
        }
    }

    let denied = || {
        ApiError::Forbidden(format!(
            "Insufficient permissions: artifacts:create on owner '{}'",
            owner
        ))
    };

    // Public artifact creation: any matching `artifacts:create` grant suffices.
    if visibility == ArtifactVisibility::Public {
        return authz
            .authorize(
                user,
                AuthorizationCheck {
                    resource: Resource::Artifacts,
                    action: Action::Create,
                    context: ctx,
                },
            )
            .await;
    }

    // Private artifact creation: same constrained-grant rule as reads.
    let grants = authz.effective_grants(user).await?;

    let constrained_match = grants.iter().any(|g| {
        g.resource == Resource::Artifacts
            && g.actions.contains(&Action::Create)
            && g.constraints.is_some()
            && g.allows(Resource::Artifacts, Action::Create, &ctx)
    });
    if constrained_match {
        return Ok(());
    }

    // Identity-scoped self-create.
    if scope == OwnerType::Identity {
        if let Ok(owner_id) = owner.parse::<i64>() {
            if owner_id == identity_id {
                return Ok(());
            }
        }
        return Err(denied());
    }

    // Pack-derived fallback: `packs:configure` on the parent pack.
    if let Some(pack_ref) = derive_pack_ref(scope, owner) {
        let mut pack_ctx = AuthorizationContext::new(identity_id);
        pack_ctx.pack_ref = Some(pack_ref.to_string());
        pack_ctx.target_ref = Some(pack_ref.to_string());
        if grants
            .iter()
            .any(|g| g.allows(Resource::Packs, Action::Configure, &pack_ctx))
        {
            return Ok(());
        }
    }

    Err(denied())
}

/// Filter a list of artifacts to those the calling user is authorized to read.
///
/// Returns the filtered list. For non-Access/Execution tokens (sensor, refresh,
/// …) the list is returned unchanged because those tokens are not subject to
/// identity-RBAC artifact checks.
async fn filter_artifacts_for_read(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    artifacts: Vec<attune_common::models::artifact::Artifact>,
) -> Result<Vec<attune_common::models::artifact::Artifact>, ApiError> {
    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return Ok(artifacts);
    }

    // Per-row authorization. This is O(N) checks per page, which is acceptable
    // at typical page sizes (≤100). A SQL-level filter is a future optimization
    // (see AGENTS.md for the artifact RBAC design notes).
    let mut allowed = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        if authorize_artifact_action(state, user, Action::Read, &artifact)
            .await
            .is_ok()
        {
            allowed.push(artifact);
        }
    }
    Ok(allowed)
}

fn artifact_authorization_context(
    identity_id: i64,
    artifact: &attune_common::models::artifact::Artifact,
) -> AuthorizationContext {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_id = Some(artifact.id);
    ctx.target_ref = Some(artifact.r#ref.clone());
    ctx.owner_type = Some(artifact.scope);
    ctx.owner_ref = Some(artifact.owner.clone());
    ctx.visibility = Some(artifact.visibility);
    if let Some(pack_ref) = derive_pack_ref(artifact.scope, &artifact.owner) {
        ctx.pack_ref = Some(pack_ref.to_string());
    }
    if artifact.scope == OwnerType::Identity {
        if let Ok(owner_id) = artifact.owner.parse::<i64>() {
            ctx.owner_identity_id = Some(owner_id);
        }
    }
    ctx
}

fn action_name_lower(action: Action) -> &'static str {
    match action {
        Action::Read => "read",
        Action::Create => "create",
        Action::Install => "install",
        Action::Configure => "configure",
        Action::Update => "update",
        Action::Delete => "delete",
        Action::Execute => "execute",
        Action::Cancel => "cancel",
        Action::Respond => "respond",
        Action::Manage => "manage",
        Action::Decrypt => "decrypt",
    }
}

fn sensor_log_artifact_parts(artifact_ref: &str) -> Option<(String, &'static str)> {
    let rest = artifact_ref.strip_prefix("sensor.")?;
    if let Some(sensor_ref) = rest.strip_suffix(".stdout") {
        Some((sensor_ref.to_string(), "stdout"))
    } else {
        rest.strip_suffix(".stderr")
            .map(|sensor_ref| (sensor_ref.to_string(), "stderr"))
    }
}

async fn serve_sensor_log_legacy_or_empty(
    artifacts_dir: &str,
    artifact_ref: &str,
    sensor_ref: &str,
    stream: &str,
) -> ApiResult<axum::response::Response> {
    let legacy_path = std::path::Path::new(artifacts_dir)
        .join("sensors")
        .join(sensor_ref)
        .join(format!("{}.log", stream));

    match tokio::fs::read(&legacy_path).await {
        Ok(bytes) => serve_sensor_log_bytes(artifact_ref, bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serve_empty_sensor_log(artifact_ref),
        Err(e) => Err(ApiError::InternalServerError(format!(
            "Failed to read sensor log '{}': {}",
            legacy_path.display(),
            e
        ))),
    }
}

fn serve_empty_sensor_log(artifact_ref: &str) -> ApiResult<axum::response::Response> {
    serve_sensor_log_bytes(artifact_ref, Vec::new())
}

fn serve_sensor_log_bytes(
    artifact_ref: &str,
    bytes: Vec<u8>,
) -> ApiResult<axum::response::Response> {
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "text/plain; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"{}.log\"",
                    artifact_ref.replace('.', "_")
                ),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

/// Serve a file-backed artifact version from disk.
async fn serve_file_from_disk(
    artifacts_dir: &str,
    file_path: &str,
    artifact_ref: &str,
    version: i32,
    content_type: Option<&str>,
) -> ApiResult<axum::response::Response> {
    let full_path = std::path::Path::new(artifacts_dir).join(file_path);
    let wait_deadline = Instant::now() + Duration::from_millis(500);

    while !full_path.exists() {
        if Instant::now() >= wait_deadline {
            return Err(ApiError::NotFound(format!(
                "File for version {} of artifact '{}' not found on disk (expected at '{}')",
                version, artifact_ref, file_path,
            )));
        }

        sleep(Duration::from_millis(25)).await;
    }

    let bytes = tokio::fs::read(&full_path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return ApiError::Forbidden(format!(
                "Permission denied reading artifact file '{}'",
                full_path.display()
            ));
        }
        ApiError::InternalServerError(format!(
            "Failed to read artifact file '{}': {}",
            full_path.display(),
            e,
        ))
    })?;

    let ct = content_type
        .unwrap_or("application/octet-stream")
        .to_string();
    let filename = format!(
        "{}_v{}.{}",
        artifact_ref.replace('.', "_"),
        version,
        extension_from_content_type(&ct),
    );

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, ct),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", filename),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

/// Serve a DB-stored artifact version (BYTEA or JSON content).
fn serve_db_content(
    artifact_ref: &str,
    version: i32,
    ver: &attune_common::models::artifact_version::ArtifactVersion,
) -> ApiResult<axum::response::Response> {
    // For binary content
    if let Some(ref bytes) = ver.content {
        let ct = ver
            .content_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let filename = format!(
            "{}_v{}.{}",
            artifact_ref.replace('.', "_"),
            version,
            extension_from_content_type(&ct),
        );

        return Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, ct),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                ),
            ],
            Body::from(bytes.clone()),
        )
            .into_response());
    }

    // For JSON content, serialize and return
    if let Some(ref json) = ver.content_json {
        let bytes = serde_json::to_vec_pretty(json).map_err(|e| {
            ApiError::InternalServerError(format!("Failed to serialize JSON: {}", e))
        })?;

        let ct = ver
            .content_type
            .clone()
            .unwrap_or_else(|| "application/json".to_string());

        let filename = format!("{}_v{}.json", artifact_ref.replace('.', "_"), version);

        return Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, ct),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                ),
            ],
            Body::from(bytes),
        )
            .into_response());
    }

    Err(ApiError::NotFound(format!(
        "Version {} of artifact '{}' has no downloadable content",
        version, artifact_ref,
    )))
}

/// Delete disk files for a set of file-backed artifact versions.
/// Logs warnings on failure but does not propagate errors.
fn cleanup_version_files(
    artifacts_dir: &str,
    versions: &[attune_common::models::artifact_version::ArtifactVersion],
) {
    for ver in versions {
        if let Some(ref file_path) = ver.file_path {
            let full_path = std::path::Path::new(artifacts_dir).join(file_path);
            if full_path.exists() {
                if let Err(e) = std::fs::remove_file(&full_path) {
                    warn!(
                        "Failed to delete artifact file '{}': {}",
                        full_path.display(),
                        e,
                    );
                }
            }
        }
    }
}

/// Attempt to remove empty parent directories up to (but not including) the
/// artifacts_dir root. This is best-effort cleanup.
fn cleanup_empty_parents(dir: &std::path::Path, stop_at: &str) {
    let stop_path = std::path::Path::new(stop_at);
    let mut current = dir.to_path_buf();
    while current != stop_path && current.starts_with(stop_path) {
        match std::fs::read_dir(&current) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    // Directory is not empty, stop climbing
                    break;
                }
                if let Err(e) = std::fs::remove_dir(&current) {
                    warn!(
                        "Failed to remove empty directory '{}': {}",
                        current.display(),
                        e,
                    );
                    break;
                }
            }
            Err(_) => break,
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }
}
// ============================================================================
// SSE file streaming
// ============================================================================

/// Internal state machine for the `stream_artifact` SSE generator.
///
/// We use `futures::stream::unfold` instead of `async_stream::stream!` to avoid
/// adding an external dependency.
enum TailState {
    /// Waiting for the file to appear on disk.
    WaitingForFile {
        full_path: std::path::PathBuf,
        file_path: String,
        execution_id: Option<i64>,
        db: sqlx::PgPool,
        started: tokio::time::Instant,
    },
    /// File exists — send initial content.
    SendInitial {
        full_path: std::path::PathBuf,
        file_path: String,
        execution_id: Option<i64>,
        db: sqlx::PgPool,
    },
    /// Tailing the file for new bytes.
    Tailing {
        full_path: std::path::PathBuf,
        file_path: String,
        execution_id: Option<i64>,
        db: sqlx::PgPool,
        offset: u64,
        idle_count: u32,
    },
    /// Emit the final `done` SSE event and close.
    SendDone,
    /// Stream has ended — return `None` to close.
    Finished,
}

/// How long to wait for the file to appear on disk.
const STREAM_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
/// How often to poll for new bytes / file existence.
const STREAM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
/// After this many consecutive empty polls we check whether the execution
/// is done and, if so, terminate the stream.
const STREAM_IDLE_CHECKS_BEFORE_DONE: u32 = 6; // 3 seconds of no new data

/// Check whether the given execution has reached a terminal status.
async fn is_execution_terminal(db: &sqlx::PgPool, execution_id: Option<i64>) -> bool {
    let Some(exec_id) = execution_id else {
        return false;
    };
    match sqlx::query_scalar::<_, String>("SELECT status::text FROM execution WHERE id = $1")
        .bind(exec_id)
        .fetch_optional(db)
        .await
    {
        Ok(Some(status)) => matches!(
            status.as_str(),
            "succeeded" | "failed" | "timeout" | "canceled" | "abandoned"
        ),
        Ok(None) => true, // execution deleted — treat as done
        Err(_) => false,  // DB error — keep tailing
    }
}

/// Do one final read from `offset` to EOF and return the new bytes (if any).
async fn final_read_bytes(full_path: &std::path::Path, offset: u64) -> Option<String> {
    let mut f = tokio::fs::File::open(full_path).await.ok()?;
    let meta = f.metadata().await.ok()?;
    if meta.len() <= offset {
        return None;
    }
    f.seek(std::io::SeekFrom::Start(offset)).await.ok()?;
    let mut tail = Vec::new();
    f.read_to_end(&mut tail).await.ok()?;
    if tail.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&tail).into_owned())
}

/// Stream the latest file-backed artifact version as Server-Sent Events.
///
/// The endpoint:
/// 1. Waits (up to ~30 s) for the file to appear on disk if it has been
///    allocated but not yet written by the worker.
/// 2. Once the file exists it sends the current content as an initial `content`
///    event, then tails the file every 500 ms, sending `append` events with new
///    bytes.
/// 3. When no new bytes have appeared for several consecutive checks **and** the
///    linked execution (if any) has reached a terminal status, it sends a `done`
///    event and the stream ends.
/// 4. If the client disconnects the stream is cleaned up automatically.
///
/// **Event types** (SSE `event:` field):
/// - `content`  – full file content up to the current offset (sent once)
/// - `append`   – incremental bytes appended since the last event
/// - `waiting`  – file does not exist yet; sent periodically while waiting
/// - `done`     – no more data expected; stream will close
/// - `error`    – something went wrong; `data` contains a human-readable message
#[utoipa::path(
    get,
    path = "/api/v1/artifacts/{id}/stream",
    tag = "artifacts",
    params(
        ("id" = i64, Path, description = "Artifact ID"),
    ),
    responses(
        (status = 200, description = "SSE stream of file content", content_type = "text/event-stream"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Artifact not found or not file-backed"),
    ),
)]
pub async fn stream_artifact(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError> {
    // --- auth ---------------------------------------------------------------
    use crate::auth::jwt::{extract_token_from_header, validate_token};

    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or(ApiError::Unauthorized(
            "Missing authentication token".to_string(),
        ))?;
    let token = extract_token_from_header(auth_header).ok_or(ApiError::Unauthorized(
        "Invalid authentication token".to_string(),
    ))?;
    let claims = validate_token(token, &state.jwt_config)
        .map_err(|_| ApiError::Unauthorized("Invalid authentication token".to_string()))?;
    let user = AuthenticatedUser { claims };

    // --- resolve artifact + latest version ---------------------------------
    let artifact = ArtifactRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    authorize_artifact_action(&state, &user, Action::Read, &artifact)
        .await
        .map_err(|_| ApiError::NotFound(format!("Artifact with ID {} not found", id)))?;

    if !is_file_backed_type(artifact.r#type) {
        return Err(ApiError::BadRequest(format!(
            "Artifact '{}' is type {:?} which is not file-backed. \
             Use the download endpoint instead.",
            artifact.r#ref, artifact.r#type,
        )));
    }

    let ver = ArtifactVersionRepository::find_latest(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("No versions found for artifact {}", id)))?;

    let file_path = ver.file_path.ok_or_else(|| {
        ApiError::NotFound(format!(
            "Latest version of artifact '{}' has no file_path allocated",
            artifact.r#ref,
        ))
    })?;

    let artifacts_dir = state.config.artifacts_dir.clone();
    let full_path = std::path::PathBuf::from(&artifacts_dir).join(&file_path);
    let execution_id = ver.execution;
    let db = state.db.clone();

    // --- build the SSE stream via unfold -----------------------------------
    let initial_state = TailState::WaitingForFile {
        full_path,
        file_path,
        execution_id,
        db,
        started: tokio::time::Instant::now(),
    };

    let stream = futures::stream::unfold(initial_state, |state| async move {
        match state {
            TailState::Finished => None,

            // ---- Drain state for clean shutdown ----
            TailState::SendDone => Some((
                Ok(Event::default()
                    .event("done")
                    .data("Execution complete — stream closed")),
                TailState::Finished,
            )),

            // ---- Phase 1: wait for the file to appear ----
            TailState::WaitingForFile {
                full_path,
                file_path,
                execution_id,
                db,
                started,
            } => {
                if full_path.exists() {
                    let next = TailState::SendInitial {
                        full_path,
                        file_path,
                        execution_id,
                        db,
                    };
                    Some((
                        Ok(Event::default()
                            .event("waiting")
                            .data("File found — loading content")),
                        next,
                    ))
                } else if started.elapsed() > STREAM_MAX_WAIT {
                    Some((
                        Ok(Event::default().event("error").data(format!(
                            "Timed out waiting for file to appear at '{}'",
                            file_path,
                        ))),
                        TailState::Finished,
                    ))
                } else {
                    tokio::time::sleep(STREAM_POLL_INTERVAL).await;
                    Some((
                        Ok(Event::default()
                            .event("waiting")
                            .data("File not yet available — waiting for worker to create it")),
                        TailState::WaitingForFile {
                            full_path,
                            file_path,
                            execution_id,
                            db,
                            started,
                        },
                    ))
                }
            }

            // ---- Phase 2: read and send current file content ----
            TailState::SendInitial {
                full_path,
                file_path,
                execution_id,
                db,
            } => match tokio::fs::File::open(&full_path).await {
                Ok(mut file) => {
                    let mut buf = Vec::new();
                    match file.read_to_end(&mut buf).await {
                        Ok(_) => {
                            let offset = buf.len() as u64;
                            debug!(
                                "artifact stream: sent initial {} bytes for '{}'",
                                offset, file_path,
                            );
                            Some((
                                Ok(Event::default()
                                    .event("content")
                                    .data(String::from_utf8_lossy(&buf))),
                                TailState::Tailing {
                                    full_path,
                                    file_path,
                                    execution_id,
                                    db,
                                    offset,
                                    idle_count: 0,
                                },
                            ))
                        }
                        Err(e) => Some((
                            Ok(Event::default()
                                .event("error")
                                .data(format!("Failed to read file: {}", e))),
                            TailState::Finished,
                        )),
                    }
                }
                Err(e) => Some((
                    Ok(Event::default()
                        .event("error")
                        .data(format!("Failed to open file: {}", e))),
                    TailState::Finished,
                )),
            },

            // ---- Phase 3: tail the file for new bytes ----
            TailState::Tailing {
                full_path,
                file_path,
                execution_id,
                db,
                mut offset,
                mut idle_count,
            } => {
                tokio::time::sleep(STREAM_POLL_INTERVAL).await;

                // Re-open the file each iteration so we pick up content that
                // was written by a different process (the worker).
                let mut file = match tokio::fs::File::open(&full_path).await {
                    Ok(f) => f,
                    Err(e) => {
                        return Some((
                            Ok(Event::default()
                                .event("error")
                                .data(format!("File disappeared: {}", e))),
                            TailState::Finished,
                        ));
                    }
                };

                let meta = match file.metadata().await {
                    Ok(m) => m,
                    Err(_) => {
                        // Transient metadata error — keep going.
                        return Some((
                            Ok(Event::default().comment("metadata-retry")),
                            TailState::Tailing {
                                full_path,
                                file_path,
                                execution_id,
                                db,
                                offset,
                                idle_count,
                            },
                        ));
                    }
                };

                let file_len = meta.len();

                if file_len > offset {
                    // New data available — seek and read.
                    if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)).await {
                        return Some((
                            Ok(Event::default()
                                .event("error")
                                .data(format!("Seek error: {}", e))),
                            TailState::Finished,
                        ));
                    }
                    let mut new_buf = Vec::with_capacity((file_len - offset) as usize);
                    match file.read_to_end(&mut new_buf).await {
                        Ok(n) => {
                            offset += n as u64;
                            idle_count = 0;
                            Some((
                                Ok(Event::default()
                                    .event("append")
                                    .data(String::from_utf8_lossy(&new_buf))),
                                TailState::Tailing {
                                    full_path,
                                    file_path,
                                    execution_id,
                                    db,
                                    offset,
                                    idle_count,
                                },
                            ))
                        }
                        Err(e) => Some((
                            Ok(Event::default()
                                .event("error")
                                .data(format!("Read error: {}", e))),
                            TailState::Finished,
                        )),
                    }
                } else if file_len < offset {
                    // File truncated — resend from scratch.
                    drop(file);
                    Some((
                        Ok(Event::default()
                            .event("waiting")
                            .data("File was truncated — resending content")),
                        TailState::SendInitial {
                            full_path,
                            file_path,
                            execution_id,
                            db,
                        },
                    ))
                } else {
                    // No change.
                    idle_count += 1;

                    if idle_count >= STREAM_IDLE_CHECKS_BEFORE_DONE {
                        let done = is_execution_terminal(&db, execution_id).await
                            || (execution_id.is_none()
                                && idle_count >= STREAM_IDLE_CHECKS_BEFORE_DONE * 4);

                        if done {
                            // One final read to catch trailing bytes.
                            return if let Some(trailing) =
                                final_read_bytes(&full_path, offset).await
                            {
                                Some((
                                    Ok(Event::default().event("append").data(trailing)),
                                    TailState::SendDone,
                                ))
                            } else {
                                Some((
                                    Ok(Event::default()
                                        .event("done")
                                        .data("Execution complete — stream closed")),
                                    TailState::Finished,
                                ))
                            };
                        }

                        // Reset so we don't hit the DB every poll.
                        idle_count = 0;
                    }

                    Some((
                        Ok(Event::default().comment("no-change")),
                        TailState::Tailing {
                            full_path,
                            file_path,
                            execution_id,
                            db,
                            offset,
                            idle_count,
                        },
                    ))
                }
            }
        }
    });

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keepalive"),
    ))
}

fn extension_from_content_type(ct: &str) -> &str {
    match ct {
        "text/plain" => "txt",
        "text/html" => "html",
        "text/css" => "css",
        "text/csv" => "csv",
        "text/xml" => "xml",
        "application/json" => "json",
        "application/xml" => "xml",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/gzip" => "gz",
        "application/octet-stream" => "bin",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        _ => "bin",
    }
}

// ============================================================================
// Router
// ============================================================================

/// Register artifact routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Artifact CRUD
        .route("/artifacts", get(list_artifacts).post(create_artifact))
        .route(
            "/artifacts/{id}",
            get(get_artifact)
                .put(update_artifact)
                .delete(delete_artifact),
        )
        .route("/artifacts/ref/{ref}", get(get_artifact_by_ref))
        .route(
            "/artifacts/ref/{ref}/versions/upload",
            post(upload_version_by_ref),
        )
        .route(
            "/artifacts/ref/{ref}/versions/file",
            post(allocate_file_version_by_ref),
        )
        // Progress / data
        .route("/artifacts/{id}/progress", post(append_progress))
        .route(
            "/artifacts/{id}/data",
            axum::routing::put(set_artifact_data),
        )
        // Download (latest)
        .route("/artifacts/{id}/download", get(download_latest))
        // SSE streaming for file-backed artifacts
        .route("/artifacts/{id}/stream", get(stream_artifact))
        // Version management
        .route(
            "/artifacts/{id}/versions",
            get(list_versions).post(create_version_json),
        )
        .route("/artifacts/{id}/versions/latest", get(get_latest_version))
        .route("/artifacts/{id}/versions/upload", post(upload_version))
        .route("/artifacts/{id}/versions/file", post(create_version_file))
        .route(
            "/artifacts/{id}/versions/{version}",
            get(get_version).delete(delete_version),
        )
        .route(
            "/artifacts/{id}/versions/{version}/download",
            get(download_version),
        )
        // By execution
        .route(
            "/executions/{execution_id}/artifacts",
            get(list_artifacts_by_execution),
        )
}

fn emit_artifact_audit(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    event_type: &'static str,
    outcome: AuditOutcome,
    artifact: &attune_common::models::artifact::Artifact,
    details: serde_json::Value,
) {
    let mut builder = AuditEventBuilder::new(AuditCategory::Secret, event_type, outcome)
        .resource("artifact")
        .resource_id(artifact.id)
        .resource_ref(artifact.r#ref.clone())
        .with_details(details);

    if let Ok(identity_id) = user.identity_id() {
        builder = builder.actor_identity(identity_id);
    }
    builder = builder
        .actor_login(user.login().to_string())
        .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase());

    state.audit_emitter.emit(builder.build());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_routes_structure() {
        let _router = routes();
    }

    #[test]
    fn test_extension_from_content_type() {
        assert_eq!(extension_from_content_type("text/plain"), "txt");
        assert_eq!(extension_from_content_type("application/json"), "json");
        assert_eq!(extension_from_content_type("image/png"), "png");
        assert_eq!(extension_from_content_type("unknown/type"), "bin");
    }

    #[test]
    fn test_default_visibility_for_runtime_logs_is_private() {
        assert_eq!(
            default_visibility_for_artifact(
                ArtifactType::FileText,
                ArtifactClassification::RuntimeLog,
            ),
            ArtifactVisibility::Private
        );
        assert_eq!(
            default_visibility_for_artifact(
                ArtifactType::Progress,
                ArtifactClassification::General
            ),
            ArtifactVisibility::Public
        );
    }

    #[test]
    fn test_runtime_log_public_visibility_is_rejected() {
        let err = validate_runtime_log_visibility(
            ArtifactClassification::RuntimeLog,
            ArtifactVisibility::Public,
        )
        .expect_err("runtime log visibility should be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)));

        validate_runtime_log_visibility(
            ArtifactClassification::RuntimeLog,
            ArtifactVisibility::Private,
        )
        .expect("private runtime log visibility should be accepted");
    }
}
