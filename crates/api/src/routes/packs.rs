//! Pack management API routes

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, LazyLock};
use validator::Validate;

// Documentation-only shape for the manually parsed multipart endpoint.
#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
struct PackUploadForm {
    #[schema(format = Binary)]
    pack: String,
    force: Option<String>,
    skip_tests: Option<String>,
}

use attune_common::audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome};
use attune_common::models::{pack_test::PackTestResult, Pack, PackInstall, PackInstallStatus};
use attune_common::mq::{
    MessageEnvelope, MessageType, PackChangedPayload, PackDeletedPayload, PackRegisteredPayload,
    PackTestRequestedPayload,
};
use attune_common::rbac::{
    Action, AuthorizationContext, ExecutionScopeConstraint, Grant, GrantConstraints, Resource,
};
use attune_common::repositories::{
    cache::CacheNamespaceRepository,
    pack::{
        CreatePackInput, PackSearchFilters, PackVisibilityFilter, PackVisibilityScope,
        UpdatePackInput,
    },
    pack_registry_index::{CreatePackRegistryIndexInput, UpdatePackRegistryIndexInput},
    work_queue::WorkQueueRepository,
    ActionRepository, Create, Delete, FindById, FindByRef, List, PackInstallRepository,
    PackRegistryIndexRepository, PackRepository, PackTestRepository, Patch, RuleRepository,
    SensorRepository, TriggerRepository, Update,
};
use attune_common::workflow::{PackWorkflowService, PackWorkflowServiceConfig};

use crate::{
    auth::middleware::RequireAuth,
    authz::{AuthorizationCheck, AuthorizationService},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        pack::{
            BrowsePackIndexQuery, BuildPackEnvsRequest, BuildPackEnvsResponse,
            CreatePackRegistryIndexRequest, CreatePackRequest, DownloadPacksRequest,
            DownloadPacksResponse, GetPackDependenciesRequest, GetPackDependenciesResponse,
            IndexedPackResponse, InstallPackRequest, PackDescriptionPatch, PackInstallProvenance,
            PackInstallResponse, PackInstallStatusResponse, PackListParams,
            PackRegistryIndexResponse, PackRegistryIndexSummary, PackResponse, PackSummary,
            PackWorkflowSyncResponse, PackWorkflowValidationResponse, RegisterPackRequest,
            RegisterPacksRequest, RegisterPacksResponse, UpdatePackRegistryIndexRequest,
            UpdatePackRequest, WorkflowSyncResult,
        },
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

const PACK_UPLOAD_MAX_BYTES: usize = 100 * 1024 * 1024; // 100 MB
const PACK_INDEX_CREATED_EVENT: &str = "pack.registry_index.created";
const PACK_INDEX_UPDATED_EVENT: &str = "pack.registry_index.updated";
const PACK_INDEX_DELETED_EVENT: &str = "pack.registry_index.deleted";
static PACK_INSTALL_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

struct PackInstallationMetadata {
    source_type: String,
    source_url: Option<String>,
    source_ref: Option<String>,
    checksum: Option<String>,
    checksum_verified: bool,
    installed_by: Option<i64>,
    storage_path: String,
    provenance: PackInstallProvenance,
}

struct RegisteredPack {
    id: i64,
    test_install: Option<PackInstall>,
}

struct TemporaryInstallCleanup {
    root: Option<PathBuf>,
}

impl TemporaryInstallCleanup {
    fn new(temp_base_dir: &FsPath, pack_path: &FsPath) -> Self {
        let managed_root = temp_base_dir.join("pack-installs");
        let root = pack_path
            .strip_prefix(&managed_root)
            .ok()
            .and_then(|relative| relative.components().next())
            .map(|component| managed_root.join(component.as_os_str()));
        Self { root }
    }

    fn disarm(&mut self) {
        self.root = None;
    }
}

impl Drop for TemporaryInstallCleanup {
    fn drop(&mut self) {
        if let Some(root) = self.root.take() {
            if let Err(error) = std::fs::remove_dir_all(&root) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        "Failed to clean up temporary pack install {}: {}",
                        root.display(),
                        error
                    );
                }
            }
        }
    }
}

/// List all packs with pagination
#[utoipa::path(
    get,
    path = "/api/v1/packs",
    tag = "packs",
    params(PackListParams),
    responses(
        (status = 200, description = "List of packs", body = PaginatedResponse<PackSummary>),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_packs(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<PackListParams>,
) -> ApiResult<impl IntoResponse> {
    let pagination = query.pagination();
    let mut filters = PackSearchFilters {
        limit: pagination.limit() as i64,
        offset: pagination.offset() as i64,
        query: query.q,
        ..Default::default()
    };

    if matches!(
        user.claims.token_type,
        crate::auth::jwt::TokenType::Access | crate::auth::jwt::TokenType::Execution
    ) {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = state.authorization_service();
        let grants = authz.effective_grants(&user).await?;
        filters.visibility = Some(build_pack_visibility_filter(identity_id, &grants));
    }

    let result = PackRepository::list_search(&state.db, &filters).await?;
    let summaries: Vec<PackSummary> = result.rows.into_iter().map(PackSummary::from).collect();
    let response = PaginatedResponse::new(summaries, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// Get a single pack by reference
#[utoipa::path(
    get,
    path = "/api/v1/packs/{ref}",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Pack reference identifier")
    ),
    responses(
        (status = 200, description = "Pack details", body = inline(ApiResponse<PackResponse>)),
        (status = 404, description = "Pack not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    if matches!(
        user.claims.token_type,
        crate::auth::jwt::TokenType::Access | crate::auth::jwt::TokenType::Execution
    ) {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = state.authorization_service();
        let grants = authz.effective_grants(&user).await?;
        if !pack_action_allowed(&grants, Action::Read, identity_id, &pack) {
            return Err(ApiError::NotFound(format!("Pack '{}' not found", pack_ref)));
        }
    }

    let mut response = PackResponse::from(pack);
    response.action_count =
        Some(ActionRepository::count_by_pack_ref(&state.db, &response.r#ref).await?);
    response.trigger_count =
        Some(TriggerRepository::count_by_pack_ref(&state.db, &response.r#ref).await?);
    response.rule_count =
        Some(RuleRepository::count_by_pack_ref(&state.db, &response.r#ref).await?);
    response.sensor_count =
        Some(SensorRepository::count_by_pack_ref(&state.db, &response.r#ref).await?);

    let response = ApiResponse::new(response);

    Ok((StatusCode::OK, Json(response)))
}

/// Serve the optional icon bundled at a pack root as `pack-icon.{jpg,png,ico,svg}`.
#[utoipa::path(
    get,
    path = "/api/v1/packs/{ref}/icon",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Pack reference identifier")
    ),
    responses(
        (status = 200, description = "Pack icon image"),
        (status = 404, description = "Pack icon not found"),
    )
)]
pub async fn get_pack_icon(
    State(state): State<Arc<AppState>>,
    Path(pack_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    if !is_valid_pack_ref_path_segment(&pack_ref) {
        return Err(ApiError::NotFound(format!(
            "Icon for pack '{}' not found",
            pack_ref
        )));
    }

    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);
    let Some((icon_path, content_type)) = find_pack_icon(&packs_base_dir, &pack_ref).await else {
        return Err(ApiError::NotFound(format!(
            "Icon for pack '{}' not found",
            pack_ref
        )));
    };

    let bytes = tokio::fs::read(&icon_path).await.map_err(|err| {
        tracing::warn!(
            pack_ref = %pack_ref,
            path = %icon_path.display(),
            error = %err,
            "failed to read pack icon"
        );
        ApiError::NotFound(format!("Icon for pack '{}' not found", pack_ref))
    })?;

    let mut response = Body::from(bytes).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );

    Ok(response)
}

async fn publish_pack_metadata_change(
    state: &Arc<AppState>,
    pack: &Pack,
    operation: &str,
    updated_at: chrono::DateTime<chrono::Utc>,
) {
    let Some(publisher) = state.get_publisher().await else {
        return;
    };

    let payload = PackChangedPayload {
        pack_id: pack.id,
        pack_ref: pack.r#ref.clone(),
        operation: operation.to_string(),
        updated_at,
    };
    let envelope =
        MessageEnvelope::new(MessageType::PackChanged, payload).with_source("api-service");
    if let Err(error) = publisher.publish_envelope(&envelope).await {
        tracing::warn!(
            "Failed to publish PackChanged metadata invalidation for pack '{}': {}",
            pack.r#ref,
            error
        );
    }
}

/// Create a new pack
#[utoipa::path(
    post,
    path = "/api/v1/packs",
    tag = "packs",
    request_body = CreatePackRequest,
    responses(
        (status = 201, description = "Pack created successfully", body = inline(ApiResponse<PackResponse>)),
        (status = 400, description = "Validation error"),
        (status = 409, description = "Pack with same ref already exists")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreatePackRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Check if pack with same ref already exists
    if PackRepository::exists_by_ref(&state.db, &request.r#ref).await? {
        return Err(ApiError::Conflict(format!(
            "Pack with ref '{}' already exists",
            request.r#ref
        )));
    }

    let mut creator_identity = None;
    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        creator_identity = Some(identity_id);
        let authz = state.authorization_service();
        let mut ctx = AuthorizationContext::new(identity_id);
        ctx.target_ref = Some(request.r#ref.clone());
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Packs,
                    action: Action::Create,
                    context: ctx,
                },
            )
            .await?;
    }

    // Create pack input
    let pack_input = CreatePackInput {
        r#ref: request.r#ref,
        label: request.label,
        description: request.description,
        version: request.version,
        conf_schema: request.conf_schema,
        config: request.config,
        meta: request.meta,
        tags: request.tags,
        runtime_deps: request.runtime_deps,
        dependencies: request.dependencies,
        is_standard: request.is_standard,
        installers: serde_json::json!({}),
    };

    let mut pack = PackRepository::create(&state.db, pack_input).await?;
    pack = PackRepository::update_worker_placement(
        &state.db,
        pack.id,
        &request.worker_selector,
        &request.worker_tolerations,
        &request.worker_affinity,
    )
    .await?;
    if let Some(identity_id) = creator_identity {
        if !pack.is_standard {
            pack = PackRepository::set_installed_by(&state.db, pack.id, identity_id).await?;
        }
    }

    // Auto-sync workflows after pack creation
    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);

    let service_config = PackWorkflowServiceConfig {
        packs_base_dir,
        skip_validation_errors: true, // Don't fail pack creation on workflow errors
        update_existing: true,
        max_file_size: 1024 * 1024,
    };

    let workflow_service = PackWorkflowService::new(state.db.clone(), service_config);

    // Attempt to sync workflows but don't fail if it errors
    match workflow_service.sync_pack_workflows(&pack.r#ref).await {
        Ok(sync_result) => {
            if sync_result.registered_count > 0 {
                tracing::info!(
                    "Auto-synced {} workflows for pack '{}'",
                    sync_result.registered_count,
                    pack.r#ref
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                "Failed to auto-sync workflows for pack '{}': {}",
                pack.r#ref,
                e
            );
        }
    }

    publish_pack_metadata_change(&state, &pack, "created", pack.updated).await;

    emit_pack_audit(
        &state,
        &user,
        event_type::pack::CREATED,
        &pack,
        serde_json::json!({
            "version": pack.version.as_str(),
            "is_standard": pack.is_standard,
            "installed_by": pack.installed_by,
        }),
    );

    let response = ApiResponse::with_message(PackResponse::from(pack), "Pack created successfully");

    Ok((StatusCode::CREATED, Json(response)))
}

/// Update an existing pack
#[utoipa::path(
    put,
    path = "/api/v1/packs/{ref}",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Pack reference identifier")
    ),
    request_body = UpdatePackRequest,
    responses(
        (status = 200, description = "Pack updated successfully", body = inline(ApiResponse<PackResponse>)),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Pack not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
    Json(request): Json<UpdatePackRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Check if pack exists
    let existing_pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = state.authorization_service();
        let grants = authz.effective_grants(&user).await?;
        if !pack_action_allowed(&grants, Action::Configure, identity_id, &existing_pack) {
            return Err(ApiError::Forbidden(
                "Not authorized to configure pack".to_string(),
            ));
        }
        if existing_pack.installed_by == Some(identity_id) || existing_pack.installed_by.is_none() {
            authz
                .authorize(
                    &user,
                    AuthorizationCheck {
                        resource: Resource::Packs,
                        action: Action::Configure,
                        context: pack_authorization_context(identity_id, &existing_pack),
                    },
                )
                .await?;
        }
    }

    // Create update input
    let update_input = UpdatePackInput {
        label: request.label,
        description: request.description.map(|patch| match patch {
            PackDescriptionPatch::Set(value) => Patch::Set(value),
            PackDescriptionPatch::Clear => Patch::Clear,
        }),
        version: request.version,
        conf_schema: request.conf_schema,
        config: request.config,
        meta: request.meta,
        tags: request.tags,
        runtime_deps: request.runtime_deps,
        dependencies: request.dependencies,
        is_standard: request.is_standard,
        installers: None,
    };

    let pack = PackRepository::update(&state.db, existing_pack.id, update_input).await?;
    let pack = if request.worker_selector.is_some()
        || request.worker_tolerations.is_some()
        || request.worker_affinity.is_some()
    {
        PackRepository::update_worker_placement(
            &state.db,
            pack.id,
            request
                .worker_selector
                .as_ref()
                .unwrap_or(&pack.worker_selector),
            request
                .worker_tolerations
                .as_ref()
                .unwrap_or(&pack.worker_tolerations),
            request
                .worker_affinity
                .as_ref()
                .unwrap_or(&pack.worker_affinity),
        )
        .await?
    } else {
        pack
    };

    // Auto-sync workflows after pack update
    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);

    let service_config = PackWorkflowServiceConfig {
        packs_base_dir,
        skip_validation_errors: true, // Don't fail pack update on workflow errors
        update_existing: true,
        max_file_size: 1024 * 1024,
    };

    let workflow_service = PackWorkflowService::new(state.db.clone(), service_config);

    // Attempt to sync workflows but don't fail if it errors
    match workflow_service.sync_pack_workflows(&pack.r#ref).await {
        Ok(sync_result) => {
            if sync_result.registered_count > 0 {
                tracing::info!(
                    "Auto-synced {} workflows for pack '{}'",
                    sync_result.registered_count,
                    pack.r#ref
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                "Failed to auto-sync workflows for pack '{}': {}",
                pack.r#ref,
                e
            );
        }
    }

    publish_pack_metadata_change(&state, &pack, "updated", pack.updated).await;

    emit_pack_audit(
        &state,
        &user,
        event_type::pack::UPDATED,
        &pack,
        serde_json::json!({
            "version": pack.version.as_str(),
            "is_standard": pack.is_standard,
            "installed_by": pack.installed_by,
        }),
    );

    let response = ApiResponse::with_message(PackResponse::from(pack), "Pack updated successfully");

    Ok((StatusCode::OK, Json(response)))
}

async fn delete_pack_database_records_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack_id: i64,
) -> attune_common::Result<(bool, u64)> {
    let tombstoned_caches =
        CacheNamespaceRepository::tombstone_for_pack_deletion(&mut *tx, pack_id).await?;
    WorkQueueRepository::delete_non_adhoc_by_pack_excluding(&mut **tx, pack_id, &[]).await?;
    let deleted = PackRepository::delete(&mut **tx, pack_id).await?;
    if !deleted {
        return Ok((false, 0));
    }
    Ok((deleted, tombstoned_caches))
}

async fn delete_failed_pack_registration(
    state: &AppState,
    pack_id: i64,
    pack_ref: &str,
    remove_storage: bool,
) -> attune_common::Result<(bool, u64)> {
    // The caller retains the pack's advisory-lock transaction throughout this cleanup.
    let mut tx = state.db.begin().await?;
    let removal = if remove_storage {
        let storage = attune_common::pack_registry::PackStorage::new(&state.config.packs_base_dir);
        Some(storage.stage_uninstall(pack_ref, None)?)
    } else {
        None
    };
    let result = delete_pack_database_records_in_transaction(&mut tx, pack_id).await?;
    if let Some(removal) = removal {
        removal.commit()?;
    }
    tx.commit().await?;
    Ok(result)
}

fn validated_pack_removal_ref<'a>(
    requested_ref: &str,
    persisted_ref: &'a str,
) -> ApiResult<&'a str> {
    attune_common::schema::RefValidator::validate_pack_ref(requested_ref)
        .map_err(|error| ApiError::BadRequest(format!("Invalid pack ref: {error}")))?;
    attune_common::schema::RefValidator::validate_pack_ref(persisted_ref).map_err(|error| {
        ApiError::InternalServerError(format!("Persisted pack ref is invalid: {error}"))
    })?;
    if requested_ref != persisted_ref {
        return Err(ApiError::InternalServerError(
            "Requested and persisted pack refs do not match".to_string(),
        ));
    }
    Ok(persisted_ref)
}

/// Delete a pack
#[utoipa::path(
    delete,
    path = "/api/v1/packs/{ref}",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Pack reference identifier")
    ),
    responses(
        (status = 200, description = "Pack deleted successfully", body = SuccessResponse),
        (status = 404, description = "Pack not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    attune_common::schema::RefValidator::validate_pack_ref(&pack_ref)
        .map_err(|error| ApiError::BadRequest(format!("Invalid pack ref: {error}")))?;
    // Check if pack exists
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;
    let removal_ref = validated_pack_removal_ref(&pack_ref, &pack.r#ref)?;

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = state.authorization_service();
        let grants = authz.effective_grants(&user).await?;
        if !pack_action_allowed(&grants, Action::Delete, identity_id, &pack) {
            return Err(ApiError::Forbidden(
                "Not authorized to delete pack".to_string(),
            ));
        }
        if pack.installed_by == Some(identity_id) || pack.installed_by.is_none() {
            authz
                .authorize(
                    &user,
                    AuthorizationCheck {
                        resource: Resource::Packs,
                        action: Action::Delete,
                        context: pack_authorization_context(identity_id, &pack),
                    },
                )
                .await?;
        }
    }

    let _install_guard = PACK_INSTALL_LOCK.lock().await;
    let mut tx = state.db.begin().await?;
    PackRepository::acquire_mutation_lock(&mut tx, removal_ref).await?;
    let locked_pack = PackRepository::find_by_ref(&mut *tx, removal_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;
    if locked_pack.id != pack.id {
        return Err(ApiError::Conflict(format!(
            "Pack '{}' changed while deletion was being authorized",
            pack_ref
        )));
    }

    // Stage storage removal first; dropping the guard restores it on any error.
    let storage = attune_common::pack_registry::PackStorage::new(&state.config.packs_base_dir);
    let removal = storage
        .stage_uninstall(removal_ref, None)
        .map_err(|error| {
            ApiError::InternalServerError(format!("Failed to stage pack removal: {error}"))
        })?;

    // Cache namespaces become unreadable before the pack delete in the same
    // transaction. Typed owner/manager FKs are then cleared while text refs
    // and cache data remain for asynchronous supervisor cleanup.
    let (deleted, tombstoned_caches) =
        delete_pack_database_records_in_transaction(&mut tx, pack.id).await?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Pack '{}' not found", pack_ref)));
    }
    if tombstoned_caches > 0 {
        tracing::info!(
            "Tombstoned {} cache namespace(s) before deleting pack '{}'",
            tombstoned_caches,
            pack_ref
        );
    }

    removal.commit().map_err(|error| {
        ApiError::InternalServerError(format!("Failed to finalize pack removal: {error}"))
    })?;
    tx.commit().await?;
    let storage_removed = true;

    // Publish pack.deleted event so workers and sensors can clean up
    // local pack files and runtime environments.
    if let Some(publisher) = state.get_publisher().await {
        let payload = PackDeletedPayload {
            pack_id: pack.id,
            pack_ref: pack_ref.clone(),
        };
        let envelope = MessageEnvelope::new(MessageType::PackDeleted, payload);
        match publisher.publish_envelope(&envelope).await {
            Ok(()) => {
                tracing::info!("Published pack.deleted event for pack '{}'", pack_ref);
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to publish pack.deleted event for pack '{}': {}",
                    pack_ref,
                    e,
                );
            }
        }
    }
    publish_pack_metadata_change(&state, &pack, "deleted", chrono::Utc::now()).await;

    emit_pack_audit(
        &state,
        &user,
        event_type::pack::DELETED,
        &pack,
        serde_json::json!({
            "version": pack.version.as_str(),
            "is_standard": pack.is_standard,
            "installed_by": pack.installed_by,
            "storage_removed": storage_removed,
        }),
    );

    let response = SuccessResponse::new(format!("Pack '{}' deleted successfully", pack_ref));

    Ok((StatusCode::OK, Json(response)))
}

