//! Work queue definition and item API routes.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use axum_extra::extract::Query as FormQuery;
use serde_json::{Map, Value as JsonValue};
use validator::Validate;

use attune_common::{
    action_visibility::{ensure_action_reference_allowed, queue_reference_allowed},
    models::{key::Key, Pack, WorkQueueBatchMode, WorkQueueConfig, WorkQueueTunableValue},
    rbac::{Action as RbacAction, AuthorizationContext, Resource},
    repositories::{
        action::ActionRepository,
        execution::ExecutionRepository,
        key::KeyRepository,
        pack::PackRepository,
        work_queue::{
            CreateWorkQueueInput, CreateWorkQueueItemInput, UpdateWorkQueueInput,
            UpdateWorkQueueItemInput, WorkQueueItemJsonPathSelector, WorkQueueItemRepository,
            WorkQueueItemSearchFilters, WorkQueueRepository, WorkQueueSearchFilters,
        },
        Create, Delete, FindById, FindByRef, Patch, Update,
    },
    trace_tag::normalize_trace_tag,
};

use crate::{
    auth::{jwt::TokenType, middleware::AuthenticatedUser, middleware::RequireAuth},
    authz::{execution_standard_pack_refs, AuthorizationCheck, AuthorizationService},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        runtime::NullableStringPatch,
        work_queue::{
            ApplyWorkQueueItemsRequest, ApplyWorkQueueItemsResponse, CreateWorkQueueRequest,
            EnqueueWorkQueueItemRequest, PreviewWorkQueueItemsRequest,
            PreviewWorkQueueItemsResponse, ResolvedWorkQueueDispatchTuningResponse,
            UpdateWorkQueueItemRequest, UpdateWorkQueueRequest, WorkQueueItemBulkOperation,
            WorkQueueItemQueryParams, WorkQueueItemResponse, WorkQueueQueryParams,
            WorkQueueResponse, WorkQueueSummary,
        },
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
    validation::validate_queue_item_payload,
};

const API_ENQUEUE_SOURCE: &str = "api";

