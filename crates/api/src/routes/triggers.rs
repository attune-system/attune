//! Trigger and Sensor management API routes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value as JsonValue};
use std::sync::Arc;
use validator::Validate;

use attune_common::{
    action_visibility::trigger_reference_allowed,
    models::{enums::ActionReferenceVisibility, trigger::Trigger as TriggerModel},
    mq::{
        MessageEnvelope, MessageType, RuleDisabledPayload, RuleEnabledPayload,
        TriggerChangedPayload,
    },
    rbac::{Action, AuthorizationContext, Resource},
    repositories::{
        pack::PackRepository,
        rule::{RuleRepository, RuleSearchFilters},
        runtime::RuntimeRepository,
        trigger::{
            validate_trigger_reference_visibility_config, CreateSensorInput, CreateTriggerInput,
            SensorRepository, SensorSearchFilters, TriggerRepository, TriggerSearchFilters,
            UpdateSensorInput, UpdateTriggerInput,
        },
        Create, Delete, FindByRef, Patch, Update,
    },
};

use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    authz::AuthorizationService,
    dto::{
        common::{PaginatedResponse, PaginationParams},
        trigger::{
            CreateSensorRequest, CreateTriggerRequest, LogRetentionLimitPatch,
            LogRetentionPolicyPatch, SensorJsonPatch, SensorResponse, SensorSummary,
            TriggerJsonPatch, TriggerListParams, TriggerReferenceParams, TriggerResponse,
            TriggerStringPatch, TriggerSummary, UpdateSensorRequest, UpdateTriggerRequest,
        },
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    routes::rule_lifecycle_notifier::notify_rule_lifecycle_changed,
    state::AppState,
};

// ============================================================================
// TRIGGER ENDPOINTS
// ============================================================================

#[derive(Debug, sqlx::FromRow)]
struct RuleLifecycleRow {
    id: i64,
    r#ref: String,
    trigger_ref: String,
    trigger_params: JsonValue,
}

async fn publish_rule_lifecycle_messages(
    state: &Arc<AppState>,
    rows: Vec<RuleLifecycleRow>,
    enabled: bool,
) -> ApiResult<()> {
    let publisher = state.get_publisher().await;

    for row in rows {
        if let Some(publisher) = publisher.as_ref() {
            let publish_result = if enabled {
                let payload = RuleEnabledPayload {
                    rule_id: row.id,
                    rule_ref: row.r#ref.clone(),
                    trigger_ref: row.trigger_ref.clone(),
                    trigger_params: Some(row.trigger_params.clone()),
                };
                let envelope = MessageEnvelope::new(MessageType::RuleEnabled, payload)
                    .with_source("api-service");
                publisher.publish_envelope(&envelope).await
            } else {
                let payload = RuleDisabledPayload {
                    rule_id: row.id,
                    rule_ref: row.r#ref.clone(),
                    trigger_ref: row.trigger_ref.clone(),
                };
                let envelope = MessageEnvelope::new(MessageType::RuleDisabled, payload)
                    .with_source("api-service");
                publisher.publish_envelope(&envelope).await
            };

            if let Err(error) = publish_result {
                tracing::warn!(
                    "Failed to publish rule lifecycle message for rule {}: {}",
                    row.r#ref,
                    error
                );
            }
        }

        if let Err(error) = notify_rule_lifecycle_changed(
            &state.db,
            if enabled {
                "rule.enabled"
            } else {
                "rule.disabled"
            },
            row.id,
            &row.r#ref,
            &row.trigger_ref,
            if enabled {
                Some(&row.trigger_params)
            } else {
                None
            },
            enabled,
        )
        .await
        {
            tracing::warn!(
                "Failed to emit notifier rule lifecycle update for rule {}: {}",
                row.r#ref,
                error
            );
        }
    }

    Ok(())
}