/// Helper function to dispatch pack tests to a worker and record the install.
///
/// Returns `None` when the pack has no enabled test configuration. Otherwise
/// returns `Ok(PackInstall)` with the freshly-created tracking record in the
/// `pending` state, or `Err` if the tests could not be dispatched.
async fn dispatch_and_track_pack_tests(
    state: &AppState,
    pack_id: Option<i64>,
    pack_ref: &str,
    pack_version: &str,
    trigger_type: &str,
    pack_dir: &std::path::Path,
    candidate_path: Option<String>,
    worker_selector: serde_json::Value,
    worker_tolerations: serde_json::Value,
    worker_affinity: serde_json::Value,
) -> Option<Result<attune_common::models::pack_install::PackInstall, ApiError>> {
    use attune_common::test_executor::TestConfig;

    // Load pack.yaml from filesystem
    if !pack_dir.exists() {
        return Some(Err(ApiError::NotFound(format!(
            "Pack directory not found: {}",
            pack_dir.display()
        ))));
    }

    let pack_yaml_path = pack_dir.join("pack.yaml");
    if !pack_yaml_path.exists() {
        return Some(Err(ApiError::NotFound(format!(
            "pack.yaml not found for pack '{}'",
            pack_ref
        ))));
    }

    // Parse pack.yaml
    let pack_yaml_content = match tokio::fs::read_to_string(&pack_yaml_path).await {
        Ok(content) => content,
        Err(e) => {
            return Some(Err(ApiError::InternalServerError(format!(
                "Failed to read pack.yaml: {}",
                e
            ))))
        }
    };

    let pack_yaml: serde_yaml_ng::Value = match serde_yaml_ng::from_str(&pack_yaml_content) {
        Ok(v) => v,
        Err(e) => {
            return Some(Err(ApiError::InternalServerError(format!(
                "Failed to parse pack.yaml: {}",
                e
            ))))
        }
    };

    // Extract test configuration - if absent or disabled, skip tests gracefully
    let testing_config = match pack_yaml.get("testing") {
        Some(config) => config,
        None => {
            tracing::info!(
                "No testing configuration found in pack.yaml for pack '{}', skipping tests",
                pack_ref
            );
            return None;
        }
    };

    let test_config: TestConfig = match serde_yaml_ng::from_value(testing_config.clone()) {
        Ok(config) => config,
        Err(e) => {
            return Some(Err(ApiError::InternalServerError(format!(
                "Failed to parse test configuration: {}",
                e
            ))))
        }
    };

    if !test_config.enabled {
        tracing::info!(
            "Testing is disabled for pack '{}', skipping tests",
            pack_ref
        );
        return None;
    }

    let required_runtimes = required_runtimes_for_test_config(&test_config);
    let trigger_reason = trigger_type.to_string();

    // Create the install tracking record (survives a rollback of a new pack).
    let install = match PackInstallRepository::new(state.db.clone())
        .create(pack_ref, pack_version, &trigger_reason, pack_id)
        .await
    {
        Ok(record) => record,
        Err(e) => {
            return Some(Err(ApiError::DatabaseError(format!(
                "Failed to record pack install: {}",
                e
            ))))
        }
    };

    let candidate_path = if let Some(candidate_path) = candidate_path {
        let storage = attune_common::pack_registry::PackStorage::new(&state.config.packs_base_dir);
        match storage.bind_candidate_to_install(
            std::path::Path::new(&candidate_path),
            pack_ref,
            install.id,
        ) {
            Ok(path) => Some(path.to_string_lossy().to_string()),
            Err(error) => {
                let message = format!("Failed to prepare pack test candidate: {error}");
                if let Err(update_error) = PackInstallRepository::new(state.db.clone())
                    .update_status(install.id, PackInstallStatus::Failed, Some(message.clone()))
                    .await
                {
                    tracing::warn!(
                        "Failed to mark pack install {} failed: {}",
                        install.id,
                        update_error
                    );
                }
                return Some(Err(ApiError::InternalServerError(message)));
            }
        }
    } else {
        None
    };
    let candidate_dir = candidate_path.as_ref().map(std::path::PathBuf::from);

    // Publish the test request; the executor selects a capable worker.
    let Some(publisher) = state.get_publisher().await else {
        // No message queue available — record the failure so the finalizer can
        // roll the pack back for new installs.
        let message =
            "Message queue publisher unavailable; could not dispatch pack tests".to_string();
        if let Err(e) = PackInstallRepository::new(state.db.clone())
            .update_status(install.id, PackInstallStatus::Failed, Some(message.clone()))
            .await
        {
            tracing::warn!("Failed to mark pack install {} failed: {}", install.id, e);
        }
        if let Some(candidate_dir) = &candidate_dir {
            let _ = std::fs::remove_dir_all(candidate_dir);
        }
        return Some(Err(ApiError::InternalServerError(message)));
    };

    let payload = PackTestRequestedPayload {
        pack_install_id: install.id,
        pack_ref: pack_ref.to_string(),
        pack_version: pack_version.to_string(),
        candidate_path,
        trigger_reason,
        required_runtimes,
        worker_selector,
        worker_tolerations,
        worker_affinity,
    };
    let envelope = MessageEnvelope::new(MessageType::PackTestRequested, payload);

    if let Err(e) = publisher.publish_envelope(&envelope).await {
        tracing::warn!(
            "Failed to publish pack test request for pack '{}': {}",
            pack_ref,
            e
        );
        let message = format!("Failed to dispatch pack tests: {}", e);
        if let Err(e) = PackInstallRepository::new(state.db.clone())
            .update_status(install.id, PackInstallStatus::Failed, Some(message.clone()))
            .await
        {
            tracing::warn!("Failed to mark pack install {} failed: {}", install.id, e);
        }
        if let Some(candidate_dir) = &candidate_dir {
            let _ = std::fs::remove_dir_all(candidate_dir);
        }
        return Some(Err(ApiError::InternalServerError(message)));
    }

    Some(Ok(install))
}

/// Derive the runtime names a worker must provide to execute a pack's tests.
///
/// `unittest`/`pytest` runners require a Python interpreter; `script` runners
/// rely on a shell which every worker provides, so no requirement is added.
fn required_runtimes_for_test_config(
    test_config: &attune_common::test_executor::TestConfig,
) -> Vec<String> {
    use std::collections::BTreeSet;

    let mut required: BTreeSet<String> = BTreeSet::new();
    for runner in test_config.runners.values() {
        match runner.r#type.as_str() {
            "unittest" | "pytest" => {
                required.insert("python".to_string());
            }
            _ => {}
        }
    }
    required.into_iter().collect()
}

async fn wait_for_pack_test(state: &AppState, install_id: i64) -> Result<PackInstall, ApiError> {
    use std::time::Duration;

    let repo = PackInstallRepository::new(state.db.clone());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        if let Some(record) = repo.find_by_id(install_id).await? {
            if attune_common::repositories::pack_install_is_terminal(&record.status) {
                return Ok(record);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(repo
                .finish(
                    install_id,
                    PackInstallStatus::Failed,
                    None,
                    None,
                    Some("Timed out waiting for worker to complete pack tests".to_string()),
                )
                .await?);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Background task that watches a pack install record to completion and then
/// marks the pack installed, or rolls a brand-new pack back on failure.
async fn finalize_pack_install(
    state: Arc<AppState>,
    install_id: i64,
    pack_ref: String,
    pack_id: i64,
    is_new_pack: bool,
    force: bool,
    manages_storage: bool,
    touch_pack_status: bool,
) {
    use std::time::Duration;

    let install_repo = PackInstallRepository::new(state.db.clone());
    let max_wait = Duration::from_secs(600);
    let poll_interval = Duration::from_secs(2);
    let deadline = tokio::time::Instant::now() + max_wait;

    let terminal = loop {
        match install_repo.find_by_id(install_id).await {
            Ok(Some(record)) => {
                if attune_common::repositories::pack_install_is_terminal(&record.status) {
                    break Some(record);
                }
            }
            Ok(None) => break None,
            Err(e) => tracing::warn!(
                "Failed to poll pack install {} for pack '{}': {}",
                install_id,
                pack_ref,
                e
            ),
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                "Timed out waiting for pack install {} (pack '{}'); treating as failed",
                install_id,
                pack_ref
            );
            let _ = install_repo
                .finish(
                    install_id,
                    PackInstallStatus::Failed,
                    None,
                    None,
                    Some("Timed out waiting for worker to complete pack tests".to_string()),
                )
                .await;
            break install_repo.find_by_id(install_id).await.ok().flatten();
        }
        tokio::time::sleep(poll_interval).await;
    };

    match terminal {
        Some(record) if record.status == "succeeded" => {
            if touch_pack_status {
                if let Err(e) = mark_pack_install_status(&state.db, &pack_ref, "installed").await {
                    tracing::warn!("Failed to mark pack '{}' as installed: {}", pack_ref, e);
                }
            }
            tracing::info!("Pack install {} for '{}' succeeded", install_id, pack_ref);
        }
        Some(record) => {
            // Failure (or timeout treated as failure). Stamp finished_at and
            // preserve any worker-provided error detail.
            if record.finished_at.is_none() {
                let _ = install_repo
                    .finish(
                        install_id,
                        PackInstallStatus::Failed,
                        None,
                        None,
                        record.error_message.clone(),
                    )
                    .await;
            }
            if is_new_pack && !force {
                tracing::warn!(
                    "Pack install {} for new pack '{}' failed; rolling back",
                    install_id,
                    pack_ref
                );
                match delete_failed_pack_registration(&state, pack_id, &pack_ref, manages_storage)
                    .await
                {
                    Ok((true, _)) => {}
                    Ok((false, _)) => tracing::error!(
                        "Failed to roll back new pack '{}' after test failure: pack row disappeared",
                        pack_ref
                    ),
                    Err(e) => tracing::error!(
                        "Failed to roll back new pack '{}' after test failure: {}",
                        pack_ref,
                        e
                    ),
                }
                let _ = install_repo
                    .update_status(install_id, PackInstallStatus::RolledBack, None)
                    .await;
            } else if touch_pack_status {
                if let Err(e) = mark_pack_install_status(&state.db, &pack_ref, "installed").await {
                    tracing::warn!("Failed to mark pack '{}' as installed: {}", pack_ref, e);
                }
            }
            tracing::warn!(
                "Pack install {} for '{}' ended in state {}",
                install_id,
                pack_ref,
                record.status
            );
        }
        None => {
            tracing::error!(
                "Pack install {} for '{}' disappeared before finalization",
                install_id,
                pack_ref
            );
        }
    }
}

/// Update a pack's `install_status` column.
async fn mark_pack_install_status(
    db: &sqlx::PgPool,
    pack_ref: &str,
    status: &str,
) -> attune_common::Result<()> {
    sqlx::query("UPDATE pack SET install_status = $1 WHERE ref = $2")
        .bind(status)
        .bind(pack_ref)
        .execute(db)
        .await?;
    Ok(())
}

/// Upload and register a pack from a tar.gz archive (multipart/form-data)
///
/// The archive should be a gzipped tar containing the pack directory at its root
/// (i.e. the archive should unpack to files like `pack.yaml`, `actions/`, etc.).
/// The multipart field name must be `pack`.
///
/// Optional form fields:
/// - `force`: `"true"` to overwrite an existing pack with the same ref
/// - `skip_tests`: `"true"` to skip test execution after registration
#[utoipa::path(
    post,
    path = "/api/v1/packs/upload",
    tag = "packs",
    request_body(content = PackUploadForm, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Pack uploaded and registered successfully", body = inline(ApiResponse<PackInstallResponse>)),
        (status = 400, description = "Invalid archive or missing pack.yaml"),
        (status = 409, description = "Pack already exists (use force=true to overwrite)"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn upload_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    mut multipart: Multipart,
) -> ApiResult<impl IntoResponse> {
    use std::io::Cursor;

    authorize_pack_registry_action(&state, &user, Action::Install).await?;

    let mut pack_bytes: Option<Vec<u8>> = None;
    let mut force = false;
    let mut skip_tests = false;

    // Parse multipart fields
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Multipart error: {}", e)))?
    {
        match field.name() {
            Some("pack") => {
                let data = field.bytes().await.map_err(|e| {
                    ApiError::BadRequest(format!("Failed to read pack data: {}", e))
                })?;
                if data.len() > PACK_UPLOAD_MAX_BYTES {
                    return Err(ApiError::BadRequest(format!(
                        "Pack archive too large: {} bytes (max {} bytes)",
                        data.len(),
                        PACK_UPLOAD_MAX_BYTES
                    )));
                }
                pack_bytes = Some(data.to_vec());
            }
            Some("force") => {
                let val = field.text().await.map_err(|e| {
                    ApiError::BadRequest(format!("Failed to read force field: {}", e))
                })?;
                force = val.trim().eq_ignore_ascii_case("true");
            }
            Some("skip_tests") => {
                let val = field.text().await.map_err(|e| {
                    ApiError::BadRequest(format!("Failed to read skip_tests field: {}", e))
                })?;
                skip_tests = val.trim().eq_ignore_ascii_case("true");
            }
            _ => {
                // Consume and ignore unknown fields
                let _ = field.bytes().await;
            }
        }
    }

    let pack_data = pack_bytes.ok_or_else(|| {
        ApiError::BadRequest("Missing required 'pack' field in multipart upload".to_string())
    })?;

    // Extract the tar.gz archive into a temporary directory
    let temp_extract_dir = tempfile::tempdir().map_err(|e| {
        ApiError::InternalServerError(format!("Failed to create temp directory: {}", e))
    })?;

    {
        let cursor = Cursor::new(&pack_data[..]);
        let gz = flate2::read::GzDecoder::new(cursor);
        let mut archive = tar::Archive::new(gz);
        // Disable destructive / privileged extraction defaults.
        archive.set_overwrite(false);
        archive.set_unpack_xattrs(false);
        archive.set_preserve_permissions(false);
        archive.set_preserve_mtime(false);

        safe_unpack(
            &mut archive,
            temp_extract_dir.path(),
            &state.config.pack_upload,
        )
        .map_err(|e| ApiError::BadRequest(format!("Failed to extract pack archive: {}", e)))?;
    }

    // Find pack.yaml — it may be at the root or inside a single subdirectory
    // (e.g. when GitHub tarballs add a top-level directory)
    let pack_root = find_pack_root(temp_extract_dir.path()).ok_or_else(|| {
        ApiError::BadRequest(
            "Could not find pack.yaml in the uploaded archive. \
             Ensure the archive contains pack.yaml at its root or in a single top-level directory."
                .to_string(),
        )
    })?;

    // Read pack ref from pack.yaml to determine the final storage path
    let pack_yaml_path = pack_root.join("pack.yaml");
    let pack_yaml_content = std::fs::read_to_string(&pack_yaml_path)
        .map_err(|e| ApiError::InternalServerError(format!("Failed to read pack.yaml: {}", e)))?;
    let pack_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&pack_yaml_content)
        .map_err(|e| ApiError::BadRequest(format!("Failed to parse pack.yaml: {}", e)))?;
    let pack_ref = pack_yaml
        .get("ref")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'ref' field in pack.yaml".to_string()))?
        .to_string();
    attune_common::schema::RefValidator::validate_pack_ref(&pack_ref)
        .map_err(|error| ApiError::BadRequest(format!("Invalid pack ref: {error}")))?;
    let _install_guard = PACK_INSTALL_LOCK.lock().await;

    // Stage and activate with a rollback guard so registration failure restores the old pack.
    use attune_common::pack_registry::PackStorage;
    let storage = PackStorage::new(&state.config.packs_base_dir);
    let replacement = storage
        .stage_pack(&pack_root, &pack_ref, None)
        .map_err(|e| {
            ApiError::InternalServerError(format!("Failed to stage pack in storage: {}", e))
        })?;
    let final_path = replacement.path().to_path_buf();

    tracing::info!(
        "Pack '{}' uploaded and stored at {:?}",
        pack_ref,
        final_path
    );

    // Register the pack in the database
    let registered_pack = register_pack_internal(
        state.clone(),
        &user,
        pack_root.to_string_lossy().to_string(),
        force,
        skip_tests,
        None,
        Some(replacement),
    )
    .await?;

    // Fetch the registered pack
    let pack = PackRepository::find_by_id(&state.db, registered_pack.id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Pack with ID {} not found", registered_pack.id))
        })?;

    emit_pack_audit(
        &state,
        &user,
        event_type::pack::UPLOADED,
        &pack,
        serde_json::json!({
            "version": pack.version.as_str(),
            "force": force,
            "skip_tests": skip_tests,
            "archive_size_bytes": pack_data.len(),
        }),
    );

    let response = ApiResponse::with_message(
        PackInstallResponse {
            pack: PackResponse::from(pack),
            test_result: None,
            tests_skipped: skip_tests,
            install_id: registered_pack
                .test_install
                .as_ref()
                .map(|install| install.id),
            install_status: registered_pack.test_install.map(|install| install.status),
            provenance: None,
        },
        "Pack uploaded and registered successfully",
    );

    Ok((StatusCode::CREATED, Json(response)))
}

/// Safely extract a tar archive into `dest`, enforcing pack-upload safety limits.
///
/// Guards applied (see [`attune_common::config::PackUploadConfig`]):
/// * Rejects entries whose path is absolute or contains `..` / non-normal components.
/// * Rejects symlinks, hardlinks, character/block devices, and FIFOs.
/// * Aborts when cumulative file count or extracted byte total exceeds configured limits.
/// * Aborts on a single entry whose declared size exceeds the per-entry limit.
///
/// The destination directory must already exist.
fn safe_unpack<R: std::io::Read>(
    archive: &mut tar::Archive<R>,
    dest: &std::path::Path,
    cfg: &attune_common::config::PackUploadConfig,
) -> Result<(), String> {
    let limits = attune_common::pack_registry::SafeExtractionLimits {
        max_entries: cfg.max_file_count(),
        max_entry_bytes: cfg.max_per_entry_size_bytes(),
        max_total_bytes: cfg.max_extracted_size_bytes(),
    };
    attune_common::pack_registry::extract_tar_archive(archive, dest, limits)
        .map_err(|error| error.to_string())
}

/// Walk the extracted directory and find the directory that contains `pack.yaml`.
/// Returns the path of the directory containing `pack.yaml`, or `None` if not found.
fn find_pack_root(base: &std::path::Path) -> Option<PathBuf> {
    // Check root first
    if base.join("pack.yaml").exists() {
        return Some(base.to_path_buf());
    }

    // Check one level deep (e.g. GitHub tarballs: repo-main/pack.yaml)
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("pack.yaml").exists() {
                return Some(path);
            }
        }
    }

    None
}

/// Register a pack from local filesystem
#[utoipa::path(
    post,
    path = "/api/v1/packs/register",
    tag = "packs",
    request_body = RegisterPackRequest,
    responses(
        (status = 201, description = "Pack registered successfully", body = ApiResponse<PackInstallResponse>),
        (status = 400, description = "Invalid request or tests failed", body = ApiResponse<String>),
        (status = 409, description = "Pack already exists", body = ApiResponse<String>),
    ),
    security(("bearer_auth" = []))
)]
pub async fn register_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<crate::dto::pack::RegisterPackRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    authorize_pack_registry_action(&state, &user, Action::Install).await?;
    let _install_guard = PACK_INSTALL_LOCK.lock().await;

    // Call internal registration logic
    let registered_pack = register_pack_internal(
        state.clone(),
        &user,
        request.path.clone(),
        request.force,
        request.skip_tests,
        None,
        None,
    )
    .await?;

    // Fetch the registered pack
    let pack = PackRepository::find_by_id(&state.db, registered_pack.id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Pack with ID {} not found", registered_pack.id))
        })?;

    emit_pack_audit(
        &state,
        &user,
        event_type::pack::REGISTERED,
        &pack,
        serde_json::json!({
            "path": request.path,
            "version": pack.version.as_str(),
            "force": request.force,
            "skip_tests": request.skip_tests,
        }),
    );

    let response =
        ApiResponse::with_message(PackResponse::from(pack), "Pack registered successfully");

    Ok((StatusCode::CREATED, Json(response)))
}