#[utoipa::path(
    get,
    path = "/api/v1/queues",
    tag = "queues",
    params(WorkQueueQueryParams),
    responses(
        (status = 200, description = "List of work queue definitions", body = PaginatedResponse<WorkQueueSummary>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_queues(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<WorkQueueQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let mut rows = WorkQueueRepository::search(
        &state.db,
        &WorkQueueSearchFilters {
            enabled: query.enabled,
            is_adhoc: query.is_adhoc,
            search: query.search.clone(),
            limit: u32::MAX,
            offset: 0,
            ..Default::default()
        },
    )
    .await?
    .rows;
    if let Some((identity_id, grants)) = ensure_can_read_any_queue(&state, &user).await? {
        rows.retain(|queue| {
            AuthorizationService::is_allowed(
                &grants,
                Resource::Queues,
                RbacAction::Read,
                &queue_authorization_context(identity_id, queue),
            )
        });
    }
    rows = filter_visible_queues_for_discovery(
        &state,
        &user,
        rows,
        query.referencing_pack_ref.as_deref(),
    )
    .await?;
    let total = rows.len() as u64;
    let rows = paginate_rows(rows, query.page, query.per_page);

    Ok((
        StatusCode::OK,
        Json(PaginatedResponse::new(
            rows.into_iter().map(WorkQueueSummary::from).collect(),
            &PaginationParams {
                page: query.page,
                page_size: query.per_page,
            },
            total,
        )),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/packs/{pack_ref}/queues",
    tag = "queues",
    params(
        ("pack_ref" = String, Path, description = "Pack reference identifier"),
        WorkQueueQueryParams
    ),
    responses(
        (status = 200, description = "List of work queue definitions for a pack", body = PaginatedResponse<WorkQueueSummary>),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Pack not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_queues_by_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(query): Query<WorkQueueQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    let mut rows = WorkQueueRepository::search(
        &state.db,
        &WorkQueueSearchFilters {
            pack: Some(pack.id),
            enabled: query.enabled,
            is_adhoc: query.is_adhoc,
            search: query.search.clone(),
            limit: u32::MAX,
            offset: 0,
            ..Default::default()
        },
    )
    .await?
    .rows;
    if let Some((identity_id, grants)) = ensure_can_read_any_queue(&state, &user).await? {
        rows.retain(|queue| {
            AuthorizationService::is_allowed(
                &grants,
                Resource::Queues,
                RbacAction::Read,
                &queue_authorization_context(identity_id, queue),
            )
        });
    }
    rows = filter_visible_queues_for_discovery(
        &state,
        &user,
        rows,
        query.referencing_pack_ref.as_deref(),
    )
    .await?;
    let total = rows.len() as u64;
    let rows = paginate_rows(rows, query.page, query.per_page);

    Ok((
        StatusCode::OK,
        Json(PaginatedResponse::new(
            rows.into_iter().map(WorkQueueSummary::from).collect(),
            &PaginationParams {
                page: query.page,
                page_size: query.per_page,
            },
            total,
        )),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/queues/{ref}",
    tag = "queues",
    params(
        ("ref" = String, Path, description = "Queue reference identifier"),
        WorkQueueQueryParams
    ),
    responses(
        (status = 200, description = "Work queue definition", body = ApiResponse<WorkQueueResponse>),
        (status = 404, description = "Queue not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_queue(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(queue_ref): Path<String>,
    Query(query): Query<WorkQueueQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let queue = WorkQueueRepository::find_by_ref(&state.db, &queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;

    authorize_queue_action(&state, &user, RbacAction::Read, &queue)
        .await
        .map_err(|_| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;
    if !queue_visible_for_discovery(&state, &user, &queue, query.referencing_pack_ref.as_deref())
        .await?
    {
        return Err(ApiError::NotFound(format!(
            "Work queue '{}' not found",
            queue_ref
        )));
    }
    let resolved_dispatch_tuning = resolve_queue_dispatch_tuning(&state, &user, &queue).await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(
            WorkQueueResponse::from_with_resolved_tuning(queue, resolved_dispatch_tuning),
        )),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/queues",
    tag = "queues",
    request_body = CreateWorkQueueRequest,
    responses(
        (status = 201, description = "Work queue created successfully", body = ApiResponse<WorkQueueResponse>),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Pack or dispatch action not found"),
        (status = 409, description = "Queue with same ref already exists")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_queue(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreateWorkQueueRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    let (pack_id, pack_ref) = resolve_pack(&state, request.pack_ref.as_deref()).await?;
    authorize_queue_create(&state, &user, &request.r#ref, pack_ref.as_deref()).await?;
    attune_common::queue_definition::validate_work_queue_config_for_batch_mode(
        request.batch_mode,
        &request.config,
    )?;
    attune_common::queue_definition::validate_queue_reference_visibility_config(
        request.reference_visibility.unwrap_or_default(),
        &request.reference_allowed_pack_refs,
    )?;

    if WorkQueueRepository::find_by_ref(&state.db, &request.r#ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Work queue with ref '{}' already exists",
            request.r#ref
        )));
    }

    let is_adhoc = pack_id.is_none();
    let action = resolve_dispatch_action(&state, &request.dispatch_action_ref).await?;
    let trace_tag_template = request.trace_tag_template.clone();
    ensure_action_reference_allowed(&action, pack_ref.as_deref(), "work queue", &request.r#ref)?;
    let effective_permission_set_refs = request
        .permission_set_refs
        .clone()
        .unwrap_or_else(|| action.default_execution_permission_set_refs.clone());
    if !effective_permission_set_refs.is_empty()
        && !AuthorizationService::new(state.db.clone())
            .can_delegate_permission_sets(&user, &effective_permission_set_refs)
            .await?
    {
        return Err(ApiError::Forbidden(
            "Cannot create queue with execution permission sets beyond current access".to_string(),
        ));
    }

    let queue = WorkQueueRepository::create(
        &state.db,
        CreateWorkQueueInput {
            r#ref: request.r#ref,
            pack: pack_id,
            pack_ref,
            is_adhoc,
            label: request.label,
            description: request.description,
            enabled: request.enabled,
            accepting_new_items: request.accepting_new_items,
            dispatch_action: Some(action.id),
            dispatch_action_ref: action.r#ref,
            default_priority: request.default_priority,
            allow_pending_update: request.allow_pending_update,
            update_strategy: request.update_strategy,
            batch_mode: request.batch_mode,
            item_schema: request.item_schema,
            action_params: request.action_params,
            trace_tag_template,
            permission_set_refs: request.permission_set_refs,
            config: request.config,
            reference_visibility: request.reference_visibility.unwrap_or_default(),
            reference_allowed_pack_refs: request.reference_allowed_pack_refs,
        },
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            WorkQueueResponse::from(queue),
            "Work queue created successfully",
        )),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/queues/{ref}",
    tag = "queues",
    params(("ref" = String, Path, description = "Queue reference identifier")),
    request_body = UpdateWorkQueueRequest,
    responses(
        (status = 200, description = "Work queue updated successfully", body = ApiResponse<WorkQueueResponse>),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Insufficient permissions or pack-managed queue"),
        (status = 404, description = "Queue, pack, or dispatch action not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_queue(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(queue_ref): Path<String>,
    Json(request): Json<UpdateWorkQueueRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    let queue = WorkQueueRepository::find_by_ref(&state.db, &queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;

    authorize_queue_action(&state, &user, RbacAction::Update, &queue).await?;

    if !queue.is_adhoc {
        let mut has_non_operational_changes = false;
        has_non_operational_changes |= request.pack_ref.is_some();
        has_non_operational_changes |= request.label.is_some();
        has_non_operational_changes |= request.description.is_some();
        has_non_operational_changes |= request.dispatch_action_ref.is_some();
        has_non_operational_changes |= request.default_priority.is_some();
        has_non_operational_changes |= request.allow_pending_update.is_some();
        has_non_operational_changes |= request.update_strategy.is_some();
        has_non_operational_changes |= request.batch_mode.is_some();
        has_non_operational_changes |= request.item_schema.is_some();
        has_non_operational_changes |= request.action_params.is_some();
        has_non_operational_changes |= request.trace_tag_template.is_some();
        has_non_operational_changes |= request.permission_set_refs.is_some();
        has_non_operational_changes |= request.config.is_some();
        has_non_operational_changes |= request.reference_visibility.is_some();
        has_non_operational_changes |= request.reference_allowed_pack_refs.is_some();

        if has_non_operational_changes {
            return Err(ApiError::Forbidden(
                "Pack-managed queues may only change operational flags through the API".to_string(),
            ));
        }
    }

    let (pack_patch, pack_ref_patch, resolved_pack_ref) = match request.pack_ref.clone() {
        Some(NullableStringPatch::Set(pack_ref)) => {
            let (pack_id, resolved_pack_ref) = resolve_pack(&state, Some(&pack_ref)).await?;
            (
                pack_id.map(Patch::Set),
                resolved_pack_ref.clone().map(Patch::Set),
                resolved_pack_ref,
            )
        }
        Some(NullableStringPatch::Clear) => (Some(Patch::Clear), Some(Patch::Clear), None),
        None => (None, None, queue.pack_ref.clone()),
    };

    if request.pack_ref.is_some() {
        let identity_id = requester_identity_id(&user)?;
        if let Some(identity_id) = identity_id {
            let mut ctx = AuthorizationContext::new(identity_id);
            ctx.target_id = Some(queue.id);
            ctx.target_ref = Some(queue.r#ref.clone());
            ctx.pack_ref = resolved_pack_ref.clone();
            authorize_queue_context_action(&state, &user, RbacAction::Update, ctx).await?;
        }
    }

    let (dispatch_action_patch, dispatch_action_ref, loaded_dispatch_action) =
        if let Some(dispatch_action_ref) = request.dispatch_action_ref {
            let action = resolve_dispatch_action(&state, &dispatch_action_ref).await?;
            let r = action.r#ref.clone();
            (Some(Patch::Set(action.id)), Some(r), Some(action))
        } else {
            (None, None, None)
        };

    if dispatch_action_ref.is_some() || request.pack_ref.is_some() {
        let effective_action = match &loaded_dispatch_action {
            Some(action) => action.clone(),
            None => resolve_effective_dispatch_action(&state, &queue, None).await?,
        };
        ensure_action_reference_allowed(
            &effective_action,
            resolved_pack_ref.as_deref(),
            "work queue",
            &queue.r#ref,
        )?;
    }

    let permission_refs_to_validate = match &request.permission_set_refs {
        Some(Some(refs)) => Some(refs.clone()),
        Some(None) | None
            if request.permission_set_refs.is_some() || dispatch_action_ref.is_some() =>
        {
            // Need the effective action's defaults — reuse already-loaded action or fetch
            let effective_action = match &loaded_dispatch_action {
                Some(action) => action.clone(),
                None => resolve_effective_dispatch_action(&state, &queue, None).await?,
            };
            if request.permission_set_refs == Some(None) {
                // Explicit null = inherit from action
                Some(effective_action.default_execution_permission_set_refs)
            } else {
                // Dispatch action is changing — re-validate the effective permissions (either
                // the queue's explicit override or the new action's defaults) to prevent a user
                // from redirecting privileged permissions to an action they control.
                Some(
                    queue
                        .permission_set_refs
                        .clone()
                        .unwrap_or(effective_action.default_execution_permission_set_refs),
                )
            }
        }
        _ => None,
    };
    if let Some(permission_refs_to_validate) = permission_refs_to_validate {
        if !permission_refs_to_validate.is_empty()
            && !AuthorizationService::new(state.db.clone())
                .can_delegate_permission_sets(&user, &permission_refs_to_validate)
                .await?
        {
            return Err(ApiError::Forbidden(
                "Cannot update queue with execution permission sets beyond current access"
                    .to_string(),
            ));
        }
    }

    let effective_batch_mode = request.batch_mode.unwrap_or(queue.batch_mode);
    let effective_config = request.config.as_ref().unwrap_or(&queue.config);
    attune_common::queue_definition::validate_work_queue_config_for_batch_mode(
        effective_batch_mode,
        effective_config,
    )?;
    let effective_reference_visibility = request
        .reference_visibility
        .unwrap_or(queue.reference_visibility);
    let effective_reference_allowed_pack_refs = request
        .reference_allowed_pack_refs
        .clone()
        .unwrap_or_else(|| queue.reference_allowed_pack_refs.clone());
    attune_common::queue_definition::validate_queue_reference_visibility_config(
        effective_reference_visibility,
        &effective_reference_allowed_pack_refs,
    )?;

    let queue = WorkQueueRepository::update(
        &state.db,
        queue.id,
        UpdateWorkQueueInput {
            pack: pack_patch,
            pack_ref: pack_ref_patch,
            label: request.label,
            description: request.description.map(|patch| match patch {
                NullableStringPatch::Set(value) => Patch::Set(value),
                NullableStringPatch::Clear => Patch::Clear,
            }),
            enabled: request.enabled,
            accepting_new_items: request.accepting_new_items,
            dispatch_action: dispatch_action_patch,
            dispatch_action_ref,
            default_priority: request.default_priority,
            allow_pending_update: request.allow_pending_update,
            update_strategy: request.update_strategy,
            batch_mode: request.batch_mode,
            item_schema: request.item_schema,
            action_params: request.action_params,
            trace_tag_template: request.trace_tag_template.map(|template| match template {
                Some(template) => Patch::Set(template),
                None => Patch::Clear,
            }),
            permission_set_refs: request.permission_set_refs.map(|refs| match refs {
                Some(refs) => Patch::Set(refs),
                None => Patch::Clear,
            }),
            config: request.config,
            reference_visibility: request.reference_visibility,
            reference_allowed_pack_refs: request.reference_allowed_pack_refs,
            ..Default::default()
        },
    )
    .await?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            WorkQueueResponse::from(queue),
            "Work queue updated successfully",
        )),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/queues/{ref}",
    tag = "queues",
    params(("ref" = String, Path, description = "Queue reference identifier")),
    responses(
        (status = 200, description = "Work queue deleted successfully", body = SuccessResponse),
        (status = 403, description = "Insufficient permissions or pack-managed queue"),
        (status = 404, description = "Queue not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_queue(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(queue_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let queue = WorkQueueRepository::find_by_ref(&state.db, &queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;

    authorize_queue_action(&state, &user, RbacAction::Delete, &queue).await?;

    if !queue.is_adhoc {
        return Err(ApiError::Forbidden(
            "Pack-managed queues must be removed from pack queue definition files".to_string(),
        ));
    }

    let deleted = WorkQueueRepository::delete(&state.db, queue.id).await?;
    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Work queue '{}' not found",
            queue_ref
        )));
    }

    Ok((
        StatusCode::OK,
        Json(SuccessResponse::new(format!(
            "Work queue '{}' deleted successfully",
            queue_ref
        ))),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/queues/{ref}/items",
    tag = "queues",
    params(
        ("ref" = String, Path, description = "Queue reference identifier"),
        WorkQueueItemQueryParams
    ),
    responses(
        (status = 200, description = "List of queue items", body = PaginatedResponse<WorkQueueItemResponse>),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Queue not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_queue_items(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(queue_ref): Path<String>,
    FormQuery(query): FormQuery<WorkQueueItemQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let queue = WorkQueueRepository::find_by_ref(&state.db, &queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;

    authorize_queue_item_action(&state, &user, RbacAction::Read, &queue).await?;
    ensure_queue_item_visibility(&state, &user, RbacAction::Read, &queue).await?;

    let result = WorkQueueItemRepository::search(
        &state.db,
        &WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            item_key: query.item_key.clone(),
            enqueue_source: query.enqueue_source.clone(),
            statuses: (!query.statuses.is_empty()).then_some(query.statuses.clone()),
            limit: query.limit(),
            offset: query.offset(),
            ..Default::default()
        },
    )
    .await?;

    Ok((
        StatusCode::OK,
        Json(PaginatedResponse::new(
            result
                .rows
                .into_iter()
                .map(WorkQueueItemResponse::from)
                .collect(),
            &PaginationParams {
                page: query.page,
                page_size: query.per_page,
            },
            result.total,
        )),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/queues/{ref}/items",
    tag = "queues",
    params(("ref" = String, Path, description = "Queue reference identifier")),
    request_body = EnqueueWorkQueueItemRequest,
    responses(
        (status = 200, description = "Pending queue item updated", body = ApiResponse<WorkQueueItemResponse>),
        (status = 201, description = "Queue item enqueued", body = ApiResponse<WorkQueueItemResponse>),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Queue not found"),
        (status = 409, description = "Pending item conflict")
    ),
    security(("bearer_auth" = []))
)]
pub async fn enqueue_queue_item(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(queue_ref): Path<String>,
    Json(request): Json<EnqueueWorkQueueItemRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    let queue = WorkQueueRepository::find_by_ref(&state.db, &queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;

    authorize_queue_item_action(&state, &user, RbacAction::Create, &queue).await?;
    ensure_queue_item_visibility(&state, &user, RbacAction::Create, &queue).await?;
    if !queue.accepting_new_items {
        return Err(ApiError::Conflict(format!(
            "Work queue '{}' is not accepting new items",
            queue_ref
        )));
    }
    validate_queue_item_payload(&queue, &request.payload)?;

    let requested_by_identity = requester_identity_id(&user)?;
    let requested_by_execution = user.execution_id();
    let create_priority = request.priority.unwrap_or(queue.default_priority);
    let mutable_statuses = WorkQueueItemRepository::mutable_pending_statuses();
    let explicit_trace_tag = request
        .trace_tag
        .as_ref()
        .map(|value| normalize_trace_tag(value))
        .transpose()
        .map_err(|e| ApiError::BadRequest(format!("Invalid trace_tag: {e}")))?;
    let inherited_trace_tag = if explicit_trace_tag.is_none() {
        match requested_by_execution {
            Some(execution_id) => ExecutionRepository::find_by_id(&state.db, execution_id)
                .await?
                .and_then(|execution| execution.trace_tag),
            None => None,
        }
    } else {
        None
    };
    let source_trace_tag = explicit_trace_tag.or(inherited_trace_tag);
    let source_trace_patch = source_trace_tag.clone().map(Patch::Set);

    if let Some(item_key) = request.item_key.as_deref() {
        if queue.allow_pending_update {
            let pending =
                WorkQueueItemRepository::find_pending_by_item_key(&state.db, queue.id, item_key)
                    .await?;
            if pending.len() > 1 {
                return Err(ApiError::Conflict(format!(
                    "Queue '{}' has multiple mutable pending items for item_key '{}'",
                    queue.r#ref, item_key
                )));
            }

            if let Some(existing) = pending.into_iter().next() {
                let updated = match queue.update_strategy {
                    attune_common::models::WorkQueueUpdateStrategy::Immutable => {
                        return Err(ApiError::Conflict(format!(
                            "Pending queue item with key '{}' already exists in queue '{}'",
                            item_key, queue.r#ref
                        )));
                    }
                    attune_common::models::WorkQueueUpdateStrategy::Replace => {
                        WorkQueueItemRepository::update_if_statuses(
                            &state.db,
                            existing.id,
                            mutable_statuses,
                            UpdateWorkQueueItemInput {
                                priority: Some(create_priority),
                                payload: Some(request.payload.clone()),
                                metadata: Some(request.metadata.clone()),
                                trace_tag: source_trace_patch.clone(),
                                ..Default::default()
                            },
                        )
                        .await?
                    }
                    attune_common::models::WorkQueueUpdateStrategy::MergePatch => {
                        let merged_payload = merge_patch_value(existing.payload, &request.payload);
                        validate_queue_item_payload(&queue, &merged_payload)?;
                        let merged_metadata =
                            merge_patch_value(existing.metadata, &request.metadata);
                        WorkQueueItemRepository::update_if_statuses(
                            &state.db,
                            existing.id,
                            mutable_statuses,
                            UpdateWorkQueueItemInput {
                                priority: request.priority,
                                payload: Some(merged_payload),
                                metadata: Some(merged_metadata),
                                trace_tag: source_trace_patch.clone(),
                                ..Default::default()
                            },
                        )
                        .await?
                    }
                }
                .ok_or_else(|| {
                    ApiError::Conflict(
                        "Queue item is no longer in a mutable pending state".to_string(),
                    )
                })?;

                return Ok((
                    StatusCode::OK,
                    Json(ApiResponse::with_message(
                        WorkQueueItemResponse::from(updated),
                        "Pending queue item updated successfully",
                    )),
                ));
            }
        }
    }

    let item = WorkQueueItemRepository::enqueue(
        &state.db,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: request.item_key,
            priority: create_priority,
            status: attune_common::models::WorkQueueItemStatus::Queued,
            payload: request.payload,
            metadata: request.metadata,
            trace_tag: source_trace_tag,
            enqueue_source: API_ENQUEUE_SOURCE.to_string(),
            requested_by_identity,
            requested_by_execution,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::with_message(
            WorkQueueItemResponse::from(item),
            "Queue item enqueued successfully",
        )),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/queues/{ref}/items/query/preview",
    tag = "queues",
    params(("ref" = String, Path, description = "Queue reference identifier")),
    request_body = PreviewWorkQueueItemsRequest,
    responses(
        (status = 200, description = "Preview queue items selected by a SQL/JSONPath selector", body = ApiResponse<PreviewWorkQueueItemsResponse>),
        (status = 400, description = "Validation error or invalid SQL/JSONPath selector"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Queue not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn preview_queue_items_by_selector(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(queue_ref): Path<String>,
    Json(request): Json<PreviewWorkQueueItemsRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;
    validate_selector_request(&request.selector)?;

    let queue = WorkQueueRepository::find_by_ref(&state.db, &queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;

    authorize_queue_item_action(&state, &user, RbacAction::Read, &queue).await?;
    ensure_queue_item_visibility(&state, &user, RbacAction::Read, &queue).await?;

    let selector = selector_from_request(&request.selector);
    let result = map_selector_result(
        WorkQueueItemRepository::preview_selected_mutable(
            &state.db,
            queue.id,
            &selector,
            request.limit.min(100),
        )
        .await,
    )?;
    let items: Vec<_> = result
        .rows
        .into_iter()
        .map(WorkQueueItemResponse::from)
        .collect();

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(PreviewWorkQueueItemsResponse {
            matched_count: result.total,
            preview_count: items.len(),
            items,
        })),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/queues/{ref}/items/query/apply",
    tag = "queues",
    params(("ref" = String, Path, description = "Queue reference identifier")),
    request_body = ApplyWorkQueueItemsRequest,
    responses(
        (status = 200, description = "Bulk queue item operation applied", body = ApiResponse<ApplyWorkQueueItemsResponse>),
        (status = 400, description = "Validation error or invalid SQL/JSONPath selector"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Queue not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn apply_queue_items_by_selector(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(queue_ref): Path<String>,
    Json(request): Json<ApplyWorkQueueItemsRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;
    validate_selector_request(&request.selector)?;
    validate_bulk_apply_request(&request)?;

    let queue = WorkQueueRepository::find_by_ref(&state.db, &queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;

    let required_action = match request.operation {
        WorkQueueItemBulkOperation::Cancel => RbacAction::Delete,
        WorkQueueItemBulkOperation::PatchPayload | WorkQueueItemBulkOperation::Reprioritize => {
            RbacAction::Update
        }
    };
    authorize_queue_item_action(&state, &user, required_action, &queue).await?;
    ensure_queue_item_visibility(&state, &user, required_action, &queue).await?;

    let selector = selector_from_request(&request.selector);
    let preview = map_selector_result(
        WorkQueueItemRepository::preview_selected_mutable(
            &state.db,
            queue.id,
            &selector,
            request.preview_limit.min(100),
        )
        .await,
    )?;
    let matched_count = preview.total;
    let preview_items: Vec<_> = preview
        .rows
        .into_iter()
        .map(WorkQueueItemResponse::from)
        .collect();

    let affected_count = match request.operation {
        WorkQueueItemBulkOperation::Cancel => {
            let mut tx = state.db.begin().await?;
            let affected = map_selector_result(
                WorkQueueItemRepository::cancel_selected_mutable(&mut *tx, queue.id, &selector)
                    .await,
            )?;
            tx.commit().await?;
            affected
        }
        WorkQueueItemBulkOperation::Reprioritize => {
            let priority = request
                .priority
                .expect("priority is validated for reprioritize operations");
            let mut tx = state.db.begin().await?;
            let affected = map_selector_result(
                WorkQueueItemRepository::reprioritize_selected_mutable(
                    &mut *tx, queue.id, &selector, priority,
                )
                .await,
            )?;
            tx.commit().await?;
            affected
        }
        WorkQueueItemBulkOperation::PatchPayload => {
            let patch = request
                .payload_patch
                .as_ref()
                .expect("payload_patch is validated for patch operations");
            apply_payload_patch_to_selected_items(&state, &queue, &selector, patch).await?
        }
    };

    let skipped_count = matched_count.saturating_sub(affected_count);
    emit_queue_items_bulk_audit(
        &state,
        &user,
        &queue,
        &request,
        matched_count,
        affected_count,
        skipped_count,
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            ApplyWorkQueueItemsResponse {
                operation: request.operation,
                matched_count,
                affected_count,
                skipped_count,
                preview_count: preview_items.len(),
                items: preview_items,
            },
            "Bulk queue item operation applied successfully",
        )),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/queues/{ref}/items/{item_id}",
    tag = "queues",
    params(
        ("ref" = String, Path, description = "Queue reference identifier"),
        ("item_id" = i64, Path, description = "Queue item identifier")
    ),
    request_body = UpdateWorkQueueItemRequest,
    responses(
        (status = 200, description = "Queue item updated", body = ApiResponse<WorkQueueItemResponse>),
        (status = 400, description = "Validation error"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Queue or queue item not found"),
        (status = 409, description = "Queue item is not mutable")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_queue_item(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((queue_ref, item_id)): Path<(String, i64)>,
    Json(request): Json<UpdateWorkQueueItemRequest>,
) -> ApiResult<impl IntoResponse> {
    request.validate()?;

    let queue = WorkQueueRepository::find_by_ref(&state.db, &queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;

    authorize_queue_item_action(&state, &user, RbacAction::Update, &queue).await?;
    ensure_queue_item_visibility(&state, &user, RbacAction::Update, &queue).await?;

    let item = WorkQueueItemRepository::find_by_queue_and_id(&state.db, queue.id, item_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Queue item '{}' not found", item_id)))?;

    if !WorkQueueItemRepository::is_mutable_pending_status(item.status) {
        return Err(ApiError::Conflict(
            "Only queued or retry queue items can be updated".to_string(),
        ));
    }

    if let Some(payload) = &request.payload {
        validate_queue_item_payload(&queue, payload)?;
    }

    let updated = WorkQueueItemRepository::update_if_statuses(
        &state.db,
        item.id,
        WorkQueueItemRepository::mutable_pending_statuses(),
        UpdateWorkQueueItemInput {
            item_key: request.item_key.map(|patch| match patch {
                NullableStringPatch::Set(value) => Patch::Set(value),
                NullableStringPatch::Clear => Patch::Clear,
            }),
            priority: request.priority,
            payload: request.payload,
            metadata: request.metadata,
            ..Default::default()
        },
    )
    .await?
    .ok_or_else(|| {
        ApiError::Conflict("Queue item is no longer in a mutable pending state".to_string())
    })?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::with_message(
            WorkQueueItemResponse::from(updated),
            "Queue item updated successfully",
        )),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/queues/{ref}/items/{item_id}",
    tag = "queues",
    params(
        ("ref" = String, Path, description = "Queue reference identifier"),
        ("item_id" = i64, Path, description = "Queue item identifier")
    ),
    responses(
        (status = 200, description = "Queue item deleted", body = SuccessResponse),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Queue or queue item not found"),
        (status = 409, description = "Queue item is not mutable")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_queue_item(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((queue_ref, item_id)): Path<(String, i64)>,
) -> ApiResult<impl IntoResponse> {
    let queue = WorkQueueRepository::find_by_ref(&state.db, &queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;

    authorize_queue_item_action(&state, &user, RbacAction::Delete, &queue).await?;
    ensure_queue_item_visibility(&state, &user, RbacAction::Delete, &queue).await?;

    let item = WorkQueueItemRepository::find_by_queue_and_id(&state.db, queue.id, item_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Queue item '{}' not found", item_id)))?;

    if !WorkQueueItemRepository::is_mutable_pending_status(item.status) {
        return Err(ApiError::Conflict(
            "Only queued or retry queue items can be deleted".to_string(),
        ));
    }

    let deleted = WorkQueueItemRepository::delete_if_statuses(
        &state.db,
        item.id,
        WorkQueueItemRepository::mutable_pending_statuses(),
    )
    .await?;
    if !deleted {
        return Err(ApiError::Conflict(
            "Queue item is no longer in a mutable pending state".to_string(),
        ));
    }

    Ok((
        StatusCode::OK,
        Json(SuccessResponse::new(format!(
            "Queue item '{}' deleted successfully",
            item_id
        ))),
    ))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/queues", get(list_queues).post(create_queue))
        .route(
            "/queues/{ref}",
            get(get_queue).put(update_queue).delete(delete_queue),
        )
        .route("/packs/{pack_ref}/queues", get(list_queues_by_pack))
        .route(
            "/queues/{ref}/items",
            get(list_queue_items).post(enqueue_queue_item),
        )
        .route(
            "/queues/{ref}/items/query/preview",
            axum::routing::post(preview_queue_items_by_selector),
        )
        .route(
            "/queues/{ref}/items/query/apply",
            axum::routing::post(apply_queue_items_by_selector),
        )
        .route(
            "/queues/{ref}/items/{item_id}",
            axum::routing::put(update_queue_item).delete(delete_queue_item),
        )
}

async fn resolve_pack(
    state: &Arc<AppState>,
    pack_ref: Option<&str>,
) -> ApiResult<(Option<i64>, Option<String>)> {
    let Some(pack_ref) = pack_ref else {
        return Ok((None, None));
    };

    let pack = PackRepository::find_by_ref(&state.db, pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    Ok((Some(pack.id), Some(pack.r#ref)))
}

async fn resolve_dispatch_action(
    state: &Arc<AppState>,
    action_ref: &str,
) -> ApiResult<attune_common::models::action::Action> {
    ActionRepository::find_by_ref(&state.db, action_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", action_ref)))
}

async fn resolve_effective_dispatch_action(
    state: &Arc<AppState>,
    queue: &attune_common::models::WorkQueue,
    updated_action_ref: Option<&str>,
) -> ApiResult<attune_common::models::action::Action> {
    if let Some(updated_action_ref) = updated_action_ref {
        return resolve_dispatch_action(state, updated_action_ref).await;
    }

    if let Some(action_id) = queue.dispatch_action {
        if let Some(action) = ActionRepository::find_by_id(&state.db, action_id).await? {
            return Ok(action);
        }
    }

    resolve_dispatch_action(state, &queue.dispatch_action_ref).await
}

async fn resolve_queue_dispatch_tuning(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queue: &attune_common::models::WorkQueue,
) -> Result<Option<ResolvedWorkQueueDispatchTuningResponse>, ApiError> {
    let parsed_config: WorkQueueConfig =
        serde_json::from_value(queue.config.clone()).map_err(|e| {
            ApiError::InternalServerError(format!(
                "Invalid persisted queue config for '{}': {}",
                queue.r#ref, e
            ))
        })?;

    let pack = if let Some(pack_id) = queue.pack {
        PackRepository::find_by_id(&state.db, pack_id).await?
    } else if let Some(pack_ref) = &queue.pack_ref {
        PackRepository::find_by_ref(&state.db, pack_ref).await?
    } else {
        None
    };

    let identity_id = if user.claims.token_type == TokenType::Access {
        Some(
            user.identity_id()
                .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?,
        )
    } else {
        None
    };

    let concurrency = resolve_u32_tunable_for_display(
        state,
        user,
        identity_id,
        pack.as_ref(),
        parsed_config
            .dispatch
            .as_ref()
            .and_then(|dispatch| dispatch.concurrency.as_ref()),
        1,
    )
    .await?;

    let batch_size = if queue.batch_mode == WorkQueueBatchMode::Single {
        Some(1)
    } else {
        resolve_u32_tunable_for_display(
            state,
            user,
            identity_id,
            pack.as_ref(),
            parsed_config
                .dispatch
                .as_ref()
                .and_then(|dispatch| dispatch.batch_size.as_ref()),
            1,
        )
        .await?
    };

    Ok(Some(ResolvedWorkQueueDispatchTuningResponse {
        concurrency,
        batch_size,
    }))
}

async fn resolve_u32_tunable_for_display(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    identity_id: Option<i64>,
    pack: Option<&Pack>,
    tunable: Option<&WorkQueueTunableValue>,
    default: u32,
) -> Result<Option<u32>, ApiError> {
    let Some(tunable) = tunable else {
        return Ok(Some(default));
    };

    match resolve_tunable_value_for_display(state, user, identity_id, pack, tunable).await? {
        DisplayTunableValue::Restricted => Ok(None),
        DisplayTunableValue::Resolved(value) => Ok(value
            .as_ref()
            .and_then(parse_positive_u32)
            .or_else(|| tunable.fallback.as_ref().and_then(parse_positive_u32))
            .or(Some(default))),
    }
}

enum DisplayTunableValue {
    Resolved(Option<JsonValue>),
    Restricted,
}

async fn resolve_tunable_value_for_display(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    identity_id: Option<i64>,
    pack: Option<&Pack>,
    tunable: &WorkQueueTunableValue,
) -> Result<DisplayTunableValue, ApiError> {
    use attune_common::models::WorkQueueTunableSource;

    let value = match tunable.source {
        WorkQueueTunableSource::Literal => tunable.value.clone(),
        WorkQueueTunableSource::PackConfig => pack.and_then(|pack| {
            tunable
                .path
                .as_deref()
                .and_then(|path| json_path_get(&pack.config, path).cloned())
        }),
        WorkQueueTunableSource::Keystore => {
            let Some(key_ref) = tunable.key_ref.as_deref() else {
                return Ok(DisplayTunableValue::Resolved(tunable.fallback.clone()));
            };
            let Some(key) = KeyRepository::find_by_ref(&state.db, key_ref).await? else {
                return Ok(DisplayTunableValue::Resolved(None));
            };

            let Some(value) = resolve_display_key_value(state, user, identity_id, &key).await?
            else {
                return Ok(DisplayTunableValue::Restricted);
            };

            if let Some(path) = tunable.path.as_deref() {
                json_path_get(&value, path).cloned()
            } else {
                Some(value)
            }
        }
    };

    Ok(DisplayTunableValue::Resolved(
        value.or_else(|| tunable.fallback.clone()),
    ))
}

async fn resolve_display_key_value(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    identity_id: Option<i64>,
    key: &Key,
) -> Result<Option<JsonValue>, ApiError> {
    if user.claims.token_type != TokenType::Access {
        return decrypt_key_for_display(state, key);
    }

    let Some(identity_id) = identity_id else {
        return Ok(None);
    };
    let authz = AuthorizationService::new(state.db.clone());
    let context = key_authorization_context(identity_id, key);

    if authz
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Keys,
                action: RbacAction::Read,
                context: context.clone(),
            },
        )
        .await
        .is_err()
    {
        return Ok(None);
    }

    if key.encrypted
        && authz
            .authorize(
                user,
                AuthorizationCheck {
                    resource: Resource::Keys,
                    action: RbacAction::Decrypt,
                    context,
                },
            )
            .await
            .is_err()
    {
        return Ok(None);
    }

    decrypt_key_for_display(state, key)
}

fn decrypt_key_for_display(
    state: &Arc<AppState>,
    key: &Key,
) -> Result<Option<JsonValue>, ApiError> {
    if !key.encrypted {
        return Ok(Some(key.value.clone()));
    }

    let encryption_key = state
        .config
        .security
        .encryption_key
        .as_ref()
        .ok_or_else(|| {
            ApiError::InternalServerError("Encryption key not configured on server".to_string())
        })?;
    let decrypted =
        attune_common::crypto::decrypt_json(&key.value, encryption_key).map_err(|e| {
            ApiError::InternalServerError(format!("Failed to decrypt key '{}': {}", key.r#ref, e))
        })?;

    Ok(Some(decrypted))
}

fn parse_positive_u32(value: &JsonValue) -> Option<u32> {
    match value {
        JsonValue::Number(number) => number
            .as_u64()
            .or_else(|| number.as_i64().and_then(|value| u64::try_from(value).ok()))
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0),
        JsonValue::String(value) => value.parse::<u32>().ok().filter(|value| *value > 0),
        _ => None,
    }
}

fn json_path_get<'a>(value: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    let mut current = value;
    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        current = match current {
            JsonValue::Object(object) => object.get(segment)?,
            JsonValue::Array(array) => {
                let index = segment.parse::<usize>().ok()?;
                array.get(index)?
            }
            _ => return None,
        };
    }
    Some(current)
}

fn key_authorization_context(identity_id: i64, key: &Key) -> AuthorizationContext {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_id = Some(key.id);
    ctx.target_ref = Some(key.r#ref.clone());
    ctx.owner_identity_id = key.owner_identity;
    ctx.owner_type = Some(key.owner_type);
    ctx.owner_ref = match key.owner_type {
        attune_common::models::OwnerType::Pack => key.owner_pack_ref.clone(),
        attune_common::models::OwnerType::Action => key.owner_action_ref.clone(),
        attune_common::models::OwnerType::Sensor => key.owner_sensor_ref.clone(),
        _ => key.owner.clone(),
    };
    ctx.encrypted = Some(key.encrypted);
    ctx
}

async fn authorize_queue_action(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: RbacAction,
    queue: &attune_common::models::WorkQueue,
) -> Result<(), ApiError> {
    if user.claims.token_type != TokenType::Access {
        return Ok(());
    }

    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    authorize_queue_context_action(
        state,
        user,
        action,
        queue_authorization_context(identity_id, queue),
    )
    .await
}

async fn authorize_queue_item_action(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: RbacAction,
    queue: &attune_common::models::WorkQueue,
) -> Result<(), ApiError> {
    let authz = AuthorizationService::new(state.db.clone());
    let context = queue_authorization_context_for_token(user, queue)?;
    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return Ok(());
    }

    let grants = authz.effective_grants(user).await?;
    if AuthorizationService::is_allowed(&grants, Resource::QueueItems, action, &context)
        || AuthorizationService::is_allowed(&grants, Resource::Queues, action, &context)
    {
        return Ok(());
    }

    authz
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::QueueItems,
                action,
                context,
            },
        )
        .await
}

async fn filter_visible_queues_for_discovery(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queues: Vec<attune_common::models::WorkQueue>,
    referencing_pack_ref: Option<&str>,
) -> Result<Vec<attune_common::models::WorkQueue>, ApiError> {
    let mut visible = Vec::with_capacity(queues.len());
    for queue in queues {
        if queue_visible_for_discovery(state, user, &queue, referencing_pack_ref).await? {
            visible.push(queue);
        }
    }
    Ok(visible)
}

async fn queue_visible_for_discovery(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queue: &attune_common::models::WorkQueue,
    referencing_pack_ref: Option<&str>,
) -> Result<bool, ApiError> {
    if queue_reference_allowed(queue, referencing_pack_ref) {
        return Ok(true);
    }

    has_queue_management_access(state, user, queue).await
}

/// Determine whether the caller may see items belonging to the given queue,
/// applying the same per-queue visibility rules used by queue-item endpoints.
pub(crate) async fn queue_item_visible(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: RbacAction,
    queue: &attune_common::models::WorkQueue,
) -> Result<bool, ApiError> {
    Ok(queue_reference_allowed(queue, None)
        || execution_caller_pack_refs(user)
            .iter()
            .any(|pack_ref| queue_reference_allowed(queue, Some(pack_ref)))
        || constrained_queue_item_grant_allowed(state, user, action, queue).await?
        || has_queue_management_access(state, user, queue).await?)
}

async fn ensure_queue_item_visibility(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: RbacAction,
    queue: &attune_common::models::WorkQueue,
) -> Result<(), ApiError> {
    if queue_item_visible(state, user, action, queue).await? {
        return Ok(());
    }

    Err(ApiError::Forbidden(format!(
        "Queue '{}' is not visible to this caller for item operations",
        queue.r#ref
    )))
}

async fn constrained_queue_item_grant_allowed(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: RbacAction,
    queue: &attune_common::models::WorkQueue,
) -> Result<bool, ApiError> {
    let authz = AuthorizationService::new(state.db.clone());
    let grants = authz.effective_grants(user).await?;
    let ctx = queue_authorization_context_for_token(user, queue)?;

    Ok(grants.iter().any(|grant| {
        grant.resource == Resource::QueueItems
            && grant.actions.contains(&action)
            && grant
                .constraints
                .as_ref()
                .is_some_and(|constraints| queue_constraints_match(constraints, queue))
            && grant.allows(Resource::QueueItems, action, &ctx)
    }))
}

async fn has_queue_management_access(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queue: &attune_common::models::WorkQueue,
) -> Result<bool, ApiError> {
    let authz = AuthorizationService::new(state.db.clone());
    let grants = authz.effective_grants(user).await?;
    let ctx = queue_authorization_context_for_token(user, queue)?;

    Ok([RbacAction::Update, RbacAction::Delete]
        .into_iter()
        .any(|action| AuthorizationService::is_allowed(&grants, Resource::Queues, action, &ctx)))
}

fn queue_constraints_match(
    constraints: &attune_common::rbac::GrantConstraints,
    queue: &attune_common::models::WorkQueue,
) -> bool {
    constraints
        .ids
        .as_ref()
        .is_some_and(|ids| ids.contains(&queue.id))
        || constraints
            .refs
            .as_ref()
            .is_some_and(|refs| refs.contains(&queue.r#ref))
        || constraints.pack_refs.as_ref().is_some_and(|pack_refs| {
            queue
                .pack_ref
                .as_ref()
                .is_some_and(|pack_ref| pack_refs.contains(pack_ref))
        })
}

async fn authorize_queue_create(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queue_ref: &str,
    pack_ref: Option<&str>,
) -> Result<(), ApiError> {
    if user.claims.token_type != TokenType::Access {
        return Ok(());
    }

    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_ref = Some(queue_ref.to_string());
    ctx.pack_ref = pack_ref.map(str::to_string);
    authorize_queue_context_action(state, user, RbacAction::Create, ctx).await
}

async fn authorize_queue_context_action(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: RbacAction,
    context: AuthorizationContext,
) -> Result<(), ApiError> {
    let authz = AuthorizationService::new(state.db.clone());
    authz
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Queues,
                action,
                context,
            },
        )
        .await
}

async fn ensure_can_read_any_queue(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
) -> Result<Option<(i64, Vec<attune_common::rbac::Grant>)>, ApiError> {
    if user.claims.token_type != TokenType::Access {
        return Ok(None);
    }

    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let authz = AuthorizationService::new(state.db.clone());
    let grants = authz.effective_grants(user).await?;

    let can_read_any_queue = grants
        .iter()
        .any(|g| g.resource == Resource::Queues && g.actions.contains(&RbacAction::Read));
    if !can_read_any_queue {
        return Err(ApiError::Forbidden(
            "Insufficient permissions: queues:read".to_string(),
        ));
    }

    Ok(Some((identity_id, grants)))
}

fn queue_authorization_context(
    identity_id: i64,
    queue: &attune_common::models::WorkQueue,
) -> AuthorizationContext {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_id = Some(queue.id);
    ctx.target_ref = Some(queue.r#ref.clone());
    ctx.pack_ref = queue.pack_ref.clone();
    ctx
}

fn queue_authorization_context_for_token(
    user: &AuthenticatedUser,
    queue: &attune_common::models::WorkQueue,
) -> Result<AuthorizationContext, ApiError> {
    let identity_id = match user.claims.token_type {
        TokenType::Access | TokenType::Execution => user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?,
        _ => 0,
    };
    Ok(queue_authorization_context(identity_id, queue))
}

fn execution_caller_pack_refs(user: &AuthenticatedUser) -> Vec<String> {
    if user.claims.token_type != TokenType::Execution {
        return Vec::new();
    }

    let mut refs = execution_standard_pack_refs(user);
    if let Some(action_pack_ref) = user
        .claims
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("action_ref"))
        .and_then(|value| value.as_str())
        .and_then(|action_ref| action_ref.split_once('.').map(|(pack_ref, _)| pack_ref))
    {
        refs.push(action_pack_ref.to_string());
    }
    refs.sort();
    refs.dedup();
    refs
}

fn requester_identity_id(user: &AuthenticatedUser) -> Result<Option<i64>, ApiError> {
    if user.claims.token_type != TokenType::Access {
        return Ok(None);
    }

    user.identity_id()
        .map(Some)
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))
}

fn selector_from_request(
    selector: &crate::dto::work_queue::WorkQueueItemJsonPathSelector,
) -> WorkQueueItemJsonPathSelector {
    WorkQueueItemJsonPathSelector {
        path: selector.path.trim().to_string(),
        vars: selector.vars.clone(),
    }
}

fn validate_selector_request(
    selector: &crate::dto::work_queue::WorkQueueItemJsonPathSelector,
) -> Result<(), ApiError> {
    if selector.path.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "selector.path cannot be empty".to_string(),
        ));
    }
    if !selector.vars.is_object() {
        return Err(ApiError::BadRequest(
            "selector.vars must be a JSON object".to_string(),
        ));
    }
    Ok(())
}

fn validate_bulk_apply_request(request: &ApplyWorkQueueItemsRequest) -> Result<(), ApiError> {
    match request.operation {
        WorkQueueItemBulkOperation::Cancel => {
            if request.payload_patch.is_some() || request.priority.is_some() {
                return Err(ApiError::BadRequest(
                    "cancel operations must not include payload_patch or priority".to_string(),
                ));
            }
        }
        WorkQueueItemBulkOperation::PatchPayload => {
            let Some(patch) = &request.payload_patch else {
                return Err(ApiError::BadRequest(
                    "patch_payload operations require payload_patch".to_string(),
                ));
            };
            if !patch.is_object() {
                return Err(ApiError::BadRequest(
                    "payload_patch must be a JSON object".to_string(),
                ));
            }
            if request.priority.is_some() {
                return Err(ApiError::BadRequest(
                    "patch_payload operations must not include priority".to_string(),
                ));
            }
        }
        WorkQueueItemBulkOperation::Reprioritize => {
            if request.priority.is_none() {
                return Err(ApiError::BadRequest(
                    "reprioritize operations require priority".to_string(),
                ));
            }
            if request.payload_patch.is_some() {
                return Err(ApiError::BadRequest(
                    "reprioritize operations must not include payload_patch".to_string(),
                ));
            }
        }
    }
    Ok(())
}

async fn apply_payload_patch_to_selected_items(
    state: &Arc<AppState>,
    queue: &attune_common::models::WorkQueue,
    selector: &WorkQueueItemJsonPathSelector,
    patch: &JsonValue,
) -> Result<u64, ApiError> {
    let mut tx = state.db.begin().await?;
    let selected = map_selector_result(
        WorkQueueItemRepository::list_selected_mutable(&mut *tx, queue.id, selector).await,
    )?;

    let mut updates = Vec::with_capacity(selected.len());
    for item in selected {
        let merged_payload = merge_patch_value(item.payload, patch);
        validate_queue_item_payload(queue, &merged_payload)?;
        updates.push((item.id, merged_payload));
    }

    let mut affected_count = 0_u64;
    for (item_id, merged_payload) in updates {
        let updated = WorkQueueItemRepository::update_if_statuses(
            &mut *tx,
            item_id,
            WorkQueueItemRepository::mutable_pending_statuses(),
            UpdateWorkQueueItemInput {
                payload: Some(merged_payload),
                ..Default::default()
            },
        )
        .await?;
        if updated.is_some() {
            affected_count += 1;
        }
    }

    tx.commit().await?;
    Ok(affected_count)
}

fn map_selector_result<T>(result: attune_common::Result<T>) -> Result<T, ApiError> {
    result.map_err(|err| match err {
        attune_common::Error::Database(sqlx::Error::Database(db_err))
            if is_jsonpath_database_error(db_err.as_ref()) =>
        {
            ApiError::BadRequest(format!(
                "Invalid SQL/JSONPath queue item selector: {}",
                db_err.message()
            ))
        }
        other => other.into(),
    })
}

fn is_jsonpath_database_error(error: &dyn sqlx::error::DatabaseError) -> bool {
    let code = error
        .code()
        .map(|code| code.to_string())
        .unwrap_or_default();
    matches!(
        code.as_str(),
        "42601" | "22023" | "2203A" | "2203B" | "2203C" | "2203D" | "2203E" | "2203F"
    ) || error.message().to_ascii_lowercase().contains("jsonpath")
        || error.message().to_ascii_lowercase().contains("json path")
}

fn emit_queue_items_bulk_audit(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queue: &attune_common::models::WorkQueue,
    request: &ApplyWorkQueueItemsRequest,
    matched_count: u64,
    affected_count: u64,
    skipped_count: u64,
) {
    use attune_common::audit::{AuditCategory, AuditEventBuilder, AuditOutcome};

    let mut builder = AuditEventBuilder::new(
        AuditCategory::Admin,
        "queue_items.bulk_operation.applied",
        AuditOutcome::Success,
    )
    .resource("work_queue")
    .resource_id(queue.id)
    .resource_ref(queue.r#ref.clone())
    .actor_login(user.login().to_string())
    .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase())
    .with_details(serde_json::json!({
        "operation": request.operation,
        "selector_path": request.selector.path,
        "selector_vars": request.selector.vars,
        "matched_count": matched_count,
        "affected_count": affected_count,
        "skipped_count": skipped_count,
    }));

    if let Ok(identity_id) = user.identity_id() {
        builder = builder.actor_identity(identity_id);
    }

    state.audit_emitter.emit(builder.build());
}

fn paginate_rows<T>(rows: Vec<T>, page: u32, per_page: u32) -> Vec<T> {
    let offset = page.saturating_sub(1) as usize * per_page as usize;
    let limit = per_page as usize;

    rows.into_iter().skip(offset).take(limit).collect()
}

fn merge_patch_value(current: JsonValue, patch: &JsonValue) -> JsonValue {
    let mut merged = current;
    apply_merge_patch(&mut merged, patch);
    merged
}

fn apply_merge_patch(target: &mut JsonValue, patch: &JsonValue) {
    match patch {
        JsonValue::Object(patch_map) => {
            if !target.is_object() {
                *target = JsonValue::Object(Map::new());
            }

            let target_map = target
                .as_object_mut()
                .expect("target should be an object after initialization");
            for (key, value) in patch_map {
                if value.is_null() {
                    target_map.remove(key);
                } else {
                    apply_merge_patch(
                        target_map.entry(key.clone()).or_insert(JsonValue::Null),
                        value,
                    );
                }
            }
        }
        _ => *target = patch.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{apply_merge_patch, paginate_rows, routes};

    #[test]
    fn test_work_queue_routes_structure() {
        let _router = routes();
    }

    #[test]
    fn test_apply_merge_patch_uses_json_merge_patch_semantics() {
        let mut value = json!({
            "nested": {"keep": true, "remove": 1},
            "replace": 1
        });

        apply_merge_patch(
            &mut value,
            &json!({
                "nested": {"remove": null, "added": "yes"},
                "replace": {"now": "object"}
            }),
        );

        assert_eq!(
            value,
            json!({
                "nested": {"keep": true, "added": "yes"},
                "replace": {"now": "object"}
            })
        );
    }

    #[test]
    fn test_paginate_rows_returns_requested_page() {
        let rows = vec![1, 2, 3, 4, 5];

        assert_eq!(paginate_rows(rows, 2, 2), vec![3, 4]);
    }
}
