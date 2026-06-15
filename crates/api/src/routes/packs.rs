//! Pack management API routes

use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use validator::Validate;

use attune_common::audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome};
use attune_common::models::{pack_test::PackTestResult, Pack};
use attune_common::mq::{MessageEnvelope, MessageType, PackDeletedPayload, PackRegisteredPayload};
use attune_common::rbac::{Action, AuthorizationContext, Grant, Resource};
use attune_common::repositories::{
    pack::{CreatePackInput, UpdatePackInput},
    pack_registry_index::{CreatePackRegistryIndexInput, UpdatePackRegistryIndexInput},
    work_queue::WorkQueueRepository,
    Create, Delete, FindById, FindByRef, List, PackRegistryIndexRepository, PackRepository,
    PackTestRepository, Patch, Update,
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
            IndexedPackResponse, InstallPackRequest, PackDescriptionPatch, PackInstallResponse,
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

/// List all packs with pagination
#[utoipa::path(
    get,
    path = "/api/v1/packs",
    tag = "packs",
    params(PaginationParams),
    responses(
        (status = 200, description = "List of packs", body = PaginatedResponse<PackSummary>),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_packs(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    let mut packs = PackRepository::list(&state.db).await?;

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        let grants = authz.effective_grants(&user).await?;
        packs.retain(|pack| pack_action_allowed(&grants, Action::Read, identity_id, pack));
    }

    let total = packs.len() as u64;
    let limit = pagination.limit() as usize;
    let offset = pagination.page.saturating_sub(1) as usize * limit;
    let packs = packs.into_iter().skip(offset).take(limit);

    // Convert to summaries
    let summaries: Vec<PackSummary> = packs.map(PackSummary::from).collect();

    let response = PaginatedResponse::new(summaries, &pagination, total);

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

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        let grants = authz.effective_grants(&user).await?;
        if !pack_action_allowed(&grants, Action::Read, identity_id, &pack) {
            return Err(ApiError::NotFound(format!("Pack '{}' not found", pack_ref)));
        }
    }

    let response = ApiResponse::new(PackResponse::from(pack));

    Ok((StatusCode::OK, Json(response)))
}

/// Serve the optional icon bundled at a pack root as `pack-icon.{jpg,png,ico,svg}`.
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
        let authz = AuthorizationService::new(state.db.clone());
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
        let authz = AuthorizationService::new(state.db.clone());
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
    // Check if pack exists
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
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

    // Remove pack-owned queue definitions first.
    // work_queue.pack uses ON DELETE SET NULL so explicit cleanup preserves the
    // shared model while ensuring declarative queues disappear with their pack.
    WorkQueueRepository::delete_non_adhoc_by_pack_excluding(&state.db, pack.id, &[]).await?;

    // Delete the pack from the database (cascades to actions, triggers, sensors, rules, etc.
    // Foreign keys on execution, event, enforcement, and rule tables use ON DELETE SET NULL
    // so historical records are preserved with their text ref fields intact.)
    let deleted = PackRepository::delete(&state.db, pack.id).await?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Pack '{}' not found", pack_ref)));
    }

    // Remove pack directory from permanent storage
    let pack_dir = PathBuf::from(&state.config.packs_base_dir).join(&pack_ref);
    if pack_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&pack_dir) {
            tracing::warn!(
                "Pack '{}' deleted from database but failed to remove directory {}: {}",
                pack_ref,
                pack_dir.display(),
                e
            );
        } else {
            tracing::info!("Removed pack directory: {}", pack_dir.display());
        }
    }

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

    emit_pack_audit(
        &state,
        &user,
        event_type::pack::DELETED,
        &pack,
        serde_json::json!({
            "version": pack.version.as_str(),
            "is_standard": pack.is_standard,
            "installed_by": pack.installed_by,
            "storage_removed": !pack_dir.exists(),
        }),
    );

    let response = SuccessResponse::new(format!("Pack '{}' deleted successfully", pack_ref));

    Ok((StatusCode::OK, Json(response)))
}