/// Internal helper function for pack registration logic
async fn register_pack_internal(
    state: Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
    path: String,
    force: bool,
    skip_tests: bool,
    installation_metadata: Option<PackInstallationMetadata>,
    mut replacement: Option<attune_common::pack_registry::PackReplacement>,
) -> Result<RegisteredPack, ApiError> {
    use std::fs;

    // Verify pack directory exists
    let source_pack_path = PathBuf::from(&path);
    if !source_pack_path.exists() || !source_pack_path.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "Pack directory does not exist: {}",
            path
        )));
    }

    // Read pack.yaml
    let pack_yaml_path = source_pack_path.join("pack.yaml");
    if !pack_yaml_path.exists() {
        return Err(ApiError::BadRequest(format!(
            "pack.yaml not found in directory: {}",
            path
        )));
    }

    let pack_yaml_content = fs::read_to_string(&pack_yaml_path)
        .map_err(|e| ApiError::InternalServerError(format!("Failed to read pack.yaml: {}", e)))?;

    let pack_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&pack_yaml_content)
        .map_err(|e| ApiError::InternalServerError(format!("Failed to parse pack.yaml: {}", e)))?;

    // Extract pack metadata
    let pack_ref = pack_yaml
        .get("ref")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'ref' field in pack.yaml".to_string()))?
        .to_string();
    attune_common::schema::RefValidator::validate_pack_ref(&pack_ref)
        .map_err(|error| ApiError::BadRequest(format!("Invalid pack ref: {error}")))?;

    let label = pack_yaml
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(&pack_ref)
        .to_string();

    let version = pack_yaml
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("Missing 'version' field in pack.yaml".to_string()))?
        .to_string();

    let description = pack_yaml
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Extract common metadata fields used for both create and update
    let conf_schema = pack_yaml
        .get("config_schema")
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let meta = pack_yaml
        .get("metadata")
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let tags: Vec<String> = pack_yaml
        .get("keywords")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let runtime_deps: Vec<String> = pack_yaml
        .get("runtime_deps")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let dependencies: Vec<String> = pack_yaml
        .get("dependencies")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let worker_selector = pack_yaml
        .get("worker_selector")
        .and_then(|value| serde_json::to_value(value).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let worker_tolerations = pack_yaml
        .get("worker_tolerations")
        .and_then(|value| serde_json::to_value(value).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    let worker_affinity = pack_yaml
        .get("worker_affinity")
        .and_then(|value| serde_json::to_value(value).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    // Keep this separate transaction open through post-registration tests and
    // rollback. Test-result writes need the registered pack to be committed.
    let mut lock_tx = state.db.begin().await?;
    PackRepository::acquire_mutation_lock(&mut lock_tx, &pack_ref).await?;
    // Move the rollback guard into a scope created after the lock transaction,
    // so error-path drops restore the filesystem before releasing the lock.
    let mut active_replacement = replacement.take();

    // Pack metadata and every component mutation commit or roll back together.
    let mut tx = state.db.begin().await?;
    let manages_storage = active_replacement.is_some();
    let existing_pack = PackRepository::find_by_ref(&mut *tx, &pack_ref).await?;
    let existing_installed_by = existing_pack.as_ref().map(|pack| pack.installed_by);
    let is_new_pack = existing_pack.is_none();

    let pack = if let Some(existing) = existing_pack {
        if !force {
            return Err(ApiError::Conflict(format!(
                "Pack '{}' already exists. Use force=true to reinstall.",
                pack_ref
            )));
        }
        authorize_existing_pack_replacement(&state, user, &existing).await?;
        let installers = installation_metadata.as_ref().map(|metadata| {
            merge_installation_provenance(&existing.installers, &metadata.provenance)
        });

        // Update existing pack in place, preserving pack and component IDs.
        let update_input = UpdatePackInput {
            label: Some(label),
            description: Some(match description {
                Some(value) => Patch::Set(value),
                None => Patch::Clear,
            }),
            version: Some(version.clone()),
            conf_schema: Some(conf_schema),
            config: None, // preserve user-set config
            meta: Some(meta),
            tags: Some(tags),
            runtime_deps: Some(runtime_deps),
            dependencies: Some(dependencies),
            is_standard: None,
            installers,
        };

        let pack = PackRepository::update(&mut *tx, existing.id, update_input).await?;
        PackRepository::update_worker_placement(
            &mut *tx,
            pack.id,
            &worker_selector,
            &worker_tolerations,
            &worker_affinity,
        )
        .await?
    } else {
        // Create new pack
        let pack_input = CreatePackInput {
            r#ref: pack_ref.clone(),
            label,
            description,
            version: version.clone(),
            conf_schema,
            config: serde_json::json!({}),
            meta,
            tags,
            runtime_deps,
            dependencies,
            is_standard: false,
            installers: installation_metadata
                .as_ref()
                .map(|metadata| {
                    merge_installation_provenance(&serde_json::json!({}), &metadata.provenance)
                })
                .unwrap_or_else(|| serde_json::json!({})),
        };

        let pack = PackRepository::create(&mut *tx, pack_input).await?;
        PackRepository::update_worker_placement(
            &mut *tx,
            pack.id,
            &worker_selector,
            &worker_tolerations,
            &worker_affinity,
        )
        .await?
    };

    if let Some(replacement) = active_replacement.as_mut() {
        replacement.activate().map_err(|e| {
            ApiError::InternalServerError(format!("Failed to activate pack: {}", e))
        })?;
    }
    let pack_path = active_replacement
        .as_ref()
        .map(|replacement| replacement.path().to_path_buf())
        .unwrap_or(source_pack_path);

    // Load pack components (triggers, actions, sensors) into the database
    {
        use attune_common::pack_registry::PackComponentLoader;

        let component_loader = PackComponentLoader::new(
            &state.db,
            pack.id,
            &pack.r#ref,
            &state.config.cache_admission,
        );
        match component_loader
            .load_all_in_transaction(&mut tx, &pack_path)
            .await
        {
            Ok(load_result) => {
                tracing::info!(
                    "Pack '{}' components loaded: {} created, {} updated, {} skipped, {} removed, {} warnings \
                     (runtimes: {}/{}, triggers: {}/{}, actions: {}/{}, policies: {}/{}, sensors: {}/{}, caches: {}/{}/{})",
                    pack.r#ref,
                    load_result.total_loaded(),
                    load_result.total_updated(),
                    load_result.total_skipped(),
                    load_result.removed,
                    load_result.warnings.len(),
                    load_result.runtimes_loaded, load_result.runtimes_updated,
                    load_result.triggers_loaded, load_result.triggers_updated,
                    load_result.actions_loaded, load_result.actions_updated,
                    load_result.policies_loaded, load_result.policies_updated,
                    load_result.sensors_loaded, load_result.sensors_updated,
                    load_result.caches_loaded, load_result.caches_updated, load_result.caches_skipped,
                );
                for warning in &load_result.warnings {
                    tracing::warn!("Pack component warning: {}", warning);
                }
            }
            Err(e) => {
                let message = format!(
                    "Pack registration failed while loading components for '{}': {}",
                    pack.r#ref, e
                );
                return Err(ApiError::BadRequest(message));
            }
        }
    }
    if let Some(mut metadata) = installation_metadata {
        if let Some(installed_by) = existing_installed_by {
            metadata.installed_by = installed_by;
        }
        PackRepository::update_installation_metadata(
            &mut *tx,
            pack.id,
            metadata.source_type,
            metadata.source_url,
            metadata.source_ref,
            metadata.checksum,
            metadata.checksum_verified,
            metadata.installed_by,
            "api".to_string(),
            metadata.storage_path,
        )
        .await?;
    }
    tx.commit().await?;
    if let Some(replacement) = active_replacement.take() {
        replacement.commit().map_err(|e| {
            ApiError::InternalServerError(format!("Failed to finalize pack activation: {}", e))
        })?;
    }

    // Auto-sync workflows after component loading succeeds.
    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);
    let service_config = PackWorkflowServiceConfig {
        packs_base_dir: packs_base_dir.clone(),
        skip_validation_errors: true,
        update_existing: true,
        max_file_size: 1024 * 1024,
    };

    let workflow_service = PackWorkflowService::new(state.db.clone(), service_config);

    // Attempt to sync workflows but don't fail if it errors
    match workflow_service.sync_pack_workflows(&pack.r#ref).await {
        Ok(sync_result) => {
            if sync_result.registered_count > 0 {
                tracing::info!(
                    "Auto-synced {} workflows for pack '{}'",
                    sync_result.registered_count,
                    pack.r#ref
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                "Failed to auto-sync workflows for pack '{}': {}",
                pack.r#ref,
                e
            );
        }
    }

    // Since entities are now updated in place (IDs preserved), ad-hoc rules
    // and cross-pack FK references survive reinstallation automatically.
    // No need to save/restore rules or re-link FKs.

    // Set up runtime environments for the pack's actions.
    // This creates virtualenvs, installs dependencies, etc. based on each
    // runtime's execution_config from the database.
    //
    // Environment directories are placed at:
    //   {runtime_envs_dir}/{pack_ref}/{runtime_name}
    // e.g., /opt/attune/runtime_envs/python_example/python
    // This keeps the pack directory clean and read-only.
    {
        use attune_common::repositories::runtime::RuntimeRepository;
        use attune_common::repositories::FindById as _;

        let runtime_envs_base = PathBuf::from(&state.config.runtime_envs_dir);

        // Collect unique runtime IDs from the pack's actions
        let actions =
            attune_common::repositories::ActionRepository::find_by_pack(&state.db, pack.id)
                .await
                .unwrap_or_default();

        let mut seen_runtime_ids = std::collections::HashSet::new();
        for action in &actions {
            if let Some(runtime_id) = action.runtime {
                seen_runtime_ids.insert(runtime_id);
            }
        }

        for runtime_id in seen_runtime_ids {
            match RuntimeRepository::find_by_id(&state.db, runtime_id).await {
                Ok(Some(rt)) => {
                    let exec_config = rt.parsed_execution_config();
                    let rt_name = rt.name.to_lowercase();

                    // Check if this runtime has environment/dependency config
                    if exec_config.environment.is_some() || exec_config.has_dependencies(&pack_path)
                    {
                        // Compute external env_dir: {runtime_envs_dir}/{pack_ref}/{runtime_name}
                        let env_dir = runtime_envs_base.join(&pack.r#ref).join(&rt_name);

                        tracing::info!(
                            "Runtime '{}' for pack '{}' requires environment setup (env_dir: {})",
                            rt.name,
                            pack.r#ref,
                            env_dir.display()
                        );

                        // Attempt to create environment if configured.
                        // NOTE: In Docker deployments the API container typically does NOT
                        // have runtime interpreters (e.g., python3) installed, so this will
                        // fail. That is expected — the worker service will create the
                        // environment on-demand before the first execution. This block is
                        // a best-effort optimisation for non-Docker (bare-metal) setups
                        // where the API host has the interpreter available.
                        if let Some(ref env_cfg) = exec_config.environment {
                            if env_cfg.env_type != "none"
                                && !env_dir.exists()
                                && !env_cfg.create_command.is_empty()
                            {
                                // Ensure parent directories exist
                                if let Some(parent) = env_dir.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }

                                let vars = exec_config
                                    .build_template_vars_with_env(&pack_path, Some(&env_dir));
                                let resolved_cmd = attune_common::models::runtime::RuntimeExecutionConfig::resolve_command(
                                        &env_cfg.create_command,
                                        &vars,
                                    );

                                tracing::info!(
                                    "Attempting to create {} environment (best-effort) at {}: {:?}",
                                    env_cfg.env_type,
                                    env_dir.display(),
                                    resolved_cmd
                                );

                                if let Some((program, args)) = resolved_cmd.split_first() {
                                    match tokio::process::Command::new(program)
                                        .args(args)
                                        .current_dir(&pack_path)
                                        .output()
                                        .await
                                    {
                                        Ok(output) if output.status.success() => {
                                            tracing::info!(
                                                "Created {} environment at {}",
                                                env_cfg.env_type,
                                                env_dir.display()
                                            );
                                        }
                                        Ok(output) => {
                                            let stderr = String::from_utf8_lossy(&output.stderr);
                                            tracing::info!(
                                                    "Environment creation skipped in API service (exit {}): {}. \
                                                     The worker will create it on first execution.",
                                                    output.status.code().unwrap_or(-1),
                                                    stderr.trim()
                                                );
                                        }
                                        Err(e) => {
                                            tracing::info!(
                                                    "Runtime '{}' not available in API service: {}. \
                                                     The worker will create the environment on first execution.",
                                                    program, e
                                                );
                                        }
                                    }
                                }
                            }
                        }

                        // Attempt to install dependencies if manifest file exists.
                        // Same caveat as above — this is best-effort in the API service.
                        if let Some(ref dep_cfg) = exec_config.dependencies {
                            let manifest_path = pack_path.join(&dep_cfg.manifest_file);
                            if manifest_path.exists() && !dep_cfg.install_command.is_empty() {
                                // Only attempt if the environment directory already exists
                                // (i.e., the venv creation above succeeded).
                                let env_exists = env_dir.exists();

                                if env_exists {
                                    let vars = exec_config
                                        .build_template_vars_with_env(&pack_path, Some(&env_dir));
                                    let resolved_cmd = attune_common::models::runtime::RuntimeExecutionConfig::resolve_command(
                                        &dep_cfg.install_command,
                                        &vars,
                                    );

                                    tracing::info!(
                                        "Installing dependencies for pack '{}': {:?}",
                                        pack.r#ref,
                                        resolved_cmd
                                    );

                                    if let Some((program, args)) = resolved_cmd.split_first() {
                                        match tokio::process::Command::new(program)
                                            .args(args)
                                            .current_dir(&pack_path)
                                            .output()
                                            .await
                                        {
                                            Ok(output) if output.status.success() => {
                                                tracing::info!(
                                                    "Dependencies installed for pack '{}'",
                                                    pack.r#ref
                                                );
                                            }
                                            Ok(output) => {
                                                let stderr =
                                                    String::from_utf8_lossy(&output.stderr);
                                                tracing::info!(
                                                    "Dependency installation skipped in API service (exit {}): {}. \
                                                     The worker will handle this on first execution.",
                                                    output.status.code().unwrap_or(-1),
                                                    stderr.trim()
                                                );
                                            }
                                            Err(e) => {
                                                tracing::info!(
                                                    "Dependency installer not available in API service: {}. \
                                                     The worker will handle this on first execution.",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                } else {
                                    tracing::info!(
                                        "Skipping dependency installation for pack '{}' — \
                                         environment not yet created. The worker will handle \
                                         environment setup and dependency installation on first execution.",
                                        pack.r#ref
                                    );
                                }
                            }
                        }
                    }
                }
                Ok(None) => {
                    tracing::debug!(
                        "Runtime ID {} not found, skipping environment setup",
                        runtime_id
                    );
                }
                Err(e) => {
                    tracing::warn!("Failed to load runtime {}: {}", runtime_id, e);
                }
            }
        }
    }

    // Publish pack.registered before dispatching tests. In volume transport
    // mode workers receive the pack contents through this event; dispatching
    // the test first lets a worker observe the test request before the pack
    // directory has been synchronized, which can fail the install and roll
    // back a newly registered pack.
    if let Some(publisher) = state.get_publisher().await {
        let runtime_names = attune_common::pack_environment::collect_runtime_names_for_pack(
            &state.db, pack.id, &pack_path,
        )
        .await;

        let payload = PackRegisteredPayload {
            pack_id: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version.clone(),
            runtime_names: runtime_names.clone(),
        };

        let envelope = MessageEnvelope::new(MessageType::PackRegistered, payload);

        match publisher.publish_envelope(&envelope).await {
            Ok(()) => {
                tracing::info!(
                    "Published pack.registered event for pack '{}' (runtimes: {:?})",
                    pack.r#ref,
                    runtime_names,
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to publish pack.registered event for pack '{}': {}. \
                     Workers will sync pack content lazily on first execution.",
                    pack.r#ref,
                    e,
                );
            }
        }
    }

    // Execute tests if not skipped
    let mut test_install = None;
    if !skip_tests {
        let trigger_reason = if is_new_pack { "install" } else { "update" };
        if let Some(dispatch_outcome) = dispatch_and_track_pack_tests(
            &state,
            Some(pack.id),
            &pack.r#ref,
            &pack.version,
            trigger_reason,
            &pack_path,
            None,
            pack.worker_selector.clone(),
            pack.worker_tolerations.clone(),
            pack.worker_affinity.clone(),
        )
        .await
        {
            match dispatch_outcome {
                Ok(install) => {
                    test_install = Some(install.clone());
                    // Mark the pack as pending while tests run on a worker, then
                    // let the background finalizer watch for the terminal state.
                    if let Err(e) =
                        mark_pack_install_status(&state.db, &pack.r#ref, "pending").await
                    {
                        tracing::warn!(
                            "Failed to mark pack '{}' as pending install: {}",
                            pack.r#ref,
                            e
                        );
                    }
                    tracing::info!(
                        "Pack tests for '{}' dispatched (install {}); finalizing in background",
                        pack.r#ref,
                        install.id
                    );
                    let finalize_state = state.clone();
                    let finalize_ref = pack.r#ref.clone();
                    tokio::spawn(async move {
                        finalize_pack_install(
                            finalize_state,
                            install.id,
                            finalize_ref,
                            pack.id,
                            is_new_pack,
                            force,
                            manages_storage,
                            true,
                        )
                        .await;
                    });
                }
                Err(e) => {
                    tracing::warn!("Failed to dispatch tests for pack '{}': {}", pack.r#ref, e);
                    // If tests can't be dispatched and force is not set, fail the registration
                    if !force {
                        if is_new_pack {
                            match delete_failed_pack_registration(
                                &state,
                                pack.id,
                                &pack.r#ref,
                                manages_storage,
                            )
                            .await
                            {
                                Ok((true, _)) => {}
                                Ok((false, _)) => tracing::error!(
                                    "Failed to roll back new pack '{}' after test dispatch error: pack row disappeared",
                                    pack.r#ref
                                ),
                                Err(delete_error) => tracing::error!(
                                    "Failed to roll back new pack '{}' after test dispatch error: {}",
                                    pack.r#ref, delete_error
                                ),
                            }
                        }
                        return Err(ApiError::BadRequest(format!(
                            "Pack registration failed: could not dispatch tests. Error: {}. Use force=true to register anyway.",
                            e
                        )));
                    }
                }
            }
        } else {
            tracing::info!(
                "No tests to run for pack '{}', proceeding with registration",
                pack.r#ref
            );
        }
    }

    publish_pack_metadata_change(&state, &pack, "registered", pack.updated).await;

    lock_tx.rollback().await?;
    Ok(RegisteredPack {
        id: pack.id,
        test_install,
    })
}

async fn authorize_pack_registry_action(
    state: &AppState,
    user: &crate::auth::middleware::AuthenticatedUser,
    action: Action,
) -> ApiResult<()> {
    require_pack_access_token(&user.claims.token_type)?;
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let authz = state.authorization_service();
    authz
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Packs,
                action,
                context: AuthorizationContext::new(identity_id),
            },
        )
        .await?;
    Ok(())
}

async fn authorize_global_pack_registry_action(
    state: &AppState,
    user: &crate::auth::middleware::AuthenticatedUser,
    action: Action,
) -> ApiResult<()> {
    require_pack_access_token(&user.claims.token_type)?;
    let grants = state.authorization_service().effective_grants(user).await?;
    if !crate::routes::visibility::has_unconstrained_resource_action(
        &grants,
        Resource::Packs,
        action,
    ) {
        let mut audit = AuditEventBuilder::new(
            AuditCategory::Rbac,
            event_type::rbac::DENIED,
            AuditOutcome::Denied,
        )
        .actor_login(user.login().to_string())
        .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase())
        .resource("packs")
        .with_details(serde_json::json!({
            "resource": "packs",
            "action": format!("{:?}", action).to_lowercase(),
            "scope": "global_pack_index",
            "reason": "unconstrained_grant_required",
        }));
        if let Ok(identity_id) = user.identity_id() {
            audit = audit.actor_identity(identity_id);
        }
        state.audit_emitter.emit(audit.build());
        return Err(ApiError::Forbidden(format!(
            "Global pack index access requires an unconstrained Packs {:?} grant",
            action
        )));
    }
    Ok(())
}

async fn authorize_existing_pack_replacement(
    state: &AppState,
    user: &crate::auth::middleware::AuthenticatedUser,
    pack: &Pack,
) -> ApiResult<()> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let authz = state.authorization_service();
    let grants = authz.effective_grants(user).await?;
    let context = pack_authorization_context(identity_id, pack);
    let allowed = if pack.installed_by.is_some() && pack.installed_by != Some(identity_id) {
        constrained_pack_grant_allows(&grants, Action::Configure, &context)
    } else {
        AuthorizationService::is_allowed(&grants, Resource::Packs, Action::Configure, &context)
    };
    if !allowed {
        return Err(ApiError::Forbidden(
            "Not authorized to replace pack".to_string(),
        ));
    }
    if pack.installed_by == Some(identity_id) || pack.installed_by.is_none() {
        authz
            .authorize(
                user,
                AuthorizationCheck {
                    resource: Resource::Packs,
                    action: Action::Configure,
                    context,
                },
            )
            .await?;
    }
    Ok(())
}

fn require_pack_access_token(token_type: &crate::auth::jwt::TokenType) -> ApiResult<()> {
    if token_type != &crate::auth::jwt::TokenType::Access {
        return Err(ApiError::Forbidden(
            "This pack operation requires an access token".to_string(),
        ));
    }
    Ok(())
}

async fn validate_managed_registry_url(state: &AppState, url: &str) -> ApiResult<String> {
    let policy =
        attune_common::pack_registry::OutboundUrlPolicy::from_config(&state.config.pack_registry)
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let validated = policy
        .validate(url)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(validated.url.to_string())
}

fn headers_from_json(
    headers: serde_json::Value,
) -> ApiResult<std::collections::HashMap<String, String>> {
    let mut result = std::collections::HashMap::new();
    let Some(object) = headers.as_object() else {
        return Err(ApiError::BadRequest(
            "Pack index headers must be a JSON object".to_string(),
        ));
    };
    for (key, value) in object {
        let Some(value) = value.as_str() else {
            return Err(ApiError::BadRequest(
                "Pack index header values must be strings".to_string(),
            ));
        };
        result.insert(key.clone(), value.to_string());
    }
    attune_common::pack_registry::validate_registry_headers(&result)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(result)
}

fn registry_encryption_key(state: &AppState) -> ApiResult<&str> {
    state
        .config
        .security
        .encryption_key
        .as_deref()
        .ok_or_else(|| {
            ApiError::InternalServerError(
                "Cannot store registry credentials without security.encryption_key".to_string(),
            )
        })
}

fn encrypt_managed_headers(
    state: &AppState,
    requested: serde_json::Value,
    existing: Option<&serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    if requested.as_object().is_some_and(serde_json::Map::is_empty) {
        return Ok(requested);
    }
    encrypt_managed_headers_with_key(requested, existing, registry_encryption_key(state)?)
}

fn encrypt_managed_headers_with_key(
    requested: serde_json::Value,
    existing: Option<&serde_json::Value>,
    encryption_key: &str,
) -> ApiResult<serde_json::Value> {
    let mut requested = headers_from_json(requested)?;
    let existing = existing
        .cloned()
        .map(headers_from_json)
        .transpose()?
        .unwrap_or_default();
    for (name, value) in &mut requested {
        if value == "[REDACTED]" {
            *value = existing.get(name).cloned().ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "Redacted registry header '{}' has no existing value",
                    name
                ))
            })?;
        }
    }
    let plaintext = serde_json::to_value(requested)
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
    attune_common::crypto::encrypt_json(&plaintext, encryption_key)
        .map_err(|e| ApiError::InternalServerError(e.to_string()))
}