async fn filter_api_visible_triggers(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    triggers: Vec<TriggerModel>,
    referencing_pack_ref: Option<&str>,
) -> ApiResult<Vec<TriggerModel>> {
    let authz = AuthorizationService::new(state.db.clone());
    let grants = authz.effective_grants(user).await?;
    let identity_id = user.identity_id().ok();

    Ok(triggers
        .into_iter()
        .filter(|trigger| {
            trigger.reference_visibility == ActionReferenceVisibility::Public
                || referencing_pack_ref
                    .is_some_and(|pack_ref| trigger_reference_allowed(trigger, Some(pack_ref)))
                || identity_id.is_some_and(|id| {
                    let mut ctx = AuthorizationContext::new(id);
                    ctx.target_id = Some(trigger.id);
                    ctx.target_ref = Some(trigger.r#ref.clone());
                    ctx.pack_ref = trigger.pack_ref.clone();
                    AuthorizationService::is_allowed(
                        &grants,
                        Resource::Triggers,
                        Action::Update,
                        &ctx,
                    )
                })
        })
        .collect())
}

async fn can_access_trigger_api(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    trigger: &TriggerModel,
    referencing_pack_ref: Option<&str>,
) -> ApiResult<bool> {
    Ok(
        filter_api_visible_triggers(state, user, vec![trigger.clone()], referencing_pack_ref)
            .await?
            .into_iter()
            .next()
            .is_some(),
    )
}

async fn visible_trigger_page(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    mut filters: TriggerSearchFilters,
    pagination: &PaginationParams,
    referencing_pack_ref: Option<&str>,
) -> ApiResult<PaginatedResponse<TriggerSummary>> {
    filters.limit = 1;
    filters.offset = 0;
    let initial = TriggerRepository::list_search(&state.db, &filters).await?;

    let all_rows = if initial.total == 0 {
        Vec::new()
    } else {
        filters.limit = u32::try_from(initial.total).unwrap_or(u32::MAX);
        filters.offset = 0;
        TriggerRepository::list_search(&state.db, &filters)
            .await?
            .rows
    };

    let visible = filter_api_visible_triggers(state, user, all_rows, referencing_pack_ref).await?;
    let total = visible.len() as u64;
    let rows = visible
        .into_iter()
        .skip(pagination.offset() as usize)
        .take(pagination.limit() as usize)
        .map(TriggerSummary::from)
        .collect();

    Ok(PaginatedResponse::new(rows, pagination, total))
}

async fn ensure_trigger_visibility_update_preserves_existing_references(
    state: &Arc<AppState>,
    existing_trigger: &TriggerModel,
    new_visibility: ActionReferenceVisibility,
    new_allowed_pack_refs: &[String],
) -> ApiResult<()> {
    let mut candidate = existing_trigger.clone();
    candidate.reference_visibility = new_visibility;
    candidate.reference_allowed_pack_refs = new_allowed_pack_refs.to_vec();

    let rules = RuleRepository::list_search(
        &state.db,
        &RuleSearchFilters {
            trigger_ref: Some(existing_trigger.r#ref.clone()),
            limit: 10_000,
            offset: 0,
            ..Default::default()
        },
    )
    .await?;

    for rule in rules.rows {
        if !trigger_reference_allowed(&candidate, Some(&rule.pack_ref)) {
            return Err(ApiError::BadRequest(format!(
                "Cannot change trigger '{}' visibility to {:?}: rule '{}' in pack '{}' currently subscribes to it",
                existing_trigger.r#ref, new_visibility, rule.r#ref, rule.pack_ref
            )));
        }
    }

    Ok(())
}

async fn publish_trigger_lifecycle_change(
    state: &Arc<AppState>,
    trigger_id: i64,
    enabled: bool,
) -> ApiResult<()> {
    let rows = sqlx::query_as::<_, RuleLifecycleRow>(
        r#"
        SELECT id, ref, trigger_ref, trigger_params
        FROM rule
        WHERE trigger = $1
          AND enabled = TRUE
        "#,
    )
    .bind(trigger_id)
    .fetch_all(&state.db)
    .await?;

    publish_rule_lifecycle_messages(state, rows, enabled).await
}

async fn publish_sensor_lifecycle_change(
    state: &Arc<AppState>,
    sensor_id: i64,
    enabled: bool,
) -> ApiResult<()> {
    let rows = sqlx::query_as::<_, RuleLifecycleRow>(
        r#"
        SELECT r.id, r.ref, r.trigger_ref, r.trigger_params
        FROM rule r
        JOIN trigger t ON t.id = r.trigger
        WHERE t.sensor = $1
          AND r.enabled = TRUE
        "#,
    )
    .bind(sensor_id)
    .fetch_all(&state.db)
    .await?;

    publish_rule_lifecycle_messages(state, rows, enabled).await
}

async fn publish_trigger_metadata_change(
    state: &Arc<AppState>,
    trigger: &TriggerModel,
    operation: &str,
    updated_at: chrono::DateTime<chrono::Utc>,
) {
    let Some(publisher) = state.get_publisher().await else {
        return;
    };

    let payload = TriggerChangedPayload {
        trigger_id: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        pack_ref: trigger.pack_ref.clone(),
        operation: operation.to_string(),
        updated_at,
    };
    let envelope =
        MessageEnvelope::new(MessageType::TriggerChanged, payload).with_source("api-service");
    if let Err(error) = publisher.publish_envelope(&envelope).await {
        tracing::warn!(
            "Failed to publish TriggerChanged metadata invalidation for trigger {}: {}",
            trigger.r#ref,
            error
        );
    }
}

/// List all triggers with pagination
#[utoipa::path(
    get,
    path = "/api/v1/triggers",
    tag = "triggers",
    params(TriggerListParams),
    responses(
        (status = 200, description = "List of triggers", body = PaginatedResponse<TriggerSummary>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_triggers(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<TriggerListParams>,
) -> ApiResult<impl IntoResponse> {
    let pagination = PaginationParams {
        page: query.page,
        page_size: query.page_size,
    };
    let filters = TriggerSearchFilters {
        pack: None,
        sensor: None,
        enabled: None,
        limit: 0,
        offset: 0,
    };

    let response = visible_trigger_page(
        &state,
        &user,
        filters,
        &pagination,
        query.referencing_pack_ref.as_deref(),
    )
    .await?;

    Ok((StatusCode::OK, Json(response)))
}

/// List enabled triggers
#[utoipa::path(
    get,
    path = "/api/v1/triggers/enabled",
    tag = "triggers",
    params(TriggerListParams),
    responses(
        (status = 200, description = "List of enabled triggers", body = PaginatedResponse<TriggerSummary>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_enabled_triggers(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<TriggerListParams>,
) -> ApiResult<impl IntoResponse> {
    let pagination = PaginationParams {
        page: query.page,
        page_size: query.page_size,
    };
    let filters = TriggerSearchFilters {
        pack: None,
        sensor: None,
        enabled: Some(true),
        limit: 0,
        offset: 0,
    };

    let response = visible_trigger_page(
        &state,
        &user,
        filters,
        &pagination,
        query.referencing_pack_ref.as_deref(),
    )
    .await?;

    Ok((StatusCode::OK, Json(response)))
}

/// List triggers by pack reference
#[utoipa::path(
    get,
    path = "/api/v1/packs/{pack_ref}/triggers",
    tag = "triggers",
    params(
        ("pack_ref" = String, Path, description = "Pack reference"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of triggers in pack", body = PaginatedResponse<TriggerSummary>),
        (status = 404, description = "Pack not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_triggers_by_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    // Verify pack exists
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    let filters = TriggerSearchFilters {
        pack: Some(pack.id),
        sensor: None,
        enabled: None,
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = TriggerRepository::list_search(&state.db, &filters).await?;

    let paginated_triggers: Vec<TriggerSummary> =
        result.rows.into_iter().map(TriggerSummary::from).collect();

    let response = PaginatedResponse::new(paginated_triggers, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// Get a single trigger by reference
#[utoipa::path(
    get,
    path = "/api/v1/triggers/{ref}",
    tag = "triggers",
    params(
        ("ref" = String, Path, description = "Trigger reference"),
        TriggerReferenceParams
    ),
    responses(
        (status = 200, description = "Trigger details", body = ApiResponse<TriggerResponse>),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_trigger(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(trigger_ref): Path<String>,
    Query(query): Query<TriggerReferenceParams>,
) -> ApiResult<impl IntoResponse> {
    let trigger = TriggerRepository::find_by_ref(&state.db, &trigger_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;
    if !can_access_trigger_api(
        &state,
        &user,
        &trigger,
        query.referencing_pack_ref.as_deref(),
    )
    .await?
    {
        return Err(ApiError::NotFound(format!(
            "Trigger '{}' not found",
            trigger_ref
        )));
    }

    let response = ApiResponse::new(TriggerResponse::from(trigger));

    Ok((StatusCode::OK, Json(response)))
}

/// Create a new trigger
#[utoipa::path(
    post,
    path = "/api/v1/triggers",
    tag = "triggers",
    request_body = CreateTriggerRequest,
    responses(
        (status = 201, description = "Trigger created successfully", body = ApiResponse<TriggerResponse>),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Pack not found"),
        (status = 409, description = "Trigger with same ref already exists"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn create_trigger(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Json(request): Json<CreateTriggerRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Check if trigger with same ref already exists
    if TriggerRepository::find_by_ref(&state.db, &request.r#ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Trigger with ref '{}' already exists",
            request.r#ref
        )));
    }

    // If pack_ref is provided, verify pack exists and get its ID
    let (pack_id, pack_ref) = if let Some(ref pack_ref_str) = request.pack_ref {
        let pack = PackRepository::find_by_ref(&state.db, pack_ref_str)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref_str)))?;
        (Some(pack.id), Some(pack.r#ref.clone()))
    } else {
        (None, None)
    };

    let reference_visibility = request.reference_visibility.unwrap_or_default();
    validate_trigger_reference_visibility_config(
        reference_visibility,
        &request.reference_allowed_pack_refs,
    )?;

    // Create trigger input
    let trigger_input = CreateTriggerInput {
        r#ref: request.r#ref,
        pack: pack_id,
        pack_ref,
        label: request.label,
        description: request.description,
        enabled: request.enabled,
        param_schema: request.param_schema,
        out_schema: request.out_schema,
        sensor: None,
        sensor_ref: None,
        is_adhoc: true, // Triggers created via API are ad-hoc (not from pack installation)
        reference_visibility,
        reference_allowed_pack_refs: request.reference_allowed_pack_refs,
    };

    let trigger = TriggerRepository::create(&state.db, trigger_input).await?;
    publish_trigger_metadata_change(&state, &trigger, "created", trigger.updated).await;

    let response = ApiResponse::with_message(
        TriggerResponse::from(trigger),
        "Trigger created successfully",
    );

    Ok((StatusCode::CREATED, Json(response)))
}

/// Update an existing trigger
#[utoipa::path(
    put,
    path = "/api/v1/triggers/{ref}",
    tag = "triggers",
    params(
        ("ref" = String, Path, description = "Trigger reference")
    ),
    request_body = UpdateTriggerRequest,
    responses(
        (status = 200, description = "Trigger updated successfully", body = ApiResponse<TriggerResponse>),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn update_trigger(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(trigger_ref): Path<String>,
    Json(request): Json<UpdateTriggerRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Check if trigger exists
    let existing_trigger = TriggerRepository::find_by_ref(&state.db, &trigger_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;

    let effective_reference_visibility = request
        .reference_visibility
        .unwrap_or(existing_trigger.reference_visibility);
    let effective_reference_allowed_pack_refs = request
        .reference_allowed_pack_refs
        .clone()
        .unwrap_or_else(|| existing_trigger.reference_allowed_pack_refs.clone());
    validate_trigger_reference_visibility_config(
        effective_reference_visibility,
        &effective_reference_allowed_pack_refs,
    )?;
    if request.reference_visibility.is_some() || request.reference_allowed_pack_refs.is_some() {
        ensure_trigger_visibility_update_preserves_existing_references(
            &state,
            &existing_trigger,
            effective_reference_visibility,
            &effective_reference_allowed_pack_refs,
        )
        .await?;
    }

    // Create update input
    let update_input = UpdateTriggerInput {
        label: request.label,
        description: request.description.map(|patch| match patch {
            TriggerStringPatch::Set(value) => Patch::Set(value),
            TriggerStringPatch::Clear => Patch::Clear,
        }),
        enabled: request.enabled,
        param_schema: request.param_schema.map(|patch| match patch {
            TriggerJsonPatch::Set(value) => Patch::Set(value),
            TriggerJsonPatch::Clear => Patch::Clear,
        }),
        out_schema: request.out_schema.map(|patch| match patch {
            TriggerJsonPatch::Set(value) => Patch::Set(value),
            TriggerJsonPatch::Clear => Patch::Clear,
        }),
        sensor: None,
        sensor_ref: None,
        reference_visibility: request.reference_visibility,
        reference_allowed_pack_refs: request.reference_allowed_pack_refs,
    };

    let trigger = TriggerRepository::update(&state.db, existing_trigger.id, update_input).await?;
    if let Some(enabled) = request.enabled {
        if enabled != existing_trigger.enabled {
            publish_trigger_lifecycle_change(&state, trigger.id, enabled).await?;
        }
    }
    let operation = if existing_trigger.enabled != trigger.enabled {
        if trigger.enabled {
            "enabled"
        } else {
            "disabled"
        }
    } else {
        "updated"
    };
    publish_trigger_metadata_change(&state, &trigger, operation, trigger.updated).await;

    let response = ApiResponse::with_message(
        TriggerResponse::from(trigger),
        "Trigger updated successfully",
    );

    Ok((StatusCode::OK, Json(response)))
}

/// Delete a trigger
#[utoipa::path(
    delete,
    path = "/api/v1/triggers/{ref}",
    tag = "triggers",
    params(
        ("ref" = String, Path, description = "Trigger reference")
    ),
    responses(
        (status = 200, description = "Trigger deleted successfully", body = SuccessResponse),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn delete_trigger(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(trigger_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Check if trigger exists
    let trigger = TriggerRepository::find_by_ref(&state.db, &trigger_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;

    // Delete the trigger
    let deleted = TriggerRepository::delete(&state.db, trigger.id).await?;

    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Trigger '{}' not found",
            trigger_ref
        )));
    }

    publish_trigger_metadata_change(&state, &trigger, "deleted", trigger.updated).await;

    let response = SuccessResponse::new(format!("Trigger '{}' deleted successfully", trigger_ref));

    Ok((StatusCode::OK, Json(response)))
}

/// Enable a trigger
#[utoipa::path(
    post,
    path = "/api/v1/triggers/{ref}/enable",
    tag = "triggers",
    params(
        ("ref" = String, Path, description = "Trigger reference")
    ),
    responses(
        (status = 200, description = "Trigger enabled successfully", body = ApiResponse<TriggerResponse>),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn enable_trigger(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(trigger_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Check if trigger exists
    let existing_trigger = TriggerRepository::find_by_ref(&state.db, &trigger_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;

    // Update trigger to enabled
    let update_input = UpdateTriggerInput {
        enabled: Some(true),
        ..Default::default()
    };

    let trigger = TriggerRepository::update(&state.db, existing_trigger.id, update_input).await?;
    if !existing_trigger.enabled {
        publish_trigger_lifecycle_change(&state, trigger.id, true).await?;
    }
    publish_trigger_metadata_change(&state, &trigger, "enabled", trigger.updated).await;

    let response = ApiResponse::with_message(
        TriggerResponse::from(trigger),
        "Trigger enabled successfully",
    );

    Ok((StatusCode::OK, Json(response)))
}

/// Disable a trigger
#[utoipa::path(
    post,
    path = "/api/v1/triggers/{ref}/disable",
    tag = "triggers",
    params(
        ("ref" = String, Path, description = "Trigger reference")
    ),
    responses(
        (status = 200, description = "Trigger disabled successfully", body = ApiResponse<TriggerResponse>),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn disable_trigger(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(trigger_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Check if trigger exists
    let existing_trigger = TriggerRepository::find_by_ref(&state.db, &trigger_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;

    // Update trigger to disabled
    let update_input = UpdateTriggerInput {
        enabled: Some(false),
        ..Default::default()
    };

    let trigger = TriggerRepository::update(&state.db, existing_trigger.id, update_input).await?;
    if existing_trigger.enabled {
        publish_trigger_lifecycle_change(&state, trigger.id, false).await?;
    }
    publish_trigger_metadata_change(&state, &trigger, "disabled", trigger.updated).await;

    let response = ApiResponse::with_message(
        TriggerResponse::from(trigger),
        "Trigger disabled successfully",
    );

    Ok((StatusCode::OK, Json(response)))
}

// ============================================================================
// SENSOR ENDPOINTS
// ============================================================================

/// List all sensors with pagination
#[utoipa::path(
    get,
    path = "/api/v1/sensors",
    tag = "sensors",
    params(PaginationParams),
    responses(
        (status = 200, description = "List of sensors", body = PaginatedResponse<SensorSummary>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_sensors(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    let filters = SensorSearchFilters {
        pack: None,
        enabled: None,
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = SensorRepository::list_search(&state.db, &filters).await?;

    let paginated_sensors: Vec<SensorSummary> =
        result.rows.into_iter().map(SensorSummary::from).collect();

    let response = PaginatedResponse::new(paginated_sensors, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// List enabled sensors
#[utoipa::path(
    get,
    path = "/api/v1/sensors/enabled",
    tag = "sensors",
    params(PaginationParams),
    responses(
        (status = 200, description = "List of enabled sensors", body = PaginatedResponse<SensorSummary>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_enabled_sensors(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    let filters = SensorSearchFilters {
        pack: None,
        enabled: Some(true),
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = SensorRepository::list_search(&state.db, &filters).await?;

    let paginated_sensors: Vec<SensorSummary> =
        result.rows.into_iter().map(SensorSummary::from).collect();

    let response = PaginatedResponse::new(paginated_sensors, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// List sensors by pack reference
#[utoipa::path(
    get,
    path = "/api/v1/packs/{pack_ref}/sensors",
    tag = "sensors",
    params(
        ("pack_ref" = String, Path, description = "Pack reference"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of sensors in pack", body = PaginatedResponse<SensorSummary>),
        (status = 404, description = "Pack not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_sensors_by_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    // Verify pack exists
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    let filters = SensorSearchFilters {
        pack: Some(pack.id),
        enabled: None,
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = SensorRepository::list_search(&state.db, &filters).await?;

    let paginated_sensors: Vec<SensorSummary> =
        result.rows.into_iter().map(SensorSummary::from).collect();

    let response = PaginatedResponse::new(paginated_sensors, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// List sensors by trigger reference
#[utoipa::path(
    get,
    path = "/api/v1/triggers/{trigger_ref}/sensors",
    tag = "sensors",
    params(
        ("trigger_ref" = String, Path, description = "Trigger reference"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of sensors for trigger", body = PaginatedResponse<SensorSummary>),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_sensors_by_trigger(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(trigger_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    // Verify trigger exists
    let _trigger = TriggerRepository::find_by_ref(&state.db, &trigger_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;

    let filters = SensorSearchFilters {
        pack: None,
        enabled: None,
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = SensorRepository::list_search(&state.db, &filters).await?;

    let paginated_sensors: Vec<SensorSummary> =
        result.rows.into_iter().map(SensorSummary::from).collect();

    let response = PaginatedResponse::new(paginated_sensors, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// Get a single sensor by reference
#[utoipa::path(
    get,
    path = "/api/v1/sensors/{ref}",
    tag = "sensors",
    params(
        ("ref" = String, Path, description = "Sensor reference")
    ),
    responses(
        (status = 200, description = "Sensor details", body = ApiResponse<SensorResponse>),
        (status = 404, description = "Sensor not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_sensor(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(sensor_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let sensor = SensorRepository::find_by_ref(&state.db, &sensor_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Sensor '{}' not found", sensor_ref)))?;

    let response = ApiResponse::new(SensorResponse::from(sensor));

    Ok((StatusCode::OK, Json(response)))
}

/// Create a new sensor
#[utoipa::path(
    post,
    path = "/api/v1/sensors",
    tag = "sensors",
    request_body = CreateSensorRequest,
    responses(
        (status = 201, description = "Sensor created successfully", body = ApiResponse<SensorResponse>),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Pack, runtime, or trigger not found"),
        (status = 409, description = "Sensor with same ref already exists"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn create_sensor(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Json(request): Json<CreateSensorRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Check if sensor with same ref already exists
    if SensorRepository::find_by_ref(&state.db, &request.r#ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Sensor with ref '{}' already exists",
            request.r#ref
        )));
    }

    // Verify pack exists and get its ID
    let pack = PackRepository::find_by_ref(&state.db, &request.pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", request.pack_ref)))?;

    // Verify runtime exists and get its ID
    let runtime = RuntimeRepository::find_by_ref(&state.db, &request.runtime_ref)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Runtime '{}' not found", request.runtime_ref))
        })?;

    // Verify trigger exists and get its ID
    let _trigger = TriggerRepository::find_by_ref(&state.db, &request.trigger_ref)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Trigger '{}' not found", request.trigger_ref))
        })?;

    // Create sensor input
    let sensor_input = CreateSensorInput {
        r#ref: request.r#ref,
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: request.label,
        description: request.description,
        entrypoint: request.entrypoint,
        runtime: runtime.id,
        runtime_ref: runtime.r#ref.clone(),
        runtime_version_constraint: None,
        enabled: request.enabled,
        param_schema: request.param_schema,
        config: request.config,
        worker_selector: serde_json::to_value(request.worker_selector)
            .unwrap_or_else(|_| json!({})),
        worker_tolerations: serde_json::to_value(request.worker_tolerations)
            .unwrap_or_else(|_| json!([])),
        worker_affinity: serde_json::to_value(request.worker_affinity)
            .unwrap_or_else(|_| json!({})),
        artifact_retention_policy: request.artifact_retention_policy,
        artifact_retention_limit: request.artifact_retention_limit,
        log_retention_policy: request.log_retention_policy,
        log_retention_limit: request.log_retention_limit,
    };

    let sensor = SensorRepository::create(&state.db, sensor_input).await?;

    let response =
        ApiResponse::with_message(SensorResponse::from(sensor), "Sensor created successfully");

    Ok((StatusCode::CREATED, Json(response)))
}

/// Update an existing sensor
#[utoipa::path(
    put,
    path = "/api/v1/sensors/{ref}",
    tag = "sensors",
    params(
        ("ref" = String, Path, description = "Sensor reference")
    ),
    request_body = UpdateSensorRequest,
    responses(
        (status = 200, description = "Sensor updated successfully", body = ApiResponse<SensorResponse>),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Sensor not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn update_sensor(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(sensor_ref): Path<String>,
    Json(request): Json<UpdateSensorRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Check if sensor exists
    let existing_sensor = SensorRepository::find_by_ref(&state.db, &sensor_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Sensor '{}' not found", sensor_ref)))?;

    // Create update input
    let update_input = UpdateSensorInput {
        label: request.label,
        description: request.description.map(Patch::Set),
        entrypoint: request.entrypoint,
        runtime: None,
        runtime_ref: None,
        runtime_version_constraint: None,
        enabled: request.enabled,
        param_schema: request.param_schema.map(|patch| match patch {
            SensorJsonPatch::Set(value) => Patch::Set(value),
            SensorJsonPatch::Clear => Patch::Clear,
        }),
        config: None,
        worker_selector: request
            .worker_selector
            .map(|value| serde_json::to_value(value).unwrap_or_else(|_| json!({}))),
        worker_tolerations: request
            .worker_tolerations
            .map(|value| serde_json::to_value(value).unwrap_or_else(|_| json!([]))),
        worker_affinity: request
            .worker_affinity
            .map(|value| serde_json::to_value(value).unwrap_or_else(|_| json!({}))),
        artifact_retention_policy: request.artifact_retention_policy.map(|patch| match patch {
            LogRetentionPolicyPatch::Set(value) => Patch::Set(value),
            LogRetentionPolicyPatch::Clear => Patch::Clear,
        }),
        artifact_retention_limit: request.artifact_retention_limit.map(|patch| match patch {
            LogRetentionLimitPatch::Set(value) => Patch::Set(value),
            LogRetentionLimitPatch::Clear => Patch::Clear,
        }),
        log_retention_policy: request.log_retention_policy.map(|patch| match patch {
            LogRetentionPolicyPatch::Set(value) => Patch::Set(value),
            LogRetentionPolicyPatch::Clear => Patch::Clear,
        }),
        log_retention_limit: request.log_retention_limit.map(|patch| match patch {
            LogRetentionLimitPatch::Set(value) => Patch::Set(value),
            LogRetentionLimitPatch::Clear => Patch::Clear,
        }),
    };

    let sensor = SensorRepository::update(&state.db, existing_sensor.id, update_input).await?;
    if let Some(enabled) = request.enabled {
        if enabled != existing_sensor.enabled {
            publish_sensor_lifecycle_change(&state, sensor.id, enabled).await?;
        }
    }

    let response =
        ApiResponse::with_message(SensorResponse::from(sensor), "Sensor updated successfully");

    Ok((StatusCode::OK, Json(response)))
}

/// Delete a sensor
#[utoipa::path(
    delete,
    path = "/api/v1/sensors/{ref}",
    tag = "sensors",
    params(
        ("ref" = String, Path, description = "Sensor reference")
    ),
    responses(
        (status = 200, description = "Sensor deleted successfully", body = SuccessResponse),
        (status = 404, description = "Sensor not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn delete_sensor(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(sensor_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Check if sensor exists
    let sensor = SensorRepository::find_by_ref(&state.db, &sensor_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Sensor '{}' not found", sensor_ref)))?;

    // Delete the sensor
    let deleted = SensorRepository::delete(&state.db, sensor.id).await?;

    if !deleted {
        return Err(ApiError::NotFound(format!(
            "Sensor '{}' not found",
            sensor_ref
        )));
    }

    let response = SuccessResponse::new(format!("Sensor '{}' deleted successfully", sensor_ref));

    Ok((StatusCode::OK, Json(response)))
}

/// Enable a sensor
#[utoipa::path(
    post,
    path = "/api/v1/sensors/{ref}/enable",
    tag = "sensors",
    params(
        ("ref" = String, Path, description = "Sensor reference")
    ),
    responses(
        (status = 200, description = "Sensor enabled successfully", body = ApiResponse<SensorResponse>),
        (status = 404, description = "Sensor not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn enable_sensor(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(sensor_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Check if sensor exists
    let existing_sensor = SensorRepository::find_by_ref(&state.db, &sensor_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Sensor '{}' not found", sensor_ref)))?;

    // Update sensor to enabled
    let update_input = UpdateSensorInput {
        label: None,
        description: None,
        entrypoint: None,
        runtime: None,
        runtime_ref: None,
        runtime_version_constraint: None,
        enabled: Some(true),
        param_schema: None,
        config: None,
        worker_selector: None,
        worker_tolerations: None,
        worker_affinity: None,
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        log_retention_policy: None,
        log_retention_limit: None,
    };

    let sensor = SensorRepository::update(&state.db, existing_sensor.id, update_input).await?;
    if !existing_sensor.enabled {
        publish_sensor_lifecycle_change(&state, sensor.id, true).await?;
    }

    let response =
        ApiResponse::with_message(SensorResponse::from(sensor), "Sensor enabled successfully");

    Ok((StatusCode::OK, Json(response)))
}

/// Disable a sensor
#[utoipa::path(
    post,
    path = "/api/v1/sensors/{ref}/disable",
    tag = "sensors",
    params(
        ("ref" = String, Path, description = "Sensor reference")
    ),
    responses(
        (status = 200, description = "Sensor disabled successfully", body = ApiResponse<SensorResponse>),
        (status = 404, description = "Sensor not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn disable_sensor(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(sensor_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Check if sensor exists
    let existing_sensor = SensorRepository::find_by_ref(&state.db, &sensor_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Sensor '{}' not found", sensor_ref)))?;

    // Update sensor to disabled
    let update_input = UpdateSensorInput {
        label: None,
        description: None,
        entrypoint: None,
        runtime: None,
        runtime_ref: None,
        runtime_version_constraint: None,
        enabled: Some(false),
        param_schema: None,
        config: None,
        worker_selector: None,
        worker_tolerations: None,
        worker_affinity: None,
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        log_retention_policy: None,
        log_retention_limit: None,
    };

    let sensor = SensorRepository::update(&state.db, existing_sensor.id, update_input).await?;
    if existing_sensor.enabled {
        publish_sensor_lifecycle_change(&state, sensor.id, false).await?;
    }

    let response =
        ApiResponse::with_message(SensorResponse::from(sensor), "Sensor disabled successfully");

    Ok((StatusCode::OK, Json(response)))
}

/// Create trigger and sensor routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Trigger routes
        .route("/triggers", get(list_triggers).post(create_trigger))
        .route("/triggers/enabled", get(list_enabled_triggers))
        .route(
            "/triggers/{ref}",
            get(get_trigger).put(update_trigger).delete(delete_trigger),
        )
        .route("/triggers/{ref}/enable", post(enable_trigger))
        .route("/triggers/{ref}/disable", post(disable_trigger))
        .route("/packs/{pack_ref}/triggers", get(list_triggers_by_pack))
        // Sensor routes
        .route("/sensors", get(list_sensors).post(create_sensor))
        .route("/sensors/enabled", get(list_enabled_sensors))
        .route(
            "/sensors/{ref}",
            get(get_sensor).put(update_sensor).delete(delete_sensor),
        )
        .route("/sensors/{ref}/enable", post(enable_sensor))
        .route("/sensors/{ref}/disable", post(disable_sensor))
        .route("/packs/{pack_ref}/sensors", get(list_sensors_by_pack))
        .route(
            "/triggers/{trigger_ref}/sensors",
            get(list_sensors_by_trigger),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigger_sensor_routes_structure() {
        // Just verify the router can be constructed
        let _router = routes();
    }
}