/// Helper function to execute pack tests and store results
async fn execute_and_store_pack_tests(
    state: &AppState,
    pack_id: i64,
    pack_ref: &str,
    pack_version: &str,
    trigger_type: &str,
    pack_dir_override: Option<&std::path::Path>,
) -> Option<Result<attune_common::models::pack_test::PackTestResult, ApiError>> {
    use attune_common::test_executor::{TestConfig, TestExecutor};
    use serde_yaml_ng;

    // Load pack.yaml from filesystem
    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);
    let pack_dir = match pack_dir_override {
        Some(dir) => dir.to_path_buf(),
        None => packs_base_dir.join(pack_ref),
    };

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

    // Create test executor
    let executor = TestExecutor::new(packs_base_dir);

    // Execute tests - use execute_pack_tests_at when we have a specific directory
    // (e.g., temp dir during installation before pack is moved to permanent storage)
    let result = match if pack_dir_override.is_some() {
        executor
            .execute_pack_tests_at(&pack_dir, pack_ref, pack_version, &test_config)
            .await
    } else {
        executor
            .execute_pack_tests(pack_ref, pack_version, &test_config)
            .await
    } {
        Ok(r) => r,
        Err(e) => {
            return Some(Err(ApiError::InternalServerError(format!(
                "Test execution failed: {}",
                e
            ))))
        }
    };

    // Store test results in database
    let pack_test_repo = PackTestRepository::new(state.db.clone());
    if let Err(e) = pack_test_repo
        .create(pack_id, pack_version, trigger_type, &result)
        .await
    {
        tracing::warn!("Failed to store test results: {}", e);
        return Some(Err(ApiError::DatabaseError(format!(
            "Failed to store test results: {}",
            e
        ))));
    }

    Some(Ok(result))
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
    request_body(content = String, content_type = "multipart/form-data"),
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

    const MAX_PACK_SIZE: usize = 100 * 1024 * 1024; // 100 MB

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Packs,
                    action: Action::Install,
                    context: AuthorizationContext::new(identity_id),
                },
            )
            .await?;
    }

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
                if data.len() > MAX_PACK_SIZE {
                    return Err(ApiError::BadRequest(format!(
                        "Pack archive too large: {} bytes (max {} bytes)",
                        data.len(),
                        MAX_PACK_SIZE
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

    // Move pack to permanent storage
    use attune_common::pack_registry::PackStorage;
    let storage = PackStorage::new(&state.config.packs_base_dir);
    let final_path = storage
        .install_pack(&pack_root, &pack_ref, None)
        .map_err(|e| {
            ApiError::InternalServerError(format!("Failed to move pack to storage: {}", e))
        })?;

    tracing::info!(
        "Pack '{}' uploaded and stored at {:?}",
        pack_ref,
        final_path
    );

    // Register the pack in the database
    let pack_id = register_pack_internal(
        state.clone(),
        user.claims.sub.clone(),
        final_path.to_string_lossy().to_string(),
        force,
        skip_tests,
    )
    .await
    .inspect_err(|_e| {
        // Clean up permanent storage on failure
        let _ = std::fs::remove_dir_all(&final_path);
    })?;

    // Fetch the registered pack
    let pack = PackRepository::find_by_id(&state.db, pack_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack with ID {} not found", pack_id)))?;

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
    use std::path::Component;
    use tar::EntryType;

    let max_total = cfg.max_extracted_size_bytes();
    let max_files = cfg.max_file_count();
    let max_entry = cfg.max_per_entry_size_bytes();
    let allow_symlinks = cfg.allow_symlinks();

    let dest_canon = std::fs::canonicalize(dest)
        .map_err(|e| format!("Failed to canonicalize destination: {}", e))?;

    let mut total_bytes: u64 = 0;
    let mut file_count: u32 = 0;

    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read tar entries: {}", e))?;

    for entry in entries {
        let mut entry = entry.map_err(|e| format!("Corrupt tar entry: {}", e))?;

        file_count = file_count.saturating_add(1);
        if file_count > max_files {
            return Err(format!(
                "Archive contains too many entries (limit: {})",
                max_files
            ));
        }

        let header = entry.header().clone();
        let etype = header.entry_type();

        match etype {
            EntryType::Regular | EntryType::Directory => {}
            EntryType::Symlink | EntryType::Link if allow_symlinks => {}
            EntryType::Symlink => {
                return Err("Archive contains a symbolic link, which is not allowed".to_string());
            }
            EntryType::Link => {
                return Err("Archive contains a hard link, which is not allowed".to_string());
            }
            EntryType::Char | EntryType::Block | EntryType::Fifo => {
                return Err(format!(
                    "Archive contains a forbidden device/FIFO entry (type: {:?})",
                    etype
                ));
            }
            other => {
                return Err(format!(
                    "Archive contains unsupported entry type: {:?}",
                    other
                ));
            }
        }

        let declared_size = header
            .size()
            .map_err(|e| format!("Invalid entry size header: {}", e))?;
        if declared_size > max_entry {
            return Err(format!(
                "Archive entry exceeds per-entry size limit ({} > {} bytes)",
                declared_size, max_entry
            ));
        }
        let projected = total_bytes.saturating_add(declared_size);
        if projected > max_total {
            return Err(format!(
                "Archive total extracted size exceeds limit ({} > {} bytes)",
                projected, max_total
            ));
        }

        let raw_path = entry
            .path()
            .map_err(|e| format!("Invalid entry path: {}", e))?
            .into_owned();
        if raw_path.is_absolute() {
            return Err(format!(
                "Archive entry has an absolute path (forbidden): {}",
                raw_path.display()
            ));
        }
        for comp in raw_path.components() {
            match comp {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir => {
                    return Err(format!(
                        "Archive entry contains '..' path traversal: {}",
                        raw_path.display()
                    ));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!(
                        "Archive entry has a non-relative path: {}",
                        raw_path.display()
                    ));
                }
            }
        }

        let target = dest_canon.join(&raw_path);
        if !target.starts_with(&dest_canon) {
            return Err(format!(
                "Archive entry escapes destination directory: {}",
                raw_path.display()
            ));
        }

        match etype {
            EntryType::Directory => {
                std::fs::create_dir_all(&target).map_err(|e| {
                    format!("Failed to create directory {}: {}", raw_path.display(), e)
                })?;
            }
            EntryType::Regular => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        format!(
                            "Failed to create parent directory for {}: {}",
                            raw_path.display(),
                            e
                        )
                    })?;
                }
                // Defense-in-depth: bound the bytes we write via the entry's
                // Read impl, rather than trusting the declared header size.
                // `take(max_entry + 1)` lets us detect any over-read (one extra
                // byte signals the limit was exceeded). The `+1` is saturating
                // so a configured `u64::MAX` limit doesn't wrap.
                let read_cap = max_entry.saturating_add(1);
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map_err(|e| format!("Failed to create {}: {}", raw_path.display(), e))?;
                let mut writer = std::io::BufWriter::new(file);
                let mut limited = std::io::Read::take(&mut entry, read_cap);
                let written = std::io::copy(&mut limited, &mut writer).map_err(|e| {
                    let _ = std::fs::remove_file(&target);
                    format!("Failed to write entry {}: {}", raw_path.display(), e)
                })?;
                std::io::Write::flush(&mut writer).map_err(|e| {
                    let _ = std::fs::remove_file(&target);
                    format!("Failed to flush entry {}: {}", raw_path.display(), e)
                })?;
                drop(writer);

                if written > max_entry {
                    let _ = std::fs::remove_file(&target);
                    return Err(format!(
                        "Archive entry exceeds per-entry size limit (actual bytes \
                         written exceeded {} bytes for {})",
                        max_entry,
                        raw_path.display()
                    ));
                }
                let projected_actual = total_bytes.saturating_add(written);
                if projected_actual > max_total {
                    let _ = std::fs::remove_file(&target);
                    return Err(format!(
                        "Archive total extracted size exceeds limit ({} > {} bytes)",
                        projected_actual, max_total
                    ));
                }
                total_bytes = projected_actual;
            }
            EntryType::Symlink | EntryType::Link if allow_symlinks => {
                // Link targets carry no payload, so unpack_in is fine here and
                // it preserves the existing path-validation semantics.
                let unpacked = entry
                    .unpack_in(&dest_canon)
                    .map_err(|e| format!("Failed to unpack entry {}: {}", raw_path.display(), e))?;
                if !unpacked {
                    return Err(format!(
                        "Tar refused to unpack entry (unsafe path): {}",
                        raw_path.display()
                    ));
                }
            }
            _ => {
                // All other entry types were rejected by the type-check above.
                unreachable!("entry type already validated");
            }
        }
    }

    Ok(())
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

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Packs,
                    action: Action::Install,
                    context: AuthorizationContext::new(identity_id),
                },
            )
            .await?;
    }

    // Call internal registration logic
    let pack_id = register_pack_internal(
        state.clone(),
        user.claims.sub.clone(),
        request.path.clone(),
        request.force,
        request.skip_tests,
    )
    .await?;

    // Fetch the registered pack
    let pack = PackRepository::find_by_id(&state.db, pack_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack with ID {} not found", pack_id)))?;

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
    _user_id: String,
    path: String,
    force: bool,
    skip_tests: bool,
) -> Result<i64, ApiError> {
    use std::fs;

    // Verify pack directory exists
    let pack_path = PathBuf::from(&path);
    if !pack_path.exists() || !pack_path.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "Pack directory does not exist: {}",
            path
        )));
    }

    // Read pack.yaml
    let pack_yaml_path = pack_path.join("pack.yaml");
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

    // Check if pack already exists — update in place to preserve IDs
    let existing_pack = PackRepository::find_by_ref(&state.db, &pack_ref).await?;

    let is_new_pack;

    let pack = if let Some(existing) = existing_pack {
        if !force {
            return Err(ApiError::Conflict(format!(
                "Pack '{}' already exists. Use force=true to reinstall.",
                pack_ref
            )));
        }

        // Update existing pack in place — preserves pack ID and all child entity IDs
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
            installers: None,
        };

        let updated = PackRepository::update(&state.db, existing.id, update_input).await?;
        tracing::info!(
            "Updated existing pack '{}' (ID: {}) in place",
            pack_ref,
            updated.id
        );
        is_new_pack = false;
        updated
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
            installers: serde_json::json!({}),
        };

        is_new_pack = true;
        PackRepository::create(&state.db, pack_input).await?
    };

    // Auto-sync workflows after pack creation
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

    // Load pack components (triggers, actions, sensors) into the database
    {
        use attune_common::pack_registry::PackComponentLoader;

        let component_loader = PackComponentLoader::new(&state.db, pack.id, &pack.r#ref);
        match component_loader.load_all(&pack_path).await {
            Ok(load_result) => {
                tracing::info!(
                    "Pack '{}' components loaded: {} created, {} updated, {} skipped, {} removed, {} warnings \
                     (runtimes: {}/{}, triggers: {}/{}, actions: {}/{}, policies: {}/{}, sensors: {}/{})",
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
                );
                for warning in &load_result.warnings {
                    tracing::warn!("Pack component warning: {}", warning);
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to load components for pack '{}': {}. Components can be loaded manually.",
                    pack.r#ref,
                    e
                );
            }
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

    // Execute tests if not skipped
    if !skip_tests {
        if let Some(test_outcome) = execute_and_store_pack_tests(
            &state,
            pack.id,
            &pack.r#ref,
            &pack.version,
            "register",
            Some(&pack_path),
        )
        .await
        {
            match test_outcome {
                Ok(result) => {
                    let test_passed = result.status == "passed";

                    if !test_passed && !force {
                        // Tests failed and force is not set — only delete if we just created this pack.
                        // If we updated an existing pack, deleting would destroy the original.
                        if is_new_pack {
                            let _ = PackRepository::delete(&state.db, pack.id).await;
                        }
                        return Err(ApiError::BadRequest("Pack registration failed: tests did not pass. Use force=true to register anyway.".to_string()));
                    }

                    if !test_passed && force {
                        tracing::warn!(
                            "Pack '{}' tests failed but force=true, continuing with registration",
                            pack.r#ref
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to execute tests for pack '{}': {}", pack.r#ref, e);
                    // If tests can't be executed and force is not set, fail the registration
                    if !force {
                        if is_new_pack {
                            let _ = PackRepository::delete(&state.db, pack.id).await;
                        }
                        return Err(ApiError::BadRequest(format!(
                            "Pack registration failed: could not execute tests. Error: {}. Use force=true to register anyway.",
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

    // Publish pack.registered event so workers can sync pack content and
    // proactively set up runtime environments (virtualenvs, node_modules, etc.).
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

    Ok(pack.id)
}

async fn authorize_pack_registry_action(
    state: &AppState,
    user: &crate::auth::middleware::AuthenticatedUser,
    action: Action,
) -> ApiResult<()> {
    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
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
    }
    Ok(())
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
    Ok(result)
}

async fn effective_pack_registry_config(
    state: &AppState,
    include_disabled: bool,
) -> ApiResult<Option<attune_common::config::PackRegistryConfig>> {
    if !state.config.pack_registry.enabled {
        return Ok(None);
    }

    let managed = PackRegistryIndexRepository::list(&state.db).await?;
    if managed.is_empty() {
        return Ok(Some(state.config.pack_registry.clone()));
    }

    let mut indices = Vec::new();
    for index in managed {
        if !include_disabled && !index.enabled {
            continue;
        }
        indices.push(attune_common::config::RegistryIndexConfig {
            url: index.url,
            priority: index.position as u32,
            enabled: index.enabled,
            name: index.name,
            headers: headers_from_json(index.headers)?,
        });
    }

    let mut config = state.config.pack_registry.clone();
    config.indices = indices;
    Ok(Some(config))
}

async fn configured_registry_summaries(
    state: &AppState,
    include_disabled: bool,
) -> ApiResult<Vec<PackRegistryIndexSummary>> {
    let managed = PackRegistryIndexRepository::list(&state.db).await?;
    if !managed.is_empty() {
        return Ok(managed
            .into_iter()
            .filter(|index| include_disabled || index.enabled)
            .map(|index| PackRegistryIndexSummary {
                id: Some(index.id),
                name: index.name,
                url: index.url,
                position: index.position,
            })
            .collect());
    }

    Ok(state
        .config
        .pack_registry
        .indices
        .iter()
        .filter(|index| include_disabled || index.enabled)
        .map(|index| PackRegistryIndexSummary {
            id: None,
            name: index.name.clone(),
            url: index.url.clone(),
            position: index.priority as i32,
        })
        .collect())
}

pub async fn list_pack_indices(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
) -> ApiResult<impl IntoResponse> {
    authorize_pack_registry_action(&state, &user, Action::Read).await?;
    let indices = PackRegistryIndexRepository::list(&state.db).await?;
    let response: Vec<PackRegistryIndexResponse> = indices
        .into_iter()
        .map(PackRegistryIndexResponse::from)
        .collect();
    Ok((StatusCode::OK, Json(ApiResponse::new(response))))
}

pub async fn create_pack_index(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreatePackRegistryIndexRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;
    authorize_pack_registry_action(&state, &user, Action::Configure).await?;
    let index = PackRegistryIndexRepository::create(
        &state.db,
        CreatePackRegistryIndexInput {
            name: request.name,
            url: request.url,
            position: request.position,
            enabled: request.enabled,
            headers: request.headers,
        },
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::new(PackRegistryIndexResponse::from(index))),
    ))
}

pub async fn update_pack_index(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i64>,
    Json(request): Json<UpdatePackRegistryIndexRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;
    authorize_pack_registry_action(&state, &user, Action::Configure).await?;
    let index = PackRegistryIndexRepository::update(
        &state.db,
        id,
        UpdatePackRegistryIndexInput {
            name: request.name,
            url: request.url,
            position: request.position,
            enabled: request.enabled,
            headers: request.headers,
        },
    )
    .await?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(PackRegistryIndexResponse::from(index))),
    ))
}

pub async fn delete_pack_index(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    authorize_pack_registry_action(&state, &user, Action::Configure).await?;
    let deleted = PackRegistryIndexRepository::delete(&state.db, id).await?;
    if !deleted {
        return Err(ApiError::NotFound(format!("Pack index {} not found", id)));
    }
    Ok((
        StatusCode::OK,
        Json(SuccessResponse::new(format!("Pack index {} deleted", id))),
    ))
}

pub async fn browse_indexed_packs(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<BrowsePackIndexQuery>,
) -> ApiResult<impl IntoResponse> {
    authorize_pack_registry_action(&state, &user, Action::Read).await?;
    let Some(config) = effective_pack_registry_config(&state, query.include_disabled).await? else {
        return Ok((
            StatusCode::OK,
            Json(ApiResponse::new(Vec::<IndexedPackResponse>::new())),
        ));
    };
    let client = attune_common::pack_registry::RegistryClient::new(config)
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
    let summaries = configured_registry_summaries(&state, query.include_disabled).await?;
    let selected_id = query.registry_id;
    let query_text = query.q.unwrap_or_default().to_lowercase();
    let mut seen = std::collections::HashSet::new();
    let mut packs = Vec::new();

    for registry in client.get_registries() {
        let Some(summary) = summaries.iter().find(|summary| {
            summary.url == registry.url && selected_id.is_none_or(|id| summary.id == Some(id))
        }) else {
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

pub async fn get_indexed_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    authorize_pack_registry_action(&state, &user, Action::Read).await?;
    let Some(config) = effective_pack_registry_config(&state, false).await? else {
        return Err(ApiError::NotFound(format!(
            "Indexed pack '{}' not found",
            pack_ref
        )));
    };
    let client = attune_common::pack_registry::RegistryClient::new(config)
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
    let summaries = configured_registry_summaries(&state, false).await?;

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

/// Install a pack from remote source (git repository)
#[utoipa::path(
    post,
    path = "/api/v1/packs/install",
    tag = "packs",
    request_body = InstallPackRequest,
    responses(
        (status = 201, description = "Pack installed successfully", body = ApiResponse<PackInstallResponse>),
        (status = 400, description = "Invalid request or tests failed", body = ApiResponse<String>),
        (status = 501, description = "Not implemented yet", body = ApiResponse<String>),
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

    tracing::info!("Installing pack from source: {}", request.source);

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Packs,
                    action: Action::Install,
                    context: AuthorizationContext::new(identity_id),
                },
            )
            .await?;
    }

    // Get user ID early to avoid borrow issues
    let user_id = user.identity_id().ok();
    let user_sub = user.claims.sub.clone();

    // Create temp directory for installations
    let temp_dir = std::env::temp_dir().join("attune-pack-installs");

    // Load registry configuration
    let registry_config = effective_pack_registry_config(&state, false).await?;

    // Create installer
    let installer = PackInstaller::new(&temp_dir, registry_config)
        .await
        .map_err(|e| ApiError::InternalServerError(format!("Failed to create installer: {}", e)))?;

    // Detect source type and create PackSource
    let source = detect_pack_source(&request.source, request.ref_spec.as_deref())?;
    let source_type = get_source_type(&source);

    // Install the pack (to temporary location)
    let installed = installer.install(source.clone()).await?;

    tracing::info!("Pack downloaded to: {:?}", installed.path);

    // Validate dependencies if not skipping
    if !request.skip_deps {
        tracing::info!("Validating pack dependencies...");

        // Load pack.yaml for dependency information
        let pack_yaml_path = installed.path.join("pack.yaml");
        if !pack_yaml_path.exists() {
            return Err(ApiError::BadRequest(format!(
                "pack.yaml not found in installed pack at: {}",
                installed.path.display()
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
    let pack_ref_for_storage = {
        let content = std::fs::read_to_string(&pack_yaml_path_for_ref).map_err(|e| {
            ApiError::InternalServerError(format!("Failed to read pack.yaml: {}", e))
        })?;
        let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&content).map_err(|e| {
            ApiError::InternalServerError(format!("Failed to parse pack.yaml: {}", e))
        })?;
        yaml.get("ref")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::BadRequest("Missing 'ref' field in pack.yaml".to_string()))?
            .to_string()
    };

    // Move pack to permanent storage BEFORE registration so that environment
    // setup (virtualenv creation, dependency installation) happens at the
    // final location rather than a temporary directory.
    let storage = PackStorage::new(&state.config.packs_base_dir);
    let final_path = storage
        .install_pack(&installed.path, &pack_ref_for_storage, None)
        .map_err(|e| {
            ApiError::InternalServerError(format!("Failed to move pack to storage: {}", e))
        })?;

    tracing::info!("Pack moved to permanent storage: {:?}", final_path);

    // Register the pack in database (from permanent storage location).
    // Remote installs always force-overwrite: if you're pulling from a remote,
    // the intent is to get that pack installed regardless of local state.
    let pack_id = register_pack_internal(
        state.clone(),
        user_sub,
        final_path.to_string_lossy().to_string(),
        true, // always force for remote installs
        request.skip_tests,
    )
    .await
    .inspect_err(|_e| {
        // Clean up the permanent storage if registration fails
        let _ = std::fs::remove_dir_all(&final_path);
    })?;

    // Fetch the registered pack
    let pack = PackRepository::find_by_id(&state.db, pack_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack with ID {} not found", pack_id)))?;

    // Calculate checksum of installed pack
    let checksum = calculate_directory_checksum(&final_path)
        .map_err(|e| {
            tracing::warn!("Failed to calculate checksum: {}", e);
            e
        })
        .ok();

    // Store installation metadata
    let (source_url, source_ref) =
        get_source_metadata(&source, &request.source, request.ref_spec.as_deref());

    PackRepository::update_installation_metadata(
        &state.db,
        pack_id,
        source_type.to_string(),
        source_url,
        source_ref,
        checksum.clone(),
        installed.checksum.is_some() && checksum.is_some(),
        user_id,
        "api".to_string(),
        final_path.to_string_lossy().to_string(),
    )
    .await
    .map_err(|e| {
        tracing::warn!("Failed to store installation metadata: {}", e);
        ApiError::DatabaseError(format!("Failed to store installation metadata: {}", e))
    })?;

    // Clean up temp directory
    let _ = installer.cleanup(&installed.path).await;

    emit_pack_audit(
        &state,
        &user,
        event_type::pack::INSTALLED,
        &pack,
        serde_json::json!({
            "source": request.source,
            "ref_spec": request.ref_spec,
            "version": pack.version.as_str(),
            "skip_tests": request.skip_tests,
            "checksum": checksum,
        }),
    );

    let response = PackInstallResponse {
        pack: PackResponse::from(pack),
        test_result: None, // TODO: Include test results
        tests_skipped: request.skip_tests,
    };

    Ok((StatusCode::OK, Json(crate::dto::ApiResponse::new(response))))
}

fn detect_pack_source(
    source: &str,
    ref_spec: Option<&str>,
) -> Result<attune_common::pack_registry::PackSource, ApiError> {
    use attune_common::pack_registry::PackSource;
    use std::path::Path;

    // Check if it's a URL
    if source.starts_with("http://") || source.starts_with("https://") {
        if source.ends_with(".git") || ref_spec.is_some() {
            return Ok(PackSource::Git {
                url: source.to_string(),
                git_ref: ref_spec.map(String::from),
            });
        }
        return Ok(PackSource::Archive {
            url: source.to_string(),
        });
    }

    // Check if it's a git SSH URL
    if source.starts_with("git@") || source.contains("git://") {
        return Ok(PackSource::Git {
            url: source.to_string(),
            git_ref: ref_spec.map(String::from),
        });
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

/// Extract source URL and ref from PackSource
fn get_source_metadata(
    source: &attune_common::pack_registry::PackSource,
    original_source: &str,
    _ref_spec: Option<&str>,
) -> (Option<String>, Option<String>) {
    use attune_common::pack_registry::PackSource;
    match source {
        PackSource::Git { url, git_ref } => (Some(url.clone()), git_ref.clone()),
        PackSource::Archive { url } => (Some(url.clone()), None),
        PackSource::LocalDirectory { path } => (Some(path.to_string_lossy().to_string()), None),
        PackSource::LocalArchive { path } => (Some(path.to_string_lossy().to_string()), None),
        PackSource::Registry {
            pack_ref: _,
            version,
        } => (Some(original_source.to_string()), version.clone()),
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
    use attune_common::test_executor::{TestConfig, TestExecutor};
    use serde_yaml_ng;

    // Get pack from database
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    // Load pack.yaml from filesystem
    let packs_base_dir = PathBuf::from(&state.config.packs_base_dir);
    let pack_dir = packs_base_dir.join(&pack_ref);

    if !pack_dir.exists() {
        return Err(ApiError::NotFound(format!(
            "Pack directory not found: {}",
            pack_dir.display()
        )));
    }

    let pack_yaml_path = pack_dir.join("pack.yaml");
    if !pack_yaml_path.exists() {
        return Err(ApiError::NotFound(format!(
            "pack.yaml not found for pack '{}'",
            pack_ref
        )));
    }

    // Parse pack.yaml
    let pack_yaml_content = tokio::fs::read_to_string(&pack_yaml_path)
        .await
        .map_err(|e| ApiError::InternalServerError(format!("Failed to read pack.yaml: {}", e)))?;

    let pack_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&pack_yaml_content)
        .map_err(|e| ApiError::InternalServerError(format!("Failed to parse pack.yaml: {}", e)))?;

    // Extract test configuration
    let testing_config = pack_yaml.get("testing").ok_or_else(|| {
        ApiError::BadRequest("No testing configuration found in pack.yaml".to_string())
    })?;

    let test_config: TestConfig =
        serde_yaml_ng::from_value(testing_config.clone()).map_err(|e| {
            ApiError::InternalServerError(format!("Failed to parse test configuration: {}", e))
        })?;

    if !test_config.enabled {
        return Err(ApiError::BadRequest(
            "Testing is disabled for this pack".to_string(),
        ));
    }

    // Create test executor
    let executor = TestExecutor::new(packs_base_dir);

    // Execute tests
    let result = executor
        .execute_pack_tests_at(&pack_dir, &pack_ref, &pack.version, &test_config)
        .await
        .map_err(|e| ApiError::InternalServerError(format!("Test execution failed: {}", e)))?;

    // Store test results in database
    let pack_test_repo = PackTestRepository::new(state.db.clone());
    pack_test_repo
        .create(pack.id, &pack.version, "manual", &result)
        .await
        .map_err(|e| {
            tracing::warn!("Failed to store test results: {}", e);
            ApiError::DatabaseError(format!("Failed to store test results: {}", e))
        })?;

    let response = ApiResponse::with_message(result, "Pack tests executed successfully");

    Ok((StatusCode::OK, Json(response)))
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
    RequireAuth(_user): RequireAuth,
    Json(request): Json<DownloadPacksRequest>,
) -> ApiResult<Json<ApiResponse<DownloadPacksResponse>>> {
    use attune_common::pack_registry::PackInstaller;

    // Create temp directory
    let temp_dir = std::env::temp_dir().join("attune-pack-downloads");
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| ApiError::InternalServerError(format!("Failed to create temp dir: {}", e)))?;

    // Create installer
    let registry_config = effective_pack_registry_config(&state, false).await?;

    let installer = PackInstaller::new(&temp_dir, registry_config)
        .await
        .map_err(|e| ApiError::InternalServerError(format!("Failed to create installer: {}", e)))?;

    let mut downloaded = Vec::new();
    let mut failed = Vec::new();

    for source in &request.packs {
        let pack_source = detect_pack_source(source, request.ref_spec.as_deref())?;
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
    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Packs,
                    action: Action::Install,
                    context: AuthorizationContext::new(identity_id),
                },
            )
            .await?;
    }

    let start = std::time::Instant::now();
    let mut registered = Vec::new();
    let mut failed = Vec::new();
    let total_components = 0;

    for pack_path in &request.pack_paths {
        // Call the existing register_pack_internal function
        let register_req = crate::dto::pack::RegisterPackRequest {
            path: pack_path.clone(),
            force: request.force,
            skip_tests: request.skip_tests,
        };

        match register_pack_internal(
            state.clone(),
            user.claims.sub.clone(),
            register_req.path.clone(),
            register_req.force,
            register_req.skip_tests,
        )
        .await
        {
            Ok(pack_id) => {
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
        .route("/packs/upload", axum::routing::post(upload_pack))
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
}

fn is_valid_pack_ref_path_segment(pack_ref: &str) -> bool {
    !pack_ref.is_empty()
        && pack_ref
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
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
    for (file_name, content_type) in ICON_FILES {
        let path = pack_dir.join(file_name);
        if matches!(tokio::fs::metadata(&path).await, Ok(metadata) if metadata.is_file()) {
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
    fn pack_icon_refs_must_be_safe_path_segments() {
        assert!(is_valid_pack_ref_path_segment("core"));
        assert!(is_valid_pack_ref_path_segment("my.pack_1-alpha"));

        assert!(!is_valid_pack_ref_path_segment(""));
        assert!(!is_valid_pack_ref_path_segment("../core"));
        assert!(!is_valid_pack_ref_path_segment("core/pack"));
        assert!(!is_valid_pack_ref_path_segment("core pack"));
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
            err.contains("path traversal") || err.contains("non-relative"),
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
        assert!(
            err.contains("absolute") || err.contains("non-relative"),
            "unexpected error: {}",
            err
        );
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
        assert!(err.contains("symbolic link"), "unexpected error: {}", err);
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
        assert!(err.contains("hard link"), "unexpected error: {}", err);
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
            err.contains("per-entry size limit"),
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
            err.contains("Corrupt tar entry")
                || err.contains("per-entry size limit")
                || err.contains("Failed to write entry")
                || err.contains("Failed to read tar entries"),
            "expected extraction to fail on header/payload mismatch, got: {}",
            err
        );
    }
}