async fn decrypt_managed_headers(
    state: &AppState,
    mut index: attune_common::models::PackRegistryIndex,
) -> ApiResult<(attune_common::models::PackRegistryIndex, serde_json::Value)> {
    let plaintext = plaintext_managed_headers(state, &index.headers)?;
    if !index.headers.is_string()
        && !index
            .headers
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
    {
        // Encrypt rows created before managed registry credentials were encrypted at rest.
        let encrypted = encrypt_managed_headers(state, plaintext.clone(), None)?;
        if let Some(migrated) = PackRegistryIndexRepository::compare_and_set_headers(
            &state.db,
            index.id,
            &index.headers,
            encrypted,
        )
        .await?
        {
            index = migrated;
        }
    }
    Ok((index, plaintext))
}

fn plaintext_managed_headers(
    state: &AppState,
    stored: &serde_json::Value,
) -> ApiResult<serde_json::Value> {
    let plaintext = if stored.as_object().is_some_and(serde_json::Map::is_empty) {
        stored.clone()
    } else if stored.is_string() {
        attune_common::crypto::decrypt_json(stored, registry_encryption_key(state)?)
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?
    } else {
        stored.clone()
    };
    headers_from_json(plaintext.clone())?;
    Ok(plaintext)
}

async fn registry_index_response(
    state: &AppState,
    index: attune_common::models::PackRegistryIndex,
) -> ApiResult<PackRegistryIndexResponse> {
    attune_common::pack_registry::validate_remote_pack_url(&index.url)
        .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
    let (index, headers) = decrypt_managed_headers(state, index).await?;
    Ok(PackRegistryIndexResponse::from_index_and_headers(
        index, headers,
    ))
}

struct EffectivePackRegistry {
    config: attune_common::config::PackRegistryConfig,
    summaries: Vec<PackRegistryIndexSummary>,
}

async fn effective_pack_registry(
    state: &AppState,
    include_disabled: bool,
) -> ApiResult<EffectivePackRegistry> {
    let mut config = state.config.pack_registry.clone();
    if !state.config.pack_registry.enabled {
        config.indices.clear();
        config.approved_public_hosts.clear();
        config.approved_private_hosts.clear();
        config.approved_private_cidrs.clear();
        config.allow_http = false;
        return Ok(EffectivePackRegistry {
            config,
            summaries: Vec::new(),
        });
    }

    let managed = PackRegistryIndexRepository::list(&state.db).await?;
    let (managed, managed_urls) = deduplicate_managed_registry_indices(managed);
    let static_bootstrap_enabled = static_bootstrap_indices_are_effective(&managed);
    let static_position_offset = managed
        .iter()
        .map(|index| index.position.max(0) as u32)
        .max()
        .map_or(0, |position| position.saturating_add(1));
    let mut indices = Vec::new();
    let mut summaries = Vec::new();
    for index in managed {
        if !include_disabled && !index.enabled {
            continue;
        }
        let (index, headers) = decrypt_managed_headers(state, index).await?;
        summaries.push(PackRegistryIndexSummary {
            id: Some(index.id),
            name: index.name.clone(),
            url: index.url.clone(),
            position: index.position,
        });
        indices.push(attune_common::config::RegistryIndexConfig {
            url: index.url,
            priority: index.position.max(0) as u32,
            enabled: index.enabled,
            name: index.name,
            headers: headers_from_json(headers)?,
        });
    }
    if static_bootstrap_enabled {
        for index in effective_static_registry_indices(
            &state.config.pack_registry.indices,
            &managed_urls,
            static_position_offset,
            include_disabled,
        ) {
            summaries.push(PackRegistryIndexSummary {
                id: None,
                name: index.name.clone(),
                url: index.url.clone(),
                position: i32::try_from(index.priority).unwrap_or(i32::MAX),
            });
            indices.push(index);
        }
    }

    config.indices = indices;
    Ok(EffectivePackRegistry { config, summaries })
}

async fn selected_managed_pack_registry(
    state: &AppState,
    registry_id: i64,
) -> ApiResult<EffectivePackRegistry> {
    if !state.config.pack_registry.enabled {
        return Err(ApiError::BadRequest(
            "Pack registry resolution is disabled".to_string(),
        ));
    }
    let index = PackRegistryIndexRepository::find_by_id(&state.db, registry_id)
        .await?
        .filter(|index| index.enabled)
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Enabled managed pack index {} was not found",
                registry_id
            ))
        })?;
    let (index, headers) = decrypt_managed_headers(state, index).await?;
    let summary = PackRegistryIndexSummary {
        id: Some(index.id),
        name: index.name.clone(),
        url: index.url.clone(),
        position: index.position,
    };
    let mut config = state.config.pack_registry.clone();
    config.indices = vec![attune_common::config::RegistryIndexConfig {
        url: index.url,
        priority: index.position.max(0) as u32,
        enabled: true,
        name: index.name,
        headers: headers_from_json(headers)?,
    }];
    Ok(EffectivePackRegistry {
        config,
        summaries: vec![summary],
    })
}

fn effective_static_registry_indices(
    static_indices: &[attune_common::config::RegistryIndexConfig],
    managed_urls: &std::collections::HashSet<String>,
    position_offset: u32,
    include_disabled: bool,
) -> Vec<attune_common::config::RegistryIndexConfig> {
    let mut seen_urls = managed_urls.clone();
    let mut candidates: Vec<_> = static_indices
        .iter()
        .filter(|index| include_disabled || index.enabled)
        .cloned()
        .collect();
    candidates.sort_by_key(|index| index.priority);
    candidates
        .into_iter()
        .filter(|index| seen_urls.insert(registry_identity_key(&index.url)))
        .map(|mut index| {
            index.priority = position_offset.saturating_add(index.priority);
            index
        })
        .collect()
}

fn deduplicate_managed_registry_indices(
    managed: Vec<attune_common::models::PackRegistryIndex>,
) -> (
    Vec<attune_common::models::PackRegistryIndex>,
    std::collections::HashSet<String>,
) {
    let mut identities = std::collections::HashSet::new();
    let managed = managed
        .into_iter()
        .filter(|index| identities.insert(registry_identity_key(&index.url)))
        .collect();
    (managed, identities)
}

fn static_bootstrap_indices_are_effective(
    managed: &[attune_common::models::PackRegistryIndex],
) -> bool {
    managed.iter().all(|index| index.is_standard)
}

fn static_indices_would_reactivate(
    before: &[attune_common::models::PackRegistryIndex],
    after: &[attune_common::models::PackRegistryIndex],
    static_indices: &[attune_common::config::RegistryIndexConfig],
) -> bool {
    let managed_urls = after
        .iter()
        .map(|index| registry_identity_key(&index.url))
        .collect();
    !static_bootstrap_indices_are_effective(before)
        && static_bootstrap_indices_are_effective(after)
        && !effective_static_registry_indices(static_indices, &managed_urls, 0, false).is_empty()
}

fn registry_identity_key(url: &str) -> String {
    registry_url_key(url)
}

fn registry_url_key(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    if let Some(host) = parsed.host_str() {
        let normalized = host.trim_end_matches('.').to_ascii_lowercase();
        let _ = parsed.set_host(Some(&normalized));
    }
    if parsed.port() == parsed.port_or_known_default() {
        let _ = parsed.set_port(None);
    }
    parsed.to_string()
}

#[utoipa::path(
    get,
    path = "/api/v1/pack-indices",
    tag = "packs",
    responses(
        (status = 200, description = "Configured pack registry indices", body = inline(ApiResponse<Vec<PackRegistryIndexResponse>>)),
        (status = 401, description = "Unauthorized", body = crate::auth::middleware::AuthErrorResponse),
        (status = 403, description = "Forbidden", body = crate::middleware::error::ErrorResponse),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_pack_indices(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
) -> ApiResult<impl IntoResponse> {
    authorize_global_pack_registry_action(&state, &user, Action::Read).await?;
    let indices = PackRegistryIndexRepository::list(&state.db).await?;
    let mut response = Vec::with_capacity(indices.len());
    for index in indices {
        response.push(registry_index_response(&state, index).await?);
    }
    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

#[utoipa::path(
    post,
    path = "/api/v1/pack-indices",
    tag = "packs",
    request_body = CreatePackRegistryIndexRequest,
    responses(
        (status = 201, description = "Pack registry index created", body = inline(ApiResponse<PackRegistryIndexResponse>)),
        (status = 400, description = "Validation error", body = crate::middleware::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::auth::middleware::AuthErrorResponse),
        (status = 403, description = "Forbidden", body = crate::middleware::error::ErrorResponse),
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_pack_index(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreatePackRegistryIndexRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;
    authorize_global_pack_registry_action(&state, &user, Action::Configure).await?;
    let url = validate_managed_registry_url(&state, &request.url).await?;
    let headers = encrypt_managed_headers(&state, request.headers, None)?;
    let index = PackRegistryIndexRepository::create(
        &state.db,
        CreatePackRegistryIndexInput {
            name: request.name,
            url,
            position: request.position,
            enabled: request.enabled,
            headers,
        },
    )
    .await?;
    let response = registry_index_response(&state, index.clone()).await?;
    emit_pack_index_audit(&state, &user, PACK_INDEX_CREATED_EVENT, &index);
    Ok((StatusCode::CREATED, Json(ApiResponse::new(response))))
}

#[utoipa::path(
    put,
    path = "/api/v1/pack-indices/{id}",
    tag = "packs",
    params(
        ("id" = i64, Path, description = "Pack registry index ID")
    ),
    request_body = UpdatePackRegistryIndexRequest,
    responses(
        (status = 200, description = "Pack registry index updated", body = inline(ApiResponse<PackRegistryIndexResponse>)),
        (status = 400, description = "Validation error", body = crate::middleware::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::auth::middleware::AuthErrorResponse),
        (status = 403, description = "Forbidden", body = crate::middleware::error::ErrorResponse),
        (status = 404, description = "Pack registry index not found", body = crate::middleware::error::ErrorResponse),
        (status = 409, description = "Update would reactivate static pack indices", body = crate::middleware::error::ErrorResponse),
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_pack_index(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i64>,
    Json(request): Json<UpdatePackRegistryIndexRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;
    authorize_global_pack_registry_action(&state, &user, Action::Configure).await?;
    let url = match request.url.as_deref() {
        Some(url) => Some(validate_managed_registry_url(&state, url).await?),
        None => None,
    };

    let mut tx = state.db.begin().await?;
    acquire_pack_registry_mutation_lock(&mut tx).await?;
    let indices = PackRegistryIndexRepository::list(&mut *tx).await?;
    let existing = indices
        .iter()
        .find(|index| index.id == id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("Pack index {} not found", id)))?;
    let existing_headers = plaintext_managed_headers(&state, &existing.headers)?;
    let headers = match request.headers {
        Some(headers) => Some(encrypt_managed_headers(
            &state,
            headers,
            Some(&existing_headers),
        )?),
        None if !existing.headers.is_string()
            && !existing_headers
                .as_object()
                .is_some_and(serde_json::Map::is_empty) =>
        {
            Some(encrypt_managed_headers(&state, existing_headers, None)?)
        }
        None => None,
    };
    let index = PackRegistryIndexRepository::update(
        &mut *tx,
        id,
        UpdatePackRegistryIndexInput {
            name: request.name,
            url,
            position: request.position,
            enabled: request.enabled,
            headers,
        },
    )
    .await?;
    tx.commit().await?;
    let response = registry_index_response(&state, index.clone()).await?;
    emit_pack_index_audit(&state, &user, PACK_INDEX_UPDATED_EVENT, &index);
    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

#[utoipa::path(
    delete,
    path = "/api/v1/pack-indices/{id}",
    tag = "packs",
    params(
        ("id" = i64, Path, description = "Pack registry index ID")
    ),
    responses(
        (status = 200, description = "Pack registry index deleted", body = SuccessResponse),
        (status = 401, description = "Unauthorized", body = crate::auth::middleware::AuthErrorResponse),
        (status = 403, description = "Forbidden", body = crate::middleware::error::ErrorResponse),
        (status = 404, description = "Pack registry index not found", body = crate::middleware::error::ErrorResponse),
        (status = 409, description = "Deletion would reactivate static pack indices", body = crate::middleware::error::ErrorResponse),
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_pack_index(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize_global_pack_registry_action(&state, &user, Action::Configure).await?;
    let mut tx = state.db.begin().await?;
    acquire_pack_registry_mutation_lock(&mut tx).await?;
    let indices = PackRegistryIndexRepository::list(&mut *tx).await?;
    let existing = indices
        .iter()
        .find(|index| index.id == id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("Pack index {} not found", id)))?;
    let after: Vec<_> = indices
        .iter()
        .filter(|index| index.id != id)
        .cloned()
        .collect();
    if !existing.is_standard
        && static_indices_would_reactivate(&indices, &after, &state.config.pack_registry.indices)
    {
        return Err(ApiError::Conflict(
            "Cannot delete the last non-standard managed index while enabled static pack indices are configured; remove or disable the static entries first"
                .to_string(),
        ));
    }
    let deleted = PackRegistryIndexRepository::delete(&mut *tx, id).await?;
    if !deleted {
        return Err(ApiError::NotFound(format!("Pack index {} not found", id)));
    }
    tx.commit().await?;
    emit_pack_index_audit(&state, &user, PACK_INDEX_DELETED_EVENT, &existing);
    Ok((
        StatusCode::OK,
        Json(SuccessResponse::new(format!("Pack index {} deleted", id))),
    ))
}

async fn acquire_pack_registry_mutation_lock(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind("pack_registry_index_mutation")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn emit_pack_index_audit(
    state: &Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
    event_type: &'static str,
    index: &attune_common::models::PackRegistryIndex,
) {
    let headers_configured = !index
        .headers
        .as_object()
        .is_some_and(serde_json::Map::is_empty);
    let mut builder =
        AuditEventBuilder::new(AuditCategory::Admin, event_type, AuditOutcome::Success)
            .resource("pack_registry_index")
            .resource_id(index.id)
            .resource_ref(index.url.clone())
            .with_details(serde_json::json!({
                "name": index.name,
                "url": index.url,
                "position": index.position,
                "enabled": index.enabled,
                "headers_configured": headers_configured,
            }));
    if let Ok(identity_id) = user.identity_id() {
        builder = builder.actor_identity(identity_id);
    }
    builder = builder
        .actor_login(user.login().to_string())
        .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase());
    state.audit_emitter.emit(builder.build());
}

#[utoipa::path(
    get,
    path = "/api/v1/pack-indices/packs",
    tag = "packs",
    params(
        ("q" = Option<String>, Query, description = "Text to match against indexed packs"),
        ("registry_id" = Option<i64>, Query, description = "Restrict results to a configured registry index"),
        ("include_disabled" = Option<bool>, Query, description = "Include disabled registry indices"),
    ),
    responses(
        (status = 200, description = "Available indexed packs", body = inline(ApiResponse<Vec<IndexedPackResponse>>)),
        (status = 400, description = "Invalid or disabled selected registry", body = crate::middleware::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::auth::middleware::AuthErrorResponse),
        (status = 403, description = "Forbidden", body = crate::middleware::error::ErrorResponse),
    ),
    security(("bearer_auth" = []))
)]
pub async fn browse_indexed_packs(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<BrowsePackIndexQuery>,
) -> ApiResult<impl IntoResponse> {
    authorize_global_pack_registry_action(&state, &user, Action::Read).await?;
    if query.include_disabled {
        authorize_global_pack_registry_action(&state, &user, Action::Configure).await?;
    }
    let effective = match query.registry_id {
        Some(registry_id) => selected_managed_pack_registry(&state, registry_id).await?,
        None => effective_pack_registry(&state, query.include_disabled).await?,
    };
    let client = attune_common::pack_registry::RegistryClient::new(effective.config)
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
    let summaries = effective.summaries;
    let query_text = query.q.unwrap_or_default().to_lowercase();
    let mut seen = std::collections::HashSet::new();
    let mut packs = Vec::new();

    for registry in client.get_registries_including_disabled(query.include_disabled) {
        let Some(summary) = summaries.iter().find(|summary| summary.url == registry.url) else {
            continue;
        };
        match client.fetch_index(&registry).await {
            Ok(index) => {
                for pack in index.packs {
                    if !seen.insert(pack.pack_ref.clone()) {
                        continue;
                    }
                    let matches_query = query_text.is_empty()
                        || pack.pack_ref.to_lowercase().contains(&query_text)
                        || pack.label.to_lowercase().contains(&query_text)
                        || pack.description.to_lowercase().contains(&query_text)
                        || pack
                            .use_case
                            .as_deref()
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(&query_text)
                        || pack
                            .keywords
                            .iter()
                            .any(|keyword| keyword.to_lowercase().contains(&query_text));
                    if matches_query {
                        packs.push(IndexedPackResponse {
                            pack,
                            registry: summary.clone(),
                        });
                    }
                }
            }
            Err(e) => tracing::warn!("Failed to fetch pack index {}: {}", registry.url, e),
        }
    }

    Ok((StatusCode::OK, Json(ApiResponse::new(packs))))
}

#[utoipa::path(
    get,
    path = "/api/v1/pack-indices/packs/{ref}",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Indexed pack reference identifier")
    ),
    responses(
        (status = 200, description = "Indexed pack", body = inline(ApiResponse<IndexedPackResponse>)),
        (status = 401, description = "Unauthorized", body = crate::auth::middleware::AuthErrorResponse),
        (status = 403, description = "Forbidden", body = crate::middleware::error::ErrorResponse),
        (status = 404, description = "Indexed pack not found", body = crate::middleware::error::ErrorResponse),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_indexed_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    authorize_global_pack_registry_action(&state, &user, Action::Read).await?;
    let effective = effective_pack_registry(&state, false).await?;
    let client = attune_common::pack_registry::RegistryClient::new(effective.config)
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
    let summaries = effective.summaries;

    for registry in client.get_registries() {
        let summary = summaries
            .iter()
            .find(|summary| summary.url == registry.url)
            .cloned();
        match client.fetch_index(&registry).await {
            Ok(index) => {
                if let Some(pack) = index
                    .packs
                    .into_iter()
                    .find(|pack| pack.pack_ref == pack_ref)
                {
                    return Ok((
                        StatusCode::OK,
                        Json(ApiResponse::new(IndexedPackResponse {
                            pack,
                            registry: summary.unwrap_or(PackRegistryIndexSummary {
                                id: None,
                                name: registry.name,
                                url: registry.url,
                                position: registry.priority as i32,
                            }),
                        })),
                    ));
                }
            }
            Err(e) => tracing::warn!("Failed to fetch pack index {}: {}", registry.url, e),
        }
    }

    Err(ApiError::NotFound(format!(
        "Indexed pack '{}' not found",
        pack_ref
    )))
}

/// Install a pack from a Git, archive, local, or managed-registry source.
#[utoipa::path(
    post,
    path = "/api/v1/packs/install",
    tag = "packs",
    request_body = InstallPackRequest,
    responses(
        (status = 200, description = "Pack installed successfully", body = ApiResponse<PackInstallResponse>),
        (status = 400, description = "Invalid request or tests failed", body = crate::middleware::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::auth::middleware::AuthErrorResponse),
        (status = 403, description = "Forbidden", body = crate::middleware::error::ErrorResponse),
        (status = 404, description = "Pack or local source not found", body = crate::middleware::error::ErrorResponse),
    ),
    security(("bearer_auth" = []))
)]
pub async fn install_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<crate::dto::pack::InstallPackRequest>,
) -> ApiResult<(
    StatusCode,
    Json<crate::dto::ApiResponse<PackInstallResponse>>,
)> {
    use attune_common::pack_registry::{
        calculate_directory_checksum, DependencyValidator, PackInstaller, PackStorage,
    };
    use attune_common::repositories::List;

    authorize_pack_registry_action(&state, &user, Action::Install).await?;

    // Get user ID early to avoid borrow issues
    let user_id = user.identity_id().ok();
    // Create temp directory for installations
    let temp_dir = std::env::temp_dir().join("attune-pack-installs");

    let source = detect_pack_source(
        &request.source,
        request.ref_spec.as_deref(),
        !request.no_registry,
    )?;
    if request.registry_id.is_some()
        && !matches!(
            source,
            attune_common::pack_registry::PackSource::Registry { .. }
        )
    {
        return Err(ApiError::BadRequest(
            "registry_id can only be used with a registry pack reference".to_string(),
        ));
    }

    let is_registry_install = matches!(
        source,
        attune_common::pack_registry::PackSource::Registry { .. }
    );
    let effective_registry = if is_registry_install {
        Some(if let Some(registry_id) = request.registry_id {
            selected_managed_pack_registry(&state, registry_id).await?
        } else {
            effective_pack_registry(&state, false).await?
        })
    } else {
        None
    };
    let registry_summaries = effective_registry
        .as_ref()
        .map(|registry| registry.summaries.clone())
        .unwrap_or_default();
    let mut registry_config = effective_registry
        .map(|registry| registry.config)
        .unwrap_or_else(|| direct_pack_registry_config(&state.config.pack_registry));
    let registry_resolution = if is_registry_install {
        let resolution = resolve_registry_request(&registry_config, &source).await?;
        registry_config
            .indices
            .retain(|index| index.url == resolution.registry_url);
        Some(resolution)
    } else {
        None
    };

    // Create installer
    let installer = PackInstaller::new(&temp_dir, Some(registry_config))
        .await
        .map_err(|e| ApiError::InternalServerError(format!("Failed to create installer: {}", e)))?;

    tracing::info!(
        source_type = get_source_type(&source),
        "Installing pack from requested source"
    );

    // Install the pack (to temporary location)
    let installed = match registry_resolution.as_ref() {
        Some(resolution) => {
            installer
                .install_resolved_registry_pack(
                    resolution.entry.clone(),
                    resolution.registry_url.clone(),
                )
                .await?
        }
        None => installer.install(source.clone()).await?,
    };
    let mut temporary_install_cleanup = TemporaryInstallCleanup::new(&temp_dir, &installed.path);

    tracing::info!("Pack downloaded to: {:?}", installed.path);

    // Validate dependencies if not skipping
    if !request.skip_deps {
        tracing::info!("Validating pack dependencies...");

        // Load pack.yaml for dependency information
        let pack_yaml_path = installed.path.join("pack.yaml");
        if !pack_yaml_path.exists() {
            let pack_yaml_relative = pack_yaml_path
                .strip_prefix(&temp_dir)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| installed.path.display().to_string());
            return Err(ApiError::BadRequest(format!(
                "pack.yaml not found in installed pack at: {}",
                pack_yaml_relative
            )));
        }

        let pack_yaml_content = std::fs::read_to_string(&pack_yaml_path).map_err(|e| {
            ApiError::InternalServerError(format!("Failed to read pack.yaml: {}", e))
        })?;

        let pack_yaml: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&pack_yaml_content).map_err(|e| {
                ApiError::InternalServerError(format!("Failed to parse pack.yaml: {}", e))
            })?;

        let mut validator = DependencyValidator::new();

        // Extract runtime dependencies from pack.yaml
        let mut runtime_deps: Vec<String> = Vec::new();

        if let Some(python_version) = pack_yaml.get("python").and_then(|v| v.as_str()) {
            runtime_deps.push(format!("python3>={}", python_version));
        }

        if let Some(nodejs_version) = pack_yaml.get("nodejs").and_then(|v| v.as_str()) {
            runtime_deps.push(format!("nodejs>={}", nodejs_version));
        }

        // Extract pack dependencies (ref, version)
        let pack_deps: Vec<(String, String)> = pack_yaml
            .get("dependencies")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(|s| (s.to_string(), "*".to_string())))
                    .collect()
            })
            .unwrap_or_default();

        // Get installed packs from database
        let installed_packs_list = PackRepository::list(&state.db).await?;
        let installed_packs: std::collections::HashMap<String, String> = installed_packs_list
            .into_iter()
            .map(|p| (p.r#ref, p.version))
            .collect();

        match validator
            .validate(&runtime_deps, &pack_deps, &installed_packs)
            .await
        {
            Ok(validation) => {
                if !validation.valid {
                    tracing::warn!("Pack dependency validation failed: {:?}", validation.errors);

                    // Return validation errors to user
                    return Err(ApiError::BadRequest(format!(
                        "Pack dependency validation failed:\n  - {}",
                        validation.errors.join("\n  - ")
                    )));
                }
                tracing::info!("All dependencies validated successfully");
            }
            Err(e) => {
                tracing::error!("Dependency validation error: {}", e);
                return Err(ApiError::InternalServerError(format!(
                    "Failed to validate dependencies: {}",
                    e
                )));
            }
        }
    } else {
        tracing::info!("Skipping dependency validation (disabled by user)");
    }

    // Read pack.yaml to get pack_ref so we can move to permanent storage first.
    // This ensures virtualenvs and dependencies are created at the final location
    // (Python venvs are NOT relocatable — they contain hardcoded paths).
    let pack_yaml_path_for_ref = installed.path.join("pack.yaml");
    let (pack_ref_for_storage, pack_version_for_storage) = {
        let content = std::fs::read_to_string(&pack_yaml_path_for_ref).map_err(|e| {
            ApiError::InternalServerError(format!("Failed to read pack.yaml: {}", e))
        })?;
        let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&content).map_err(|e| {
            ApiError::InternalServerError(format!("Failed to parse pack.yaml: {}", e))
        })?;
        let pack_ref = yaml
            .get("ref")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'ref' field in pack.yaml".to_string()))?
            .to_string();
        let version = yaml
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ApiError::BadRequest("Missing 'version' field in pack.yaml".to_string())
            })?
            .to_string();
        (pack_ref, version)
    };
    attune_common::schema::RefValidator::validate_pack_ref(&pack_ref_for_storage)
        .map_err(|error| ApiError::BadRequest(format!("Invalid pack ref: {error}")))?;
    validate_registry_manifest_identity(
        installed.registry_identity.as_ref(),
        &pack_ref_for_storage,
        &pack_version_for_storage,
    )?;
    let _install_guard = PACK_INSTALL_LOCK.lock().await;
    let storage = PackStorage::new(&state.config.packs_base_dir);
    let replacement = if request.force || request.skip_tests {
        storage
            .stage_pack(&installed.path, &pack_ref_for_storage, None)
            .map_err(|e| {
                ApiError::InternalServerError(format!("Failed to stage pack in storage: {}", e))
            })?
    } else {
        // Keep the candidate in its private staging directory until the worker
        // reports a passing result. The active path and database rows remain
        // untouched while this is running.
        let mut candidate_path = storage
            .stage_pack(&installed.path, &pack_ref_for_storage, None)
            .map_err(|e| {
                ApiError::InternalServerError(format!("Failed to stage pack candidate: {}", e))
            })?
            .into_staging_path();
        let candidate_yaml =
            std::fs::read_to_string(candidate_path.join("pack.yaml")).map_err(|error| {
                ApiError::InternalServerError(format!(
                    "Failed to read candidate pack.yaml: {error}"
                ))
            })?;
        let candidate_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&candidate_yaml)
            .map_err(|error| {
                ApiError::BadRequest(format!("Failed to parse candidate pack.yaml: {error}"))
            })?;
        let worker_selector = candidate_yaml
            .get("worker_selector")
            .and_then(|value| serde_json::to_value(value).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let worker_tolerations = candidate_yaml
            .get("worker_tolerations")
            .and_then(|value| serde_json::to_value(value).ok())
            .unwrap_or_else(|| serde_json::json!([]));
        let worker_affinity = candidate_yaml
            .get("worker_affinity")
            .and_then(|value| serde_json::to_value(value).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let existing_pack_id = PackRepository::find_by_ref(&state.db, &pack_ref_for_storage)
            .await?
            .map(|pack| pack.id);

        match dispatch_and_track_pack_tests(
            &state,
            existing_pack_id,
            &pack_ref_for_storage,
            &pack_version_for_storage,
            if existing_pack_id.is_some() {
                "update"
            } else {
                "install"
            },
            &candidate_path,
            Some(candidate_path.to_string_lossy().to_string()),
            worker_selector,
            worker_tolerations,
            worker_affinity,
        )
        .await
        {
            Some(Ok(install)) => {
                candidate_path = std::path::Path::new(&state.config.packs_base_dir)
                    .join(format!(".pack-test-{}", install.id));
                let terminal = wait_for_pack_test(&state, install.id).await?;
                if terminal.status != "succeeded" {
                    let _ = std::fs::remove_dir_all(&candidate_path);
                    return Err(ApiError::BadRequest(format!(
                        "Candidate pack tests failed: {}",
                        terminal
                            .error_message
                            .unwrap_or_else(|| "worker reported a failed test run".to_string())
                    )));
                }
                let replacement = storage
                    .stage_pack(&candidate_path, &pack_ref_for_storage, None)
                    .map_err(|e| {
                        ApiError::InternalServerError(format!(
                            "Failed to stage validated pack candidate: {}",
                            e
                        ))
                    })?;
                let _ = std::fs::remove_dir_all(&candidate_path);
                replacement
            }
            Some(Err(error)) => {
                let _ = std::fs::remove_dir_all(&candidate_path);
                return Err(error);
            }
            None => {
                let _ = std::fs::remove_dir_all(&candidate_path);
                storage
                    .stage_pack(&installed.path, &pack_ref_for_storage, None)
                    .map_err(|e| {
                        ApiError::InternalServerError(format!(
                            "Failed to stage pack in storage: {}",
                            e
                        ))
                    })?
            }
        }
    };
    let final_path = replacement.path().to_path_buf();

    tracing::info!("Pack moved to permanent storage: {:?}", final_path);

    let (checksum, checksum_subject) = if let Some(checksum) = installed.checksum.as_deref() {
        (
            Some(canonical_pack_checksum(checksum)?),
            installed.checksum_subject,
        )
    } else {
        let checksum = calculate_directory_checksum(&installed.path)
            .map_err(|e| {
                tracing::warn!("Failed to calculate checksum: {}", e);
                e
            })
            .ok()
            .map(|checksum| format!("sha256:{}", checksum));
        let subject = checksum
            .as_ref()
            .map(|_| attune_common::pack_registry::ChecksumSubject::DirectoryContent);
        (checksum, subject)
    };
    let checksum_verified = installed.checksum_verified;
    let fallback_occurred = registry_resolution
        .as_ref()
        .is_some_and(|resolution| !resolution.matches(&installed.source));
    let provenance = build_pack_install_provenance(
        &installed,
        &registry_summaries,
        checksum.clone(),
        checksum_subject,
        checksum_verified,
        fallback_occurred,
    );
    let (source_url, source_ref) = concrete_source_metadata(&installed.source);
    let installation_metadata = PackInstallationMetadata {
        source_type: get_source_type(&installed.source).to_string(),
        source_url,
        source_ref,
        checksum: checksum.clone(),
        checksum_verified,
        installed_by: user_id,
        storage_path: final_path.to_string_lossy().to_string(),
        provenance: provenance.clone(),
    };

    // Register the pack in database from the permanent storage location.
    let registered_pack = register_pack_internal(
        state.clone(),
        &user,
        installed.path.to_string_lossy().to_string(),
        request.force,
        true,
        Some(installation_metadata),
        Some(replacement),
    )
    .await?;

    // Fetch the registered pack
    let pack = PackRepository::find_by_id(&state.db, registered_pack.id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Pack with ID {} not found", registered_pack.id))
        })?;

    // Clean up temp directory
    match installer.cleanup(&installed.path).await {
        Ok(()) => temporary_install_cleanup.disarm(),
        Err(error) => tracing::warn!("Failed to clean up temporary pack install: {}", error),
    }

    emit_pack_audit(
        &state,
        &user,
        event_type::pack::INSTALLED,
        &pack,
        serde_json::json!({
            "version": pack.version.as_str(),
            "force": request.force,
            "skip_tests": request.skip_tests,
            "provenance": provenance,
        }),
    );

    let response = PackInstallResponse {
        pack: PackResponse::from(pack),
        test_result: None, // Available via GET /packs/{ref}/install/latest while pending
        tests_skipped: request.skip_tests,
        install_id: registered_pack
            .test_install
            .as_ref()
            .map(|install| install.id),
        install_status: registered_pack.test_install.map(|install| install.status),
        provenance: Some(provenance),
    };

    Ok((StatusCode::OK, Json(crate::dto::ApiResponse::new(response))))
}

fn direct_pack_registry_config(
    configured: &attune_common::config::PackRegistryConfig,
) -> attune_common::config::PackRegistryConfig {
    let mut config = configured.clone();
    config.indices.clear();
    if !config.enabled {
        config.approved_public_hosts.clear();
        config.approved_private_hosts.clear();
        config.approved_private_cidrs.clear();
        config.allow_http = false;
    }
    config
}

fn detect_pack_source(
    source: &str,
    ref_spec: Option<&str>,
    allow_registry: bool,
) -> Result<attune_common::pack_registry::PackSource, ApiError> {
    use attune_common::pack_registry::PackSource;
    use std::path::Path;

    // Check if it's a URL
    if source.starts_with("http://") || source.starts_with("https://") {
        let url = attune_common::pack_registry::validate_remote_pack_url(source)
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        if url.path().ends_with(".git") || ref_spec.is_some() {
            return Ok(PackSource::Git {
                url: url.to_string(),
                git_ref: ref_spec.map(String::from),
            });
        }
        return Ok(PackSource::Archive {
            url: url.to_string(),
        });
    }

    // Git subprocess transports other than validated HTTP(S) are never allowed.
    // A registry pack may legitimately be named `git`, making `git@0.2.0`
    // a valid pack@version reference. Only reject the SCP-style Git syntax
    // when the `git@` form also contains a host/path separator.
    if (source.starts_with("git@") && source.contains(':')) || source.contains("git://") {
        return Err(ApiError::BadRequest(
            "Remote Git pack sources must use an approved HTTPS URL".to_string(),
        ));
    }

    // Check if it's a local path
    let path = Path::new(source);
    if path.exists() {
        if path.is_file() {
            return Ok(PackSource::LocalArchive {
                path: path.to_path_buf(),
            });
        }
        return Ok(PackSource::LocalDirectory {
            path: path.to_path_buf(),
        });
    }

    if source.contains("://") {
        return Err(ApiError::BadRequest(
            "Unsupported remote pack source scheme".to_string(),
        ));
    }

    if !allow_registry {
        return Err(ApiError::BadRequest(
            "Source is not an explicit remote URL or existing local path".to_string(),
        ));
    }

    // Otherwise assume it's a registry reference
    // Parse version if present (format: "pack@version" or "pack")
    let (pack_ref, version) = if let Some(at_pos) = source.find('@') {
        let (pack, ver) = source.split_at(at_pos);
        (pack.to_string(), Some(ver[1..].to_string()))
    } else {
        (source.to_string(), None)
    };

    Ok(PackSource::Registry { pack_ref, version })
}

fn validate_registry_manifest_identity(
    identity: Option<&attune_common::pack_registry::RegistryPackIdentity>,
    manifest_ref: &str,
    manifest_version: &str,
) -> ApiResult<()> {
    if let Some(identity) = identity {
        if identity.pack_ref != manifest_ref || identity.version != manifest_version {
            return Err(ApiError::BadRequest(format!(
                "Registry entry {}@{} does not match downloaded manifest {}@{}",
                identity.pack_ref, identity.version, manifest_ref, manifest_version
            )));
        }
    }
    Ok(())
}

struct RegistryResolution {
    registry_url: String,
    entry: attune_common::pack_registry::PackIndexEntry,
    preferred_source: attune_common::pack_registry::InstallSource,
}

impl RegistryResolution {
    fn matches(&self, installed: &attune_common::pack_registry::PackSource) -> bool {
        use attune_common::pack_registry::{InstallSource, PackSource};
        match (&self.preferred_source, installed) {
            (
                InstallSource::Git {
                    url: expected_url,
                    git_ref: expected_ref,
                    ..
                },
                PackSource::Git { url, git_ref },
            ) => equivalent_remote_pack_urls(expected_url, url) && expected_ref == git_ref,
            (
                InstallSource::Archive {
                    url: expected_url, ..
                },
                PackSource::Archive { url },
            ) => equivalent_remote_pack_urls(expected_url, url),
            _ => false,
        }
    }
}

fn equivalent_remote_pack_urls(left: &str, right: &str) -> bool {
    let canonical = |value: &str| {
        attune_common::pack_registry::validate_remote_pack_url(value)
            .ok()
            .map(|url| registry_url_key(url.as_str()))
    };
    canonical(left)
        .zip(canonical(right))
        .is_some_and(|(left, right)| left == right)
}

async fn resolve_registry_request(
    config: &attune_common::config::PackRegistryConfig,
    source: &attune_common::pack_registry::PackSource,
) -> ApiResult<RegistryResolution> {
    use attune_common::pack_registry::{InstallSource, PackSource, RegistryClient};
    let PackSource::Registry { pack_ref, version } = source else {
        return Err(ApiError::BadRequest(
            "Registry resolution requires a registry pack reference".to_string(),
        ));
    };
    let client = RegistryClient::new(config.clone())
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let (entry, registry_url) = client.search_pack(pack_ref).await?.ok_or_else(|| {
        ApiError::NotFound(format!("Pack '{}' was not found in a registry", pack_ref))
    })?;
    if let Some(requested_version) = version {
        if requested_version != "latest" && requested_version != &entry.version {
            return Err(ApiError::BadRequest(format!(
                "Pack {} version {} not found (available: {})",
                pack_ref, requested_version, entry.version
            )));
        }
    }
    let preferred_source = entry
        .install_sources
        .iter()
        .find(|source| matches!(source, InstallSource::Git { .. }))
        .or_else(|| {
            entry
                .install_sources
                .iter()
                .find(|source| matches!(source, InstallSource::Archive { .. }))
        })
        .cloned()
        .ok_or_else(|| {
            ApiError::BadRequest(format!("Pack {} has no install sources", entry.pack_ref))
        })?;
    Ok(RegistryResolution {
        registry_url,
        entry,
        preferred_source,
    })
}

fn canonical_pack_checksum(checksum: &str) -> ApiResult<String> {
    if let Ok(parsed) = attune_common::pack_registry::Checksum::parse(checksum) {
        return Ok(parsed.to_string());
    }
    if checksum.len() == 64
        && checksum
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Ok(format!("sha256:{}", checksum.to_ascii_lowercase()));
    }
    Err(ApiError::InternalServerError(
        "Pack installer returned a checksum in a non-canonical format".to_string(),
    ))
}

fn build_pack_install_provenance(
    installed: &attune_common::pack_registry::InstalledPack,
    registry_summaries: &[PackRegistryIndexSummary],
    checksum: Option<String>,
    checksum_subject: Option<attune_common::pack_registry::ChecksumSubject>,
    checksum_verified: bool,
    fallback_occurred: bool,
) -> PackInstallProvenance {
    let (artifact_url, git_ref) = concrete_source_metadata(&installed.source);
    let registry_url = installed
        .registry_identity
        .as_ref()
        .map(|identity| identity.registry_url.clone());
    let registry_id = registry_url.as_ref().and_then(|url| {
        registry_summaries
            .iter()
            .find(|summary| registry_url_key(&summary.url) == registry_url_key(url))
            .and_then(|summary| summary.id)
    });
    let resolved_pack = installed
        .registry_identity
        .as_ref()
        .map(|identity| format!("{}@{}", identity.pack_ref, identity.version));
    PackInstallProvenance {
        artifact_type: get_source_type(&installed.source).to_string(),
        artifact_url,
        git_ref,
        registry_id,
        registry_url,
        resolved_pack,
        checksum,
        checksum_subject,
        checksum_verified,
        fallback_occurred,
    }
}

fn merge_installation_provenance(
    existing: &serde_json::Value,
    provenance: &PackInstallProvenance,
) -> serde_json::Value {
    let mut installers = existing.as_object().cloned().unwrap_or_default();
    installers.insert(
        "installation_provenance".to_string(),
        serde_json::to_value(provenance).expect("pack provenance must serialize"),
    );
    serde_json::Value::Object(installers)
}

/// Get source type string from PackSource
fn get_source_type(source: &attune_common::pack_registry::PackSource) -> &'static str {
    use attune_common::pack_registry::PackSource;
    match source {
        PackSource::Git { .. } => "git",
        PackSource::Archive { .. } => "archive",
        PackSource::LocalDirectory { .. } => "local_directory",
        PackSource::LocalArchive { .. } => "local_archive",
        PackSource::Registry { .. } => "registry",
    }
}

/// Extract the concrete artifact URL/path and Git ref from PackSource.
fn concrete_source_metadata(
    source: &attune_common::pack_registry::PackSource,
) -> (Option<String>, Option<String>) {
    use attune_common::pack_registry::PackSource;
    match source {
        PackSource::Git { url, git_ref } => (Some(url.clone()), git_ref.clone()),
        PackSource::Archive { url } => (Some(url.clone()), None),
        PackSource::LocalDirectory { path } => (Some(path.to_string_lossy().to_string()), None),
        PackSource::LocalArchive { path } => (Some(path.to_string_lossy().to_string()), None),
        PackSource::Registry { .. } => (None, None),
    }
}

/// Sync workflows from filesystem to database for a pack
#[utoipa::path(
    post,
    path = "/api/v1/packs/{ref}/workflows/sync",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Pack reference identifier")
    ),
    responses(
        (status = 200, description = "Workflows synced successfully", body = inline(ApiResponse<PackWorkflowSyncResponse>)),
        (status = 404, description = "Pack not found"),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn sync_pack_workflows(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Get packs base directory from config
    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);

    // Create workflow service
    let service_config = PackWorkflowServiceConfig {
        packs_base_dir,
        skip_validation_errors: false,
        update_existing: true,
        max_file_size: 1024 * 1024, // 1MB
    };

    let service = PackWorkflowService::new(state.db.clone(), service_config);

    // Sync workflows
    let result = service.sync_pack_workflows(&pack_ref).await?;

    // Convert to response DTO
    let response = PackWorkflowSyncResponse {
        pack_ref: result.pack_ref,
        loaded_count: result.loaded_count,
        registered_count: result.registered_count,
        workflows: result
            .workflows
            .into_iter()
            .map(|w| WorkflowSyncResult {
                ref_name: w.ref_name,
                created: w.created,
                workflow_def_id: w.workflow_def_id,
                warnings: w.warnings,
            })
            .collect(),
        errors: result.errors,
    };

    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            response,
            "Pack workflows synced successfully",
        )),
    ))
}

/// Validate workflows for a pack without syncing
#[utoipa::path(
    post,
    path = "/api/v1/packs/{ref}/workflows/validate",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Pack reference identifier")
    ),
    responses(
        (status = 200, description = "Workflows validated", body = inline(ApiResponse<PackWorkflowValidationResponse>)),
        (status = 404, description = "Pack not found"),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn validate_pack_workflows(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Get packs base directory from config
    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);

    // Create workflow service
    let service_config = PackWorkflowServiceConfig {
        packs_base_dir,
        skip_validation_errors: false,
        update_existing: false,
        max_file_size: 1024 * 1024, // 1MB
    };

    let service = PackWorkflowService::new(state.db.clone(), service_config);

    // Validate workflows
    let result = service.validate_pack_workflows(&pack_ref).await?;

    // Convert to response DTO
    let response = PackWorkflowValidationResponse {
        pack_ref: result.pack_ref,
        validated_count: result.validated_count,
        error_count: result.error_count,
        errors: result.errors,
    };

    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            response,
            "Pack workflows validated",
        )),
    ))
}

/// Execute tests for a pack
#[utoipa::path(
    post,
    path = "/api/v1/packs/{ref}/test",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Pack reference identifier")
    ),
    responses(
        (status = 200, description = "Tests executed successfully", body = inline(ApiResponse<PackTestResult>)),
        (status = 404, description = "Pack not found"),
        (status = 500, description = "Test execution failed")
    ),
    security(("bearer_auth" = []))
)]
pub async fn test_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Get pack from database
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    let pack_dir = PathBuf::from(&state.config.packs_base_dir).join(&pack_ref);
    if !pack_dir.exists() {
        return Err(ApiError::NotFound(format!(
            "Pack directory not found: {}",
            pack_dir.display()
        )));
    }

    // Dispatch the test run to a worker and track it as a pack install.
    let install = match dispatch_and_track_pack_tests(
        &state,
        Some(pack.id),
        &pack_ref,
        &pack.version,
        "manual",
        &pack_dir,
        None,
        pack.worker_selector.clone(),
        pack.worker_tolerations.clone(),
        pack.worker_affinity.clone(),
    )
    .await
    {
        Some(Ok(install)) => install,
        Some(Err(e)) => return Err(e),
        None => {
            return Err(ApiError::BadRequest(
                "No enabled testing configuration found in pack.yaml".to_string(),
            ))
        }
    };

    tracing::info!(
        "Pack tests for '{}' dispatched to worker (install {})",
        pack_ref,
        install.id
    );

    let finalize_state = state.clone();
    let finalize_ref = pack_ref.clone();
    tokio::spawn(async move {
        finalize_pack_install(
            finalize_state,
            install.id,
            finalize_ref,
            pack.id,
            false,
            false,
            false,
            false,
        )
        .await;
    });

    let response = PackInstallResponse {
        pack: PackResponse::from(pack),
        test_result: None,
        tests_skipped: false,
        install_id: Some(install.id),
        install_status: Some(install.status),
        provenance: None,
    };

    Ok((
        StatusCode::ACCEPTED,
        Json(crate::dto::ApiResponse::new(response)),
    ))
}

/// Get the most recent install status for a pack (survives a rollback).
#[utoipa::path(
    get,
    path = "/api/v1/packs/{ref}/install/latest",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Pack reference identifier")
    ),
    responses(
        (status = 200, description = "Latest pack install status", body = ApiResponse<PackInstallStatusResponse>),
        (status = 404, description = "No install records found for pack")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_pack_latest_install(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let record = PackInstallRepository::new(state.db.clone())
        .find_latest_by_pack_ref(&pack_ref)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("No install records found for pack '{}'", pack_ref))
        })?;

    Ok(Json(ApiResponse::new(PackInstallStatusResponse::from(
        record,
    ))))
}

/// Get the status of a specific pack install record.
#[utoipa::path(
    get,
    path = "/api/v1/packs/install/{id}",
    tag = "packs",
    params(
        ("id" = i64, Path, description = "Pack install record id")
    ),
    responses(
        (status = 200, description = "Pack install status", body = ApiResponse<PackInstallStatusResponse>),
        (status = 404, description = "Install record not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_pack_install(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let record = PackInstallRepository::new(state.db.clone())
        .find_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack install record {} not found", id)))?;

    Ok(Json(ApiResponse::new(PackInstallStatusResponse::from(
        record,
    ))))
}

/// Get test history for a pack
#[utoipa::path(
    get,
    path = "/api/v1/packs/{ref}/tests",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Pack reference identifier"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "Test history retrieved", body = inline(PaginatedResponse<attune_common::models::PackTestExecution>)),
        (status = 404, description = "Pack not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_pack_test_history(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    // Get pack from database
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    // Get test executions
    let pack_test_repo = PackTestRepository::new(state.db.clone());
    let test_executions = pack_test_repo
        .list_by_pack(
            pack.id,
            pagination.limit() as i64,
            (pagination.page.saturating_sub(1) * pagination.limit()) as i64,
        )
        .await?;

    // Get total count
    let total = pack_test_repo.count_by_pack(pack.id).await?;

    let response = PaginatedResponse::<attune_common::models::PackTestExecution>::new(
        test_executions,
        &pagination,
        total as u64,
    );

    Ok((StatusCode::OK, Json(response)))
}

/// Get latest test result for a pack
#[utoipa::path(
    get,
    path = "/api/v1/packs/{ref}/tests/latest",
    tag = "packs",
    params(
        ("ref" = String, Path, description = "Pack reference identifier")
    ),
    responses(
        (status = 200, description = "Latest test result retrieved"),
        (status = 404, description = "Pack not found or no tests available")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_pack_latest_test(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Get pack from database
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    // Get latest test execution
    let pack_test_repo = PackTestRepository::new(state.db.clone());
    let test_execution = pack_test_repo
        .get_latest_by_pack(pack.id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("No test results found for pack '{}'", pack_ref))
        })?;

    let response = ApiResponse::new(test_execution);

    Ok((StatusCode::OK, Json(response)))
}

/// Get a single pack test execution by ID
#[utoipa::path(
    get,
    path = "/api/v1/packs/tests/{id}",
    tag = "packs",
    params(
        ("id" = i64, Path, description = "Pack test execution id")
    ),
    responses(
        (status = 200, description = "Test execution retrieved"),
        (status = 404, description = "Test execution not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_pack_test(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let test_execution = PackTestRepository::new(state.db.clone())
        .find_by_id(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack test execution {} not found", id)))?;

    let response = ApiResponse::new(test_execution);

    Ok((StatusCode::OK, Json(response)))
}

/// Create pack routes
///
/// Note: Nested resource routes (e.g., /packs/:ref/actions) are defined
/// in their respective modules (actions.rs, triggers.rs, rules.rs) to avoid
/// route conflicts and maintain proper separation of concerns.
/// Download packs from various sources
#[utoipa::path(
    post,
    path = "/api/v1/packs/download",
    tag = "packs",
    request_body = DownloadPacksRequest,
    responses(
        (status = 200, description = "Packs downloaded", body = ApiResponse<DownloadPacksResponse>),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn download_packs(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<DownloadPacksRequest>,
) -> ApiResult<Json<ApiResponse<DownloadPacksResponse>>> {
    use attune_common::pack_registry::PackInstaller;

    authorize_pack_registry_action(&state, &user, Action::Install).await?;

    // Create temp directory
    let temp_dir = std::env::temp_dir().join("attune-pack-downloads");
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| ApiError::InternalServerError(format!("Failed to create temp dir: {}", e)))?;

    // This staged endpoint accepts direct sources only and must not depend on
    // unrelated managed-index credentials or availability.
    let registry_config = direct_pack_registry_config(&state.config.pack_registry);
    let installer = PackInstaller::new(&temp_dir, Some(registry_config))
        .await
        .map_err(|e| ApiError::InternalServerError(format!("Failed to create installer: {}", e)))?;

    let mut downloaded = Vec::new();
    let mut failed = Vec::new();

    for source in &request.packs {
        let pack_source = detect_pack_source(source, request.ref_spec.as_deref(), true)?;
        if matches!(
            pack_source,
            attune_common::pack_registry::PackSource::Registry { .. }
        ) {
            failed.push(crate::dto::pack::FailedPack {
                source: source.clone(),
                error: "Registry references must use /api/v1/packs/install so identity, authorization, and verified provenance remain bound to registration".to_string(),
            });
            continue;
        }
        let source_type_str = get_source_type(&pack_source).to_string();

        match installer.install(pack_source).await {
            Ok(installed) => {
                // Read pack.yaml
                let pack_yaml_path = installed.path.join("pack.yaml");
                if let Ok(content) = std::fs::read_to_string(&pack_yaml_path) {
                    if let Ok(yaml) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&content) {
                        let pack_ref = yaml
                            .get("ref")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let pack_version = yaml
                            .get("version")
                            .and_then(|v| v.as_str())
                            .unwrap_or("0.0.0")
                            .to_string();

                        downloaded.push(crate::dto::pack::DownloadedPack {
                            source: source.clone(),
                            source_type: source_type_str.clone(),
                            pack_path: installed.path.to_string_lossy().to_string(),
                            pack_ref,
                            pack_version,
                            git_commit: None,
                            checksum: installed.checksum,
                        });
                    }
                }
            }
            Err(e) => {
                failed.push(crate::dto::pack::FailedPack {
                    source: source.clone(),
                    error: e.to_string(),
                });
            }
        }
    }

    let response = DownloadPacksResponse {
        success_count: downloaded.len(),
        failure_count: failed.len(),
        total_count: request.packs.len(),
        downloaded_packs: downloaded,
        failed_packs: failed,
    };

    Ok(Json(ApiResponse::new(response)))
}

/// Get pack dependencies
#[utoipa::path(
    post,
    path = "/api/v1/packs/dependencies",
    tag = "packs",
    request_body = GetPackDependenciesRequest,
    responses(
        (status = 200, description = "Dependencies analyzed", body = ApiResponse<GetPackDependenciesResponse>),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_pack_dependencies(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Json(request): Json<GetPackDependenciesRequest>,
) -> ApiResult<Json<ApiResponse<GetPackDependenciesResponse>>> {
    use attune_common::repositories::List;

    let mut dependencies = Vec::new();
    let mut runtime_requirements = std::collections::HashMap::new();
    let mut analyzed_packs = Vec::new();
    let mut errors = Vec::new();

    // Get installed packs
    let installed_packs_list = PackRepository::list(&state.db).await?;
    let installed_refs: std::collections::HashSet<String> =
        installed_packs_list.into_iter().map(|p| p.r#ref).collect();

    for pack_path in &request.pack_paths {
        let pack_yaml_path = std::path::Path::new(pack_path).join("pack.yaml");

        if !pack_yaml_path.exists() {
            errors.push(crate::dto::pack::DependencyError {
                pack_path: pack_path.clone(),
                error: "pack.yaml not found".to_string(),
            });
            continue;
        }

        let content = match std::fs::read_to_string(&pack_yaml_path) {
            Ok(c) => c,
            Err(e) => {
                errors.push(crate::dto::pack::DependencyError {
                    pack_path: pack_path.clone(),
                    error: format!("Failed to read pack.yaml: {}", e),
                });
                continue;
            }
        };

        let yaml: serde_yaml_ng::Value = match serde_yaml_ng::from_str(&content) {
            Ok(y) => y,
            Err(e) => {
                errors.push(crate::dto::pack::DependencyError {
                    pack_path: pack_path.clone(),
                    error: format!("Failed to parse pack.yaml: {}", e),
                });
                continue;
            }
        };

        let pack_ref = yaml
            .get("ref")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Extract dependencies
        let mut dep_count = 0;
        if let Some(deps) = yaml.get("dependencies").and_then(|d| d.as_sequence()) {
            for dep in deps {
                if let Some(dep_str) = dep.as_str() {
                    let parts: Vec<&str> = dep_str.splitn(2, '@').collect();
                    let dep_ref = parts[0].to_string();
                    let version_spec = parts.get(1).unwrap_or(&"*").to_string();
                    let already_installed = installed_refs.contains(&dep_ref);

                    dependencies.push(crate::dto::pack::PackDependency {
                        pack_ref: dep_ref.clone(),
                        version_spec: version_spec.clone(),
                        required_by: pack_ref.clone(),
                        already_installed,
                    });
                    dep_count += 1;
                }
            }
        }

        // Extract runtime requirements
        let mut runtime_req = crate::dto::pack::RuntimeRequirements {
            pack_ref: pack_ref.clone(),
            python: None,
            nodejs: None,
        };

        if let Some(python_ver) = yaml.get("python").and_then(|v| v.as_str()) {
            let req_file = std::path::Path::new(pack_path).join("requirements.txt");
            runtime_req.python = Some(crate::dto::pack::PythonRequirements {
                version: Some(python_ver.to_string()),
                requirements_file: if req_file.exists() {
                    Some(req_file.to_string_lossy().to_string())
                } else {
                    None
                },
            });
        }

        if let Some(nodejs_ver) = yaml.get("nodejs").and_then(|v| v.as_str()) {
            let pkg_file = std::path::Path::new(pack_path).join("package.json");
            runtime_req.nodejs = Some(crate::dto::pack::NodeJsRequirements {
                version: Some(nodejs_ver.to_string()),
                package_file: if pkg_file.exists() {
                    Some(pkg_file.to_string_lossy().to_string())
                } else {
                    None
                },
            });
        }

        if runtime_req.python.is_some() || runtime_req.nodejs.is_some() {
            runtime_requirements.insert(pack_ref.clone(), runtime_req);
        }

        analyzed_packs.push(crate::dto::pack::AnalyzedPack {
            pack_ref: pack_ref.clone(),
            pack_path: pack_path.clone(),
            has_dependencies: dep_count > 0,
            dependency_count: dep_count,
        });
    }

    let missing_dependencies: Vec<_> = dependencies
        .iter()
        .filter(|d| !d.already_installed)
        .cloned()
        .collect();

    let response = GetPackDependenciesResponse {
        dependencies,
        runtime_requirements,
        missing_dependencies,
        analyzed_packs,
        errors,
    };

    Ok(Json(ApiResponse::new(response)))
}

/// Build pack environments
#[utoipa::path(
    post,
    path = "/api/v1/packs/build-envs",
    tag = "packs",
    request_body = BuildPackEnvsRequest,
    responses(
        (status = 200, description = "Environments built", body = ApiResponse<BuildPackEnvsResponse>),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn build_pack_envs(
    State(_state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Json(request): Json<BuildPackEnvsRequest>,
) -> ApiResult<Json<ApiResponse<BuildPackEnvsResponse>>> {
    use std::path::Path;
    use std::process::Command;

    let start = std::time::Instant::now();
    let mut built_environments = Vec::new();
    let mut failed_environments = Vec::new();
    let mut python_envs_built = 0;
    let mut nodejs_envs_built = 0;

    for pack_path in &request.pack_paths {
        let pack_path_obj = Path::new(pack_path);
        let pack_start = std::time::Instant::now();

        // Read pack.yaml to get pack_ref and runtime requirements
        let pack_yaml_path = pack_path_obj.join("pack.yaml");
        if !pack_yaml_path.exists() {
            failed_environments.push(crate::dto::pack::FailedEnvironment {
                pack_ref: "unknown".to_string(),
                pack_path: pack_path.clone(),
                runtime: "unknown".to_string(),
                error: "pack.yaml not found".to_string(),
            });
            continue;
        }

        let content = match std::fs::read_to_string(&pack_yaml_path) {
            Ok(c) => c,
            Err(e) => {
                failed_environments.push(crate::dto::pack::FailedEnvironment {
                    pack_ref: "unknown".to_string(),
                    pack_path: pack_path.clone(),
                    runtime: "unknown".to_string(),
                    error: format!("Failed to read pack.yaml: {}", e),
                });
                continue;
            }
        };

        let yaml: serde_yaml_ng::Value = match serde_yaml_ng::from_str(&content) {
            Ok(y) => y,
            Err(e) => {
                failed_environments.push(crate::dto::pack::FailedEnvironment {
                    pack_ref: "unknown".to_string(),
                    pack_path: pack_path.clone(),
                    runtime: "unknown".to_string(),
                    error: format!("Failed to parse pack.yaml: {}", e),
                });
                continue;
            }
        };

        let pack_ref = yaml
            .get("ref")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let mut python_env = None;
        let mut nodejs_env = None;
        let mut has_error = false;

        // Check for Python environment
        if !request.skip_python {
            if let Some(_python_ver) = yaml.get("python").and_then(|v| v.as_str()) {
                let requirements_file = pack_path_obj.join("requirements.txt");

                if requirements_file.exists() {
                    // Check if Python is available
                    match Command::new("python3").arg("--version").output() {
                        Ok(output) if output.status.success() => {
                            let version_str = String::from_utf8_lossy(&output.stdout);
                            let venv_path = pack_path_obj.join("venv");

                            // Check if venv exists or if force_rebuild is set
                            if !venv_path.exists() || request.force_rebuild {
                                tracing::info!(
                                    pack_ref = %pack_ref,
                                    "Python environment would be built here in production"
                                );
                            }

                            // Report environment status (detection mode)
                            python_env = Some(crate::dto::pack::PythonEnvironment {
                                virtualenv_path: venv_path.to_string_lossy().to_string(),
                                requirements_installed: venv_path.exists(),
                                package_count: 0, // Would count from pip freeze in production
                                python_version: version_str.trim().to_string(),
                            });
                            python_envs_built += 1;
                        }
                        _ => {
                            failed_environments.push(crate::dto::pack::FailedEnvironment {
                                pack_ref: pack_ref.clone(),
                                pack_path: pack_path.clone(),
                                runtime: "python".to_string(),
                                error: "Python 3 not available in system".to_string(),
                            });
                            has_error = true;
                        }
                    }
                }
            }
        }

        // Check for Node.js environment
        if !has_error && !request.skip_nodejs {
            if let Some(_nodejs_ver) = yaml.get("nodejs").and_then(|v| v.as_str()) {
                let package_file = pack_path_obj.join("package.json");

                if package_file.exists() {
                    // Check if Node.js is available
                    match Command::new("node").arg("--version").output() {
                        Ok(output) if output.status.success() => {
                            let version_str = String::from_utf8_lossy(&output.stdout);
                            let node_modules = pack_path_obj.join("node_modules");

                            // Check if node_modules exists or if force_rebuild is set
                            if !node_modules.exists() || request.force_rebuild {
                                tracing::info!(
                                    pack_ref = %pack_ref,
                                    "Node.js environment would be built here in production"
                                );
                            }

                            // Report environment status (detection mode)
                            nodejs_env = Some(crate::dto::pack::NodeJsEnvironment {
                                node_modules_path: node_modules.to_string_lossy().to_string(),
                                dependencies_installed: node_modules.exists(),
                                package_count: 0, // Would count from package.json in production
                                nodejs_version: version_str.trim().to_string(),
                            });
                            nodejs_envs_built += 1;
                        }
                        _ => {
                            failed_environments.push(crate::dto::pack::FailedEnvironment {
                                pack_ref: pack_ref.clone(),
                                pack_path: pack_path.clone(),
                                runtime: "nodejs".to_string(),
                                error: "Node.js not available in system".to_string(),
                            });
                            has_error = true;
                        }
                    }
                }
            }
        }

        if !has_error && (python_env.is_some() || nodejs_env.is_some()) {
            built_environments.push(crate::dto::pack::BuiltEnvironment {
                pack_ref,
                pack_path: pack_path.clone(),
                environments: crate::dto::pack::Environments {
                    python: python_env,
                    nodejs: nodejs_env,
                },
                duration_ms: pack_start.elapsed().as_millis() as u64,
            });
        }
    }

    let success_count = built_environments.len();
    let failure_count = failed_environments.len();

    let response = BuildPackEnvsResponse {
        built_environments,
        failed_environments,
        summary: crate::dto::pack::BuildSummary {
            total_packs: request.pack_paths.len(),
            success_count,
            failure_count,
            python_envs_built,
            nodejs_envs_built,
            total_duration_ms: start.elapsed().as_millis() as u64,
        },
    };

    Ok(Json(ApiResponse::new(response)))
}

/// Register multiple packs
#[utoipa::path(
    post,
    path = "/api/v1/packs/register-batch",
    tag = "packs",
    request_body = RegisterPacksRequest,
    responses(
        (status = 200, description = "Packs registered", body = ApiResponse<RegisterPacksResponse>),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn register_packs_batch(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<RegisterPacksRequest>,
) -> ApiResult<Json<ApiResponse<RegisterPacksResponse>>> {
    authorize_pack_registry_action(&state, &user, Action::Install).await?;

    let start = std::time::Instant::now();
    let mut registered = Vec::new();
    let mut failed = Vec::new();
    let total_components = 0;
    let _install_guard = PACK_INSTALL_LOCK.lock().await;

    for pack_path in &request.pack_paths {
        // Call the existing register_pack_internal function
        let register_req = crate::dto::pack::RegisterPackRequest {
            path: pack_path.clone(),
            force: request.force,
            skip_tests: request.skip_tests,
        };

        match register_pack_internal(
            state.clone(),
            &user,
            register_req.path.clone(),
            register_req.force,
            register_req.skip_tests,
            None,
            None,
        )
        .await
        {
            Ok(registered_pack) => {
                let pack_id = registered_pack.id;
                // Fetch pack details
                if let Ok(Some(pack)) = PackRepository::find_by_id(&state.db, pack_id).await {
                    // Count components (simplified)
                    registered.push(crate::dto::pack::RegisteredPack {
                        pack_ref: pack.r#ref.clone(),
                        pack_id,
                        pack_version: pack.version.clone(),
                        storage_path: format!("{}/{}", state.config.packs_base_dir, pack.r#ref),
                        components_registered: crate::dto::pack::ComponentCounts {
                            actions: 0,
                            sensors: 0,
                            triggers: 0,
                            rules: 0,
                            workflows: 0,
                            policies: 0,
                        },
                        test_result: None,
                        validation_results: crate::dto::pack::ValidationResults {
                            valid: true,
                            errors: Vec::new(),
                        },
                    });
                }
            }
            Err(e) => {
                failed.push(crate::dto::pack::FailedPackRegistration {
                    pack_ref: "unknown".to_string(),
                    pack_path: pack_path.clone(),
                    error: e.to_string(),
                    error_stage: "registration".to_string(),
                });
            }
        }
    }

    let response = RegisterPacksResponse {
        registered_packs: registered.clone(),
        failed_packs: failed.clone(),
        summary: crate::dto::pack::RegistrationSummary {
            total_packs: request.pack_paths.len(),
            success_count: registered.len(),
            failure_count: failed.len(),
            total_components,
            duration_ms: start.elapsed().as_millis() as u64,
        },
    };

    Ok(Json(ApiResponse::new(response)))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/packs", get(list_packs).post(create_pack))
        .route("/packs/register", axum::routing::post(register_pack))
        .route(
            "/packs/register-batch",
            axum::routing::post(register_packs_batch),
        )
        .route("/packs/install", axum::routing::post(install_pack))
        .route(
            "/packs/upload",
            axum::routing::post(upload_pack).layer(DefaultBodyLimit::max(PACK_UPLOAD_MAX_BYTES)),
        )
        .route("/packs/download", axum::routing::post(download_packs))
        .route(
            "/pack-indices",
            get(list_pack_indices).post(create_pack_index),
        )
        .route("/pack-indices/packs", get(browse_indexed_packs))
        .route("/pack-indices/packs/{ref}", get(get_indexed_pack))
        .route(
            "/pack-indices/{id}",
            axum::routing::put(update_pack_index).delete(delete_pack_index),
        )
        .route(
            "/packs/dependencies",
            axum::routing::post(get_pack_dependencies),
        )
        .route("/packs/build-envs", axum::routing::post(build_pack_envs))
        .route("/packs/{ref}/icon", get(get_pack_icon))
        .route(
            "/packs/{ref}",
            get(get_pack).put(update_pack).delete(delete_pack),
        )
        .route(
            "/packs/{ref}/workflows/sync",
            axum::routing::post(sync_pack_workflows),
        )
        .route(
            "/packs/{ref}/workflows/validate",
            axum::routing::post(validate_pack_workflows),
        )
        .route("/packs/{ref}/test", axum::routing::post(test_pack))
        .route("/packs/{ref}/tests", get(get_pack_test_history))
        .route("/packs/{ref}/tests/latest", get(get_pack_latest_test))
        .route("/packs/tests/{id}", get(get_pack_test))
        .route("/packs/{ref}/install/latest", get(get_pack_latest_install))
        .route("/packs/install/{id}", get(get_pack_install))
}

fn is_valid_pack_ref_path_segment(pack_ref: &str) -> bool {
    attune_common::schema::RefValidator::validate_pack_ref(pack_ref).is_ok()
}

async fn find_pack_icon(
    packs_base_dir: &FsPath,
    pack_ref: &str,
) -> Option<(PathBuf, &'static str)> {
    const ICON_FILES: [(&str, &str); 5] = [
        ("pack-icon.svg", "image/svg+xml"),
        ("pack-icon.png", "image/png"),
        ("pack-icon.jpg", "image/jpeg"),
        ("pack-icon.jpeg", "image/jpeg"),
        ("pack-icon.ico", "image/x-icon"),
    ];

    let pack_dir = packs_base_dir.join(pack_ref);
    if !matches!(tokio::fs::symlink_metadata(&pack_dir).await, Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        return None;
    }
    for (file_name, content_type) in ICON_FILES {
        let path = pack_dir.join(file_name);
        if matches!(tokio::fs::symlink_metadata(&path).await, Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink())
        {
            return Some((path, content_type));
        }
    }

    None
}

fn pack_authorization_context(identity_id: i64, pack: &Pack) -> AuthorizationContext {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_id = Some(pack.id);
    ctx.target_ref = Some(pack.r#ref.clone());
    ctx.pack_ref = Some(pack.r#ref.clone());
    ctx.owner_identity_id = pack.installed_by;
    ctx
}

fn pack_action_allowed(grants: &[Grant], action: Action, identity_id: i64, pack: &Pack) -> bool {
    if pack.is_standard {
        return true;
    }

    let ctx = pack_authorization_context(identity_id, pack);
    if pack.installed_by.is_some() && pack.installed_by != Some(identity_id) {
        return constrained_pack_grant_allows(grants, action, &ctx);
    }

    AuthorizationService::is_allowed(grants, Resource::Packs, action, &ctx)
}

fn constrained_pack_grant_allows(
    grants: &[Grant],
    action: Action,
    ctx: &AuthorizationContext,
) -> bool {
    grants.iter().any(|grant| {
        let Some(constraints) = &grant.constraints else {
            return false;
        };
        let pack_scoped = constraints.owner.is_some()
            || constraints.pack_refs.is_some()
            || constraints.refs.is_some()
            || constraints.ids.is_some();
        grant.resource == Resource::Packs
            && grant.actions.contains(&action)
            && pack_scoped
            && grant.allows(Resource::Packs, action, ctx)
    })
}

/// Translates a token's effective RBAC grants into a SQL-evaluable
/// [`PackVisibilityFilter`], mirroring `pack_action_allowed`/
/// `constrained_pack_grant_allows` row-by-row semantics entirely in SQL:
/// an unconstrained grant matches any pack the identity owns or that has no
/// owner; constrained grants that are "pack scoped" (owner/pack_refs/refs/ids)
/// additionally apply to packs installed by someone else.
fn build_pack_visibility_filter(identity_id: i64, grants: &[Grant]) -> PackVisibilityFilter {
    let mut own_or_ownerless_scopes = Vec::new();
    let mut other_owner_scopes = Vec::new();

    for grant in grants {
        if grant.resource != Resource::Packs || !grant.actions.contains(&Action::Read) {
            continue;
        }
        let Some(constraints) = &grant.constraints else {
            // Unconstrained grants only ever satisfy `is_allowed` checks,
            // which `pack_action_allowed` only consults for own/ownerless
            // packs (see `constrained_pack_grant_allows`'s `Some(constraints)`
            // requirement for other-owner packs).
            own_or_ownerless_scopes.push(PackVisibilityScope::default());
            continue;
        };
        if !pack_grant_context_feasible(constraints) {
            continue;
        }

        let scope = PackVisibilityScope {
            owner: constraints.owner,
            pack_refs: constraints.pack_refs.clone(),
            refs: constraints.refs.clone(),
            ids: constraints.ids.clone(),
        };
        let pack_scoped = constraints.owner.is_some()
            || constraints.pack_refs.is_some()
            || constraints.refs.is_some()
            || constraints.ids.is_some();
        if pack_scoped {
            other_owner_scopes.push(scope.clone());
        }
        own_or_ownerless_scopes.push(scope);
    }

    PackVisibilityFilter {
        identity_id,
        own_or_ownerless_scopes,
        other_owner_scopes,
    }
}

/// Returns `false` when `constraints` depend on authorization-context fields
/// that are never populated for pack visibility checks (packs have no
/// artifact visibility, execution scope, encryption flag, or `owner_type`),
/// meaning the grant could never match any pack row.
fn pack_grant_context_feasible(constraints: &GrantConstraints) -> bool {
    if constraints.owner_types.is_some() {
        return false;
    }
    if constraints.visibility.is_some() {
        return false;
    }
    if let Some(execution_scope) = constraints.execution_scope {
        if !matches!(execution_scope, ExecutionScopeConstraint::Any) {
            return false;
        }
    }
    if constraints.encrypted.is_some() {
        return false;
    }
    if let Some(attributes) = &constraints.attributes {
        if !attributes.is_empty() {
            return false;
        }
    }
    true
}

fn emit_pack_audit(
    state: &Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
    event_type: &'static str,
    pack: &Pack,
    details: serde_json::Value,
) {
    let mut builder =
        AuditEventBuilder::new(AuditCategory::Pack, event_type, AuditOutcome::Success)
            .resource("pack")
            .resource_id(pack.id)
            .resource_ref(pack.r#ref.clone())
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
    use attune_common::config::PackUploadConfig;
    use std::io::Write;

    #[test]
    fn test_pack_routes_structure() {
        // Just verify the router can be constructed
        let _router = routes();
    }

    #[test]
    fn privileged_pack_operations_reject_service_tokens() {
        use crate::auth::jwt::TokenType;

        assert!(require_pack_access_token(&TokenType::Access).is_ok());
        for token_type in [
            TokenType::Execution,
            TokenType::Sensor,
            TokenType::Worker,
            TokenType::Refresh,
        ] {
            assert!(require_pack_access_token(&token_type).is_err());
        }
    }

    #[test]
    fn registry_provenance_uses_concrete_artifact_and_detects_fallback() {
        use attune_common::pack_registry::{
            InstallSource, InstalledPack, PackIndexEntry, PackSource, RegistryPackIdentity,
        };

        let installed = InstalledPack {
            path: PathBuf::from("/tmp/pack"),
            source: PackSource::Archive {
                url: "https://downloads.example.com/pack.tar.gz".to_string(),
            },
            checksum: Some(format!("sha256:{}", "a".repeat(64))),
            checksum_subject: Some(attune_common::pack_registry::ChecksumSubject::ArchiveBytes),
            checksum_verified: true,
            registry_identity: Some(RegistryPackIdentity {
                pack_ref: "example".to_string(),
                version: "1.2.3".to_string(),
                registry_url: "https://registry.example.com/index.json".to_string(),
            }),
        };
        let resolution = RegistryResolution {
            registry_url: "https://registry.example.com/index.json".to_string(),
            entry: PackIndexEntry {
                pack_ref: "example".to_string(),
                label: "Example".to_string(),
                description: "test".to_string(),
                use_case: None,
                version: "1.2.3".to_string(),
                author: "Test".to_string(),
                email: None,
                homepage: None,
                repository: None,
                license: "MIT".to_string(),
                keywords: Vec::new(),
                runtime_deps: Vec::new(),
                install_sources: Vec::new(),
                contents: Default::default(),
                dependencies: None,
                meta: None,
            },
            preferred_source: InstallSource::Git {
                url: "https://github.com/example/pack.git".to_string(),
                git_ref: Some("v1.2.3".to_string()),
                checksum: format!("sha256:{}", "b".repeat(64)),
            },
        };
        assert!(!resolution.matches(&installed.source));

        let provenance = build_pack_install_provenance(
            &installed,
            &[PackRegistryIndexSummary {
                id: Some(42),
                name: Some("Example".to_string()),
                url: "https://registry.example.com/index.json".to_string(),
                position: 0,
            }],
            installed.checksum.clone(),
            installed.checksum_subject,
            true,
            true,
        );
        assert_eq!(provenance.artifact_type, "archive");
        assert_eq!(
            provenance.artifact_url.as_deref(),
            Some("https://downloads.example.com/pack.tar.gz")
        );
        assert_eq!(provenance.registry_id, Some(42));
        assert_eq!(provenance.resolved_pack.as_deref(), Some("example@1.2.3"));
        assert_eq!(
            provenance.checksum_subject,
            Some(attune_common::pack_registry::ChecksumSubject::ArchiveBytes)
        );
        assert!(provenance.checksum_verified);
        assert!(provenance.fallback_occurred);
    }

    #[test]
    fn registry_source_matching_uses_canonical_validated_urls() {
        use attune_common::pack_registry::{InstallSource, PackIndexEntry, PackSource};

        let resolution = RegistryResolution {
            registry_url: "https://registry.example.com/index.json".to_string(),
            entry: PackIndexEntry {
                pack_ref: "example".to_string(),
                label: "Example".to_string(),
                description: String::new(),
                use_case: None,
                version: "1.2.3".to_string(),
                author: "Test".to_string(),
                email: None,
                homepage: None,
                repository: None,
                license: "MIT".to_string(),
                keywords: Vec::new(),
                runtime_deps: Vec::new(),
                install_sources: Vec::new(),
                contents: Default::default(),
                dependencies: None,
                meta: None,
            },
            preferred_source: InstallSource::Git {
                url: "https://EXAMPLE.com.:443/pack.git".to_string(),
                git_ref: Some("a".repeat(40)),
                checksum: format!("sha256:{}", "b".repeat(64)),
            },
        };
        let installed = PackSource::Git {
            url: "https://example.com/pack.git".to_string(),
            git_ref: Some("a".repeat(40)),
        };

        assert!(resolution.matches(&installed));
        assert!(!equivalent_remote_pack_urls(
            "https://example.com/pack.git?token=secret",
            "https://example.com/pack.git"
        ));
    }

    #[test]
    fn provenance_merge_preserves_existing_installer_data() {
        let provenance = PackInstallProvenance {
            artifact_type: "archive".to_string(),
            artifact_url: Some("https://example.com/pack.tar.gz".to_string()),
            git_ref: None,
            registry_id: Some(7),
            registry_url: Some("https://registry.example.com/index.json".to_string()),
            resolved_pack: Some("example@1.2.3".to_string()),
            checksum: Some(format!("sha256:{}", "a".repeat(64))),
            checksum_subject: Some(attune_common::pack_registry::ChecksumSubject::ArchiveBytes),
            checksum_verified: true,
            fallback_occurred: false,
        };
        let merged = merge_installation_provenance(
            &serde_json::json!({"custom_installer": {"enabled": true}}),
            &provenance,
        );

        assert_eq!(merged["custom_installer"]["enabled"], true);
        assert_eq!(
            merged["installation_provenance"]["resolved_pack"],
            "example@1.2.3"
        );
        assert_eq!(
            merged["installation_provenance"]["checksum_subject"],
            "archive_bytes"
        );
    }

    #[test]
    fn unverified_archive_provenance_keeps_archive_checksum_subject() {
        use attune_common::pack_registry::{
            ChecksumSubject, InstalledPack, PackSource, RegistryPackIdentity,
        };

        let checksum = format!("sha256:{}", "c".repeat(64));
        let installed = InstalledPack {
            path: PathBuf::from("/tmp/pack"),
            source: PackSource::Archive {
                url: "https://downloads.example.com/pack.tar.gz".to_string(),
            },
            checksum: Some(checksum.clone()),
            checksum_subject: Some(ChecksumSubject::ArchiveBytes),
            checksum_verified: false,
            registry_identity: Some(RegistryPackIdentity {
                pack_ref: "example".to_string(),
                version: "1.2.3".to_string(),
                registry_url: "https://registry.example.com/index.json".to_string(),
            }),
        };

        let provenance = build_pack_install_provenance(
            &installed,
            &[],
            installed.checksum.clone(),
            installed.checksum_subject,
            installed.checksum_verified,
            false,
        );

        assert_eq!(provenance.checksum.as_deref(), Some(checksum.as_str()));
        assert_eq!(
            provenance.checksum_subject,
            Some(ChecksumSubject::ArchiveBytes)
        );
        assert!(!provenance.checksum_verified);
    }

    #[test]
    fn temporary_install_cleanup_removes_the_whole_install_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("pack-installs").join("install-id");
        let pack = root.join("nested-pack");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("pack.yaml"), "ref: example\n").unwrap();

        drop(TemporaryInstallCleanup::new(temp.path(), &pack));

        assert!(!root.exists());
    }

    #[tokio::test]
    async fn resolved_registry_entry_is_not_fetched_again_during_install() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let server_requests = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let accepted =
                    tokio::time::timeout(std::time::Duration::from_millis(250), listener.accept())
                        .await;
                let Ok(Ok((mut stream, _))) = accepted else {
                    break;
                };
                server_requests.fetch_add(1, Ordering::SeqCst);
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).await.unwrap();
                let body = serde_json::json!({
                    "registry_name": "Test",
                    "registry_url": "https://registry.example.com",
                    "version": "1.0",
                    "last_updated": "2026-01-01T00:00:00Z",
                    "packs": [{
                        "ref": "example",
                        "label": "Example",
                        "description": "test",
                        "version": "1.2.3",
                        "author": "Test",
                        "license": "MIT",
                        "keywords": [],
                        "runtime_deps": [],
                        "install_sources": [{
                            "type": "git",
                            "url": "https://127.0.0.1:1/unavailable.git",
                            "ref": "0123456789abcdef0123456789abcdef01234567",
                            "checksum": format!("sha256:{}", "0".repeat(64))
                        }],
                        "contents": {
                            "actions": [],
                            "sensors": [],
                            "triggers": [],
                            "rules": [],
                            "workflows": []
                        }
                    }]
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let config = attune_common::config::PackRegistryConfig {
            indices: vec![attune_common::config::RegistryIndexConfig {
                url: format!("http://{address}/index.json"),
                priority: 0,
                enabled: true,
                name: Some("Test".to_string()),
                headers: Default::default(),
            }],
            approved_public_hosts: Vec::new(),
            approved_private_hosts: vec!["127.0.0.1".to_string()],
            allow_http: true,
            ..Default::default()
        };
        let source = attune_common::pack_registry::PackSource::Registry {
            pack_ref: "example".to_string(),
            version: Some("1.2.3".to_string()),
        };
        let resolution = resolve_registry_request(&config, &source).await.unwrap();
        let temp = tempfile::tempdir().unwrap();
        let installer = attune_common::pack_registry::PackInstaller::new(temp.path(), Some(config))
            .await
            .unwrap();

        assert!(installer
            .install_resolved_registry_pack(resolution.entry, resolution.registry_url)
            .await
            .is_err());
        server.await.unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn pack_checksums_are_canonicalized() {
        let raw = "A".repeat(64);
        assert_eq!(
            canonical_pack_checksum(&raw).unwrap(),
            format!("sha256:{}", "a".repeat(64))
        );
        assert_eq!(
            canonical_pack_checksum(&format!("SHA256:{raw}")).unwrap(),
            format!("sha256:{}", "a".repeat(64))
        );
    }

    #[test]
    fn global_pack_index_administration_rejects_every_constraint_dimension() {
        let constrained = [
            GrantConstraints {
                owner: Some(attune_common::rbac::OwnerConstraint::Any),
                ..Default::default()
            },
            GrantConstraints {
                owner: Some(attune_common::rbac::OwnerConstraint::None),
                ..Default::default()
            },
            GrantConstraints {
                owner: Some(attune_common::rbac::OwnerConstraint::SelfOnly),
                ..Default::default()
            },
            GrantConstraints {
                pack_refs: Some(vec!["example".to_string()]),
                ..Default::default()
            },
            GrantConstraints {
                owner_types: Some(vec![attune_common::models::OwnerType::Pack]),
                ..Default::default()
            },
            GrantConstraints {
                owner_refs: Some(vec!["example".to_string()]),
                ..Default::default()
            },
            GrantConstraints {
                refs: Some(vec!["example".to_string()]),
                ..Default::default()
            },
            GrantConstraints {
                ids: Some(vec![1]),
                ..Default::default()
            },
            GrantConstraints {
                attributes: Some(std::collections::HashMap::from([(
                    "team".to_string(),
                    serde_json::json!("platform"),
                )])),
                ..Default::default()
            },
        ];

        for constraints in constrained {
            let grants = [Grant {
                resource: Resource::Packs,
                actions: vec![Action::Configure],
                constraints: Some(constraints),
            }];
            assert!(
                !crate::routes::visibility::has_unconstrained_resource_action(
                    &grants,
                    Resource::Packs,
                    Action::Configure,
                )
            );
        }

        for constraints in [None, Some(GrantConstraints::default())] {
            let grants = [Grant {
                resource: Resource::Packs,
                actions: vec![Action::Configure],
                constraints,
            }];
            assert!(
                crate::routes::visibility::has_unconstrained_resource_action(
                    &grants,
                    Resource::Packs,
                    Action::Configure,
                )
            );
        }
    }

    #[test]
    fn direct_pack_sources_reject_unsafe_remote_transports() {
        for source in [
            "git://example.com/repo.git",
            "git@example.com:repo.git",
            "file:///tmp/repo",
            "ftp://example.com/repo.zip",
        ] {
            assert!(
                detect_pack_source(source, None, true).is_err(),
                "{}",
                source
            );
        }

        let query_error = detect_pack_source(
            "https://github.com/attacker/pack.git?token=super-secret",
            None,
            true,
        )
        .unwrap_err();
        assert!(!query_error.to_string().contains("super-secret"));
    }

    #[test]
    fn no_registry_rejects_implicit_registry_references() {
        assert!(detect_pack_source("example", None, false).is_err());
        assert!(matches!(
            detect_pack_source("example", None, true).unwrap(),
            attune_common::pack_registry::PackSource::Registry { .. }
        ));
    }

    #[test]
    fn registry_manifest_identity_must_match_ref_and_version() {
        let identity = attune_common::pack_registry::RegistryPackIdentity {
            pack_ref: "example".to_string(),
            version: "1.2.3".to_string(),
            registry_url: "https://registry.example/index.json".to_string(),
        };
        assert!(validate_registry_manifest_identity(Some(&identity), "example", "1.2.3").is_ok());
        assert!(validate_registry_manifest_identity(Some(&identity), "other", "1.2.3").is_err());
        assert!(validate_registry_manifest_identity(Some(&identity), "example", "2.0.0").is_err());
    }

    #[test]
    fn managed_registry_headers_are_encrypted_and_redactions_preserve_secrets() {
        const KEY: &str = "registry-header-test-encryption-key-32-chars";
        let existing = serde_json::json!({"Authorization": "Bearer secret"});
        let encrypted = encrypt_managed_headers_with_key(existing.clone(), None, KEY).unwrap();
        assert!(encrypted.is_string());
        assert!(!encrypted.as_str().unwrap().contains("secret"));
        assert_eq!(
            attune_common::crypto::decrypt_json(&encrypted, KEY).unwrap(),
            existing
        );

        let encrypted = encrypt_managed_headers_with_key(
            serde_json::json!({"Authorization": "[REDACTED]"}),
            Some(&existing),
            KEY,
        )
        .unwrap();
        let decrypted = attune_common::crypto::decrypt_json(&encrypted, KEY).unwrap();
        assert_eq!(decrypted["Authorization"], "Bearer secret");
        assert!(!decrypted.to_string().contains("[REDACTED]"));
    }

    #[test]
    fn standard_marker_controls_static_registry_bootstrap() {
        let now = chrono::Utc::now();
        let standard = attune_common::models::PackRegistryIndex {
            id: 1,
            name: Some("Attune Standard Pack Index".to_string()),
            url: "https://raw.githubusercontent.com/attune-system/index/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/index.json".to_string(),
            position: 0,
            enabled: true,
            is_standard: true,
            headers: serde_json::json!({}),
            created: now,
            updated: now,
        };
        let custom = attune_common::models::PackRegistryIndex {
            id: 2,
            name: Some("Company Packs".to_string()),
            url: "https://company.example/index.json".to_string(),
            position: 1,
            enabled: true,
            is_standard: false,
            headers: serde_json::json!({}),
            created: now,
            updated: now,
        };

        assert!(static_bootstrap_indices_are_effective(
            std::slice::from_ref(&standard)
        ));
        assert!(!static_bootstrap_indices_are_effective(&[standard, custom]));
        assert!(static_bootstrap_indices_are_effective(&[]));
    }

    #[test]
    fn managed_registry_duplicates_are_first_row_wins() {
        let now = chrono::Utc::now();
        let managed = vec![
            attune_common::models::PackRegistryIndex {
                id: 1,
                name: Some("Disabled first".to_string()),
                url: "HTTPS://RAW.GITHUBUSERCONTENT.COM.:443/attune-system/index/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/index.json".to_string(),
                position: 0,
                enabled: false,
                is_standard: true,
                headers: serde_json::json!({}),
                created: now,
                updated: now,
            },
            attune_common::models::PackRegistryIndex {
                id: 2,
                name: Some("Enabled duplicate".to_string()),
                url: "https://raw.githubusercontent.com/attune-system/index/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/index.json".to_string(),
                position: 1,
                enabled: true,
                is_standard: false,
                headers: serde_json::json!({}),
                created: now,
                updated: now,
            },
        ];

        let (deduplicated, identities) = deduplicate_managed_registry_indices(managed);

        assert_eq!(deduplicated.len(), 1);
        assert_eq!(deduplicated[0].id, 1);
        assert!(!deduplicated[0].enabled);
        assert_eq!(
            identities,
            std::collections::HashSet::from([
                registry_identity_key("https://raw.githubusercontent.com/attune-system/index/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/index.json")
            ])
        );
    }

    #[test]
    fn duplicate_static_registry_uses_highest_priority_entry() {
        let static_indices = vec![
            attune_common::config::RegistryIndexConfig {
                url: "https://company.example/index.json".to_string(),
                priority: 10,
                enabled: true,
                name: Some("Lower priority".to_string()),
                headers: Default::default(),
            },
            attune_common::config::RegistryIndexConfig {
                url: "https://company.example:443/index.json".to_string(),
                priority: 2,
                enabled: true,
                name: Some("Higher priority".to_string()),
                headers: Default::default(),
            },
        ];

        let indices =
            effective_static_registry_indices(&static_indices, &Default::default(), 1, false);

        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0].name.as_deref(), Some("Higher priority"));
        assert_eq!(indices[0].priority, 3);
    }

    #[test]
    fn redacted_new_registry_header_is_rejected() {
        assert!(encrypt_managed_headers_with_key(
            serde_json::json!({"Authorization": "[REDACTED]"}),
            None,
            "registry-header-test-encryption-key-32-chars",
        )
        .is_err());
    }

    #[test]
    fn pack_icon_refs_must_be_safe_path_segments() {
        assert!(is_valid_pack_ref_path_segment("core"));
        assert!(is_valid_pack_ref_path_segment("my_pack_1-alpha"));

        assert!(!is_valid_pack_ref_path_segment(""));
        assert!(!is_valid_pack_ref_path_segment("../core"));
        assert!(!is_valid_pack_ref_path_segment("core/pack"));
        assert!(!is_valid_pack_ref_path_segment("core pack"));
    }

    #[test]
    fn malformed_pack_removal_refs_fail_before_storage_access() {
        assert!(validated_pack_removal_ref("../outside", "demo").is_err());
        assert!(validated_pack_removal_ref("demo", "../outside").is_err());
        assert!(validated_pack_removal_ref("demo", "other").is_err());
        assert_eq!(validated_pack_removal_ref("demo", "demo").unwrap(), "demo");
    }

    #[tokio::test]
    async fn pack_install_operations_are_serialized() {
        let first = PACK_INSTALL_LOCK.lock().await;
        assert!(PACK_INSTALL_LOCK.try_lock().is_err());
        drop(first);
        assert!(PACK_INSTALL_LOCK.try_lock().is_ok());
    }

    #[tokio::test]
    async fn finds_pack_icon_by_supported_filename_priority() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let pack_dir = temp_dir.path().join("demo");
        std::fs::create_dir(&pack_dir).expect("create pack dir");
        std::fs::write(pack_dir.join("pack-icon.png"), b"png").expect("write png");
        std::fs::write(pack_dir.join("pack-icon.svg"), b"svg").expect("write svg");

        let (path, content_type) = find_pack_icon(temp_dir.path(), "demo").await.expect("icon");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("pack-icon.svg")
        );
        assert_eq!(content_type, "image/svg+xml");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pack_icon_refuses_symlinks() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let pack_dir = temp_dir.path().join("demo");
        std::fs::create_dir(&pack_dir).expect("create pack dir");
        let outside = temp_dir.path().join("outside.svg");
        std::fs::write(&outside, b"secret").expect("write outside icon");
        symlink(&outside, pack_dir.join("pack-icon.svg")).expect("create icon symlink");

        assert!(find_pack_icon(temp_dir.path(), "demo").await.is_none());
    }

    // ---- safe_unpack tests --------------------------------------------------

    fn build_tar<F>(build: F) -> Vec<u8>
    where
        F: FnOnce(&mut tar::Builder<Vec<u8>>),
    {
        let mut b = tar::Builder::new(Vec::new());
        build(&mut b);
        b.into_inner().expect("tar finalize")
    }

    fn append_file(b: &mut tar::Builder<Vec<u8>>, path: &str, data: &[u8]) {
        let mut h = tar::Header::new_gnu();
        h.set_path(path).unwrap();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append(&h, data).unwrap();
    }

    fn append_raw_path_file(b: &mut tar::Builder<Vec<u8>>, raw_path: &str, data: &[u8]) {
        // Bypass `set_path` validation to construct malicious entries (absolute /
        // traversal). We append a normal entry then patch the name field of the
        // 512-byte header in-place. tar headers store the name at offset 0..100
        // (NUL-padded). We must also recompute the checksum (offset 148..156).
        let placeholder = format!("__placeholder_{}__", raw_path.len());
        let mut h = tar::Header::new_gnu();
        h.set_path(&placeholder).unwrap();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append(&h, data).unwrap();

        // Patch the most recently written header (the previous 512-byte block
        // before the data block(s)).
        let buf = b.get_mut();
        let data_blocks = data.len().div_ceil(512);
        let header_start = buf.len() - 512 - data_blocks * 512;
        // Zero the old name region.
        for byte in &mut buf[header_start..header_start + 100] {
            *byte = 0;
        }
        let bytes = raw_path.as_bytes();
        let n = bytes.len().min(100);
        buf[header_start..header_start + n].copy_from_slice(&bytes[..n]);

        // Recompute checksum: zero the checksum field, sum all 512 header bytes
        // (treating cksum field as spaces), then write octal+NUL+space.
        for byte in &mut buf[header_start + 148..header_start + 156] {
            *byte = b' ';
        }
        let sum: u32 = buf[header_start..header_start + 512]
            .iter()
            .map(|&b| b as u32)
            .sum();
        let cksum_str = format!("{:06o}\0 ", sum);
        buf[header_start + 148..header_start + 156].copy_from_slice(cksum_str.as_bytes());
    }

    fn unpack_bytes(bytes: &[u8], cfg: &PackUploadConfig) -> Result<tempfile::TempDir, String> {
        let dir = tempfile::tempdir().unwrap();
        let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
        archive.set_overwrite(false);
        archive.set_unpack_xattrs(false);
        archive.set_preserve_permissions(false);
        archive.set_preserve_mtime(false);
        safe_unpack(&mut archive, dir.path(), cfg)?;
        Ok(dir)
    }

    #[test]
    fn safe_unpack_accepts_normal_archive() {
        let bytes = build_tar(|b| {
            append_file(b, "pack.yaml", b"ref: test\nlabel: Test\n");
            append_file(b, "actions/echo.sh", b"#!/bin/sh\necho hi\n");
        });
        let dir = unpack_bytes(&bytes, &PackUploadConfig::default()).unwrap();
        assert!(dir.path().join("pack.yaml").exists());
        assert!(dir.path().join("actions/echo.sh").exists());
    }

    #[test]
    fn safe_unpack_rejects_path_traversal() {
        let bytes = build_tar(|b| {
            append_raw_path_file(b, "../escape.txt", b"pwn");
        });
        let err = unpack_bytes(&bytes, &PackUploadConfig::default()).unwrap_err();
        assert!(
            err.contains("Unsafe archive entry path"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn safe_unpack_rejects_absolute_path() {
        let bytes = build_tar(|b| {
            append_raw_path_file(b, "/etc/passwd", b"root:x:0:0::/root:/bin/sh\n");
        });
        let err = unpack_bytes(&bytes, &PackUploadConfig::default()).unwrap_err();
        assert!(err.contains("relative"), "unexpected error: {}", err);
    }

    #[test]
    fn safe_unpack_rejects_symlink() {
        let bytes = build_tar(|b| {
            let mut h = tar::Header::new_gnu();
            h.set_size(0);
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_mode(0o777);
            b.append_link(&mut h, "evil-link", "/etc/passwd").unwrap();
        });
        let err = unpack_bytes(&bytes, &PackUploadConfig::default()).unwrap_err();
        assert!(err.contains("Symlink"), "unexpected error: {}", err);
    }

    #[test]
    fn safe_unpack_rejects_hardlink() {
        let bytes = build_tar(|b| {
            let mut h = tar::Header::new_gnu();
            h.set_size(0);
            h.set_entry_type(tar::EntryType::Link);
            h.set_mode(0o644);
            b.append_link(&mut h, "evil-hard", "pack.yaml").unwrap();
        });
        let err = unpack_bytes(&bytes, &PackUploadConfig::default()).unwrap_err();
        assert!(err.contains("Link"), "unexpected error: {}", err);
    }

    #[test]
    fn safe_unpack_rejects_when_total_size_exceeded() {
        let bytes = build_tar(|b| {
            append_file(b, "a.bin", &vec![0u8; 600]);
            append_file(b, "b.bin", &vec![0u8; 600]);
        });
        let cfg = PackUploadConfig {
            max_extracted_size_bytes: Some(1000),
            max_per_entry_size_bytes: Some(800),
            ..Default::default()
        };
        let err = unpack_bytes(&bytes, &cfg).unwrap_err();
        assert!(
            err.contains("total extracted size"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn safe_unpack_rejects_when_per_entry_size_exceeded() {
        let bytes = build_tar(|b| {
            append_file(b, "huge.bin", &vec![0u8; 5000]);
        });
        let cfg = PackUploadConfig {
            max_per_entry_size_bytes: Some(1000),
            ..Default::default()
        };
        let err = unpack_bytes(&bytes, &cfg).unwrap_err();
        assert!(
            err.contains("per-entry extracted size limit"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn safe_unpack_rejects_too_many_files() {
        let bytes = build_tar(|b| {
            for i in 0..6 {
                append_file(b, &format!("f{}.txt", i), b"x");
            }
        });
        let cfg = PackUploadConfig {
            max_file_count: Some(5),
            ..Default::default()
        };
        let err = unpack_bytes(&bytes, &cfg).unwrap_err();
        assert!(
            err.contains("too many entries"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn safe_unpack_rejects_gz_bomb_via_total_size() {
        let bytes = build_tar(|b| {
            append_file(b, "big.bin", &vec![0u8; 10 * 1024]);
        });
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(&bytes).unwrap();
        let gz_bytes = gz.finish().unwrap();
        assert!(gz_bytes.len() < bytes.len());

        let dir = tempfile::tempdir().unwrap();
        let cfg = PackUploadConfig {
            max_extracted_size_bytes: Some(4 * 1024),
            max_per_entry_size_bytes: Some(64 * 1024),
            ..Default::default()
        };
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(std::io::Cursor::new(
            &gz_bytes[..],
        )));
        archive.set_overwrite(false);
        let err = safe_unpack(&mut archive, dir.path(), &cfg).unwrap_err();
        assert!(
            err.contains("total extracted size") || err.contains("per-entry size limit"),
            "unexpected error: {}",
            err
        );
    }

    /// Defense-in-depth: even if a crafted tar header lies about its size,
    /// extraction must fail rather than write unbounded data. We construct a
    /// tar where the header advertises `size=10` but the payload is much
    /// larger, with the trailing bytes being non-zero garbage so the tar
    /// reader cannot mistake them for an end-of-archive zero block.
    #[test]
    fn safe_unpack_rejects_tar_with_size_header_mismatch() {
        // Build one valid 50KB entry, then patch its size header to claim
        // the entry is only 10 bytes long. The tar reader will read 10 bytes,
        // skip to the next 512 boundary, and try to parse the trailing
        // 0xAA-filled garbage as a subsequent header (which fails checksum).
        let payload_len: usize = 50 * 1024;
        let payload = vec![0xAAu8; payload_len];

        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_path("evil.bin").unwrap();
        h.set_size(payload_len as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append(&h, &payload[..]).unwrap();
        let mut bytes = b.into_inner().expect("tar finalize");

        // Locate the most recently written header: its block precedes the
        // payload blocks (rounded up to 512). Then patch the size field
        // (offset 124..136) and recompute the checksum (offset 148..156).
        let data_blocks = payload_len.div_ceil(512);
        // The Builder also appends two trailing zero blocks on `into_inner`.
        let trailing_zero = 2 * 512;
        let header_start = bytes.len() - trailing_zero - data_blocks * 512 - 512;

        // Octal "10" with NUL terminator, padded to 12 bytes.
        let new_size = b"00000000012\0";
        bytes[header_start + 124..header_start + 136].copy_from_slice(new_size);

        // Recompute checksum over the 512-byte header (cksum field as spaces).
        for byte in &mut bytes[header_start + 148..header_start + 156] {
            *byte = b' ';
        }
        let sum: u32 = bytes[header_start..header_start + 512]
            .iter()
            .map(|&x| x as u32)
            .sum();
        let cksum_str = format!("{:06o}\0 ", sum);
        bytes[header_start + 148..header_start + 156].copy_from_slice(cksum_str.as_bytes());

        // Use generous limits so the only possible failure mode is the
        // header/payload mismatch itself (per-entry / corrupt-tar).
        let cfg = PackUploadConfig::default();
        let err = unpack_bytes(&bytes, &cfg).unwrap_err();
        assert!(
            err.contains("Invalid TAR entry")
                || err.contains("Corrupt tar entry")
                || err.contains("per-entry size limit")
                || err.contains("Failed to write entry")
                || err.contains("Failed to read tar entries"),
            "expected extraction to fail on header/payload mismatch, got: {}",
            err
        );
    }
}
