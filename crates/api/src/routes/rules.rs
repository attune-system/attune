//! Rule management API routes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;
use tracing::{info, warn};
use validator::Validate;

use attune_common::action_visibility::{
    ensure_action_reference_allowed, ensure_trigger_reference_allowed,
};
use attune_common::mq::{
    MessageEnvelope, MessageType, RuleCreatedPayload, RuleDeletedPayload, RuleDisabledPayload,
    RuleEnabledPayload,
};
use attune_common::rbac::{Action, AuthorizationContext, Resource};
use attune_common::repositories::{
    action::ActionRepository,
    pack::PackRepository,
    rule::{
        CreateRuleInput, RuleRepository, RuleSearchFilters, RuleSensorPlacementInput,
        UpdateRuleInput,
    },
    sensor_admission::{
        SensorAdmissionFailure, SensorAdmissionRepository, SensorAdmissionRequirement,
    },
    trigger::TriggerRepository,
    Delete, FindByRef, Patch, Update,
};
use attune_common::scheduling::parse_rule_sensor_placement;

use crate::{
    auth::{jwt::TokenType, middleware::RequireAuth},
    authz::AuthorizationCheck,
    dto::{
        common::{PaginatedResponse, PaginationParams, PaginationSearchParams},
        rule::{CreateRuleRequest, RuleListParams, RuleResponse, RuleSummary, UpdateRuleRequest},
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    routes::rule_lifecycle_notifier::notify_rule_lifecycle_changed,
    routes::visibility::{
        build_visibility_read_scope, is_scoped_identity_token, scope_allows_resource_ref,
    },
    state::AppState,
    validation::{validate_action_params, validate_trigger_params},
};
use attune_common::repositories::event::VisibilityReadScope;

fn format_sensor_trigger_scope(allowed_trigger_refs: &[String]) -> String {
    if allowed_trigger_refs.is_empty() {
        return "none".to_string();
    }

    allowed_trigger_refs.join(", ")
}

fn reject_sensor_admission(failures: Vec<SensorAdmissionFailure>) -> Result<(), ApiError> {
    if failures.is_empty() {
        return Ok(());
    }
    Err(ApiError::UnprocessableEntity(
        serde_json::to_string(&failures)
            .unwrap_or_else(|_| "Managed sensor placement is incompatible".to_string()),
    ))
}

fn ensure_sensor_trigger_scope_for_rule_listing(
    user: &crate::auth::middleware::AuthenticatedUser,
    requested_trigger_ref: Option<&str>,
) -> ApiResult<()> {
    if user.claims.token_type != TokenType::Sensor {
        return Ok(());
    }

    let trigger_ref = requested_trigger_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::Forbidden(
                "Sensor tokens can only list rules by explicit trigger_ref. Use /api/v1/rules?trigger_ref=<allowed_trigger_ref> or /api/v1/triggers/{trigger_ref}/rules.".to_string(),
            )
        })?;

    let allowed_trigger_refs = user.sensor_trigger_types();
    if allowed_trigger_refs
        .iter()
        .any(|allowed| allowed == trigger_ref)
    {
        return Ok(());
    }

    Err(ApiError::Forbidden(format!(
        "Sensor token is not allowed to list rules for trigger '{}'. Allowed trigger_refs: {}",
        trigger_ref,
        format_sensor_trigger_scope(&allowed_trigger_refs)
    )))
}

fn ensure_sensor_trigger_scoped_rule_endpoint(
    user: &crate::auth::middleware::AuthenticatedUser,
    endpoint: &str,
) -> ApiResult<()> {
    if user.claims.token_type != TokenType::Sensor {
        return Ok(());
    }

    Err(ApiError::Forbidden(format!(
        "Sensor tokens can only read rules through trigger-scoped endpoints. '{}' is not allowed. Use /api/v1/rules?trigger_ref=<allowed_trigger_ref> or /api/v1/triggers/{{trigger_ref}}/rules.",
        endpoint
    )))
}

fn ensure_sensor_trigger_scope_for_rule_read(
    user: &crate::auth::middleware::AuthenticatedUser,
    rule_ref: &str,
    rule_trigger_ref: &str,
) -> ApiResult<()> {
    if user.claims.token_type != TokenType::Sensor {
        return Ok(());
    }

    let allowed_trigger_refs = user.sensor_trigger_types();
    if allowed_trigger_refs
        .iter()
        .any(|allowed| allowed == rule_trigger_ref)
    {
        return Ok(());
    }

    Err(ApiError::Forbidden(format!(
        "Sensor token is not allowed to read rule '{}' because it is bound to trigger '{}'. Allowed trigger_refs: {}",
        rule_ref,
        rule_trigger_ref,
        format_sensor_trigger_scope(&allowed_trigger_refs)
    )))
}

/// Compute the row-level rule read visibility scope for a caller.
///
/// Rules are private-scoped metadata: scoped-identity tokens (access/execution)
/// only see rules their effective grants authorize. An unconstrained global
/// `rules:read` grant yields full access; scoped grants yield id/ref/pack_ref
/// allowlists; no grant yields an empty (deny) scope. Non-scoped tokens
/// (sensor/worker) return `None`, preserving their existing behavior (sensor
/// trigger-scoping is enforced separately).
async fn rule_read_visibility(
    state: &Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
) -> ApiResult<Option<VisibilityReadScope>> {
    if !is_scoped_identity_token(user) {
        return Ok(None);
    }

    let grants = state.authorization_service().effective_grants(user).await?;
    Ok(Some(build_visibility_read_scope(
        &grants,
        Resource::Rules,
        Action::Read,
        false,
    )))
}

/// List all rules with pagination
#[utoipa::path(
    get,
    path = "/api/v1/rules",
    tag = "rules",
    params(RuleListParams),
    responses(
        (status = 200, description = "List of rules", body = PaginatedResponse<RuleSummary>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_rules(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<RuleListParams>,
) -> ApiResult<impl IntoResponse> {
    ensure_sensor_trigger_scope_for_rule_listing(&user, query.trigger_ref.as_deref())?;
    let visibility = rule_read_visibility(&state, &user).await?;

    let pagination = PaginationParams {
        page: query.page,
        page_size: query.page_size,
    };
    let limit = query.limit();
    let offset = query.offset();
    let filters = RuleSearchFilters {
        pack: None,
        pack_ref: query.pack_ref,
        action: None,
        action_ref: query.action_ref,
        trigger: None,
        trigger_ref: query.trigger_ref,
        enabled: query.enabled,
        query: query.q,
        visibility,
        limit,
        offset,
    };

    let result = RuleRepository::list_search(&state.db, &filters).await?;

    let paginated_rules: Vec<RuleSummary> =
        result.rows.into_iter().map(RuleSummary::from).collect();

    let response = PaginatedResponse::new(paginated_rules, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// List enabled rules
#[utoipa::path(
    get,
    path = "/api/v1/rules/enabled",
    tag = "rules",
    params(PaginationParams),
    responses(
        (status = 200, description = "List of enabled rules", body = PaginatedResponse<RuleSummary>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_enabled_rules(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    ensure_sensor_trigger_scoped_rule_endpoint(&user, "/api/v1/rules/enabled")?;
    let visibility = rule_read_visibility(&state, &user).await?;

    let filters = RuleSearchFilters {
        pack: None,
        pack_ref: None,
        action: None,
        action_ref: None,
        trigger: None,
        trigger_ref: None,
        enabled: Some(true),
        query: None,
        visibility,
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = RuleRepository::list_search(&state.db, &filters).await?;

    let paginated_rules: Vec<RuleSummary> =
        result.rows.into_iter().map(RuleSummary::from).collect();

    let response = PaginatedResponse::new(paginated_rules, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// List rules by pack reference
#[utoipa::path(
    get,
    path = "/api/v1/packs/{pack_ref}/rules",
    tag = "rules",
    params(
        ("pack_ref" = String, Path, description = "Pack reference"),
        PaginationSearchParams
    ),
    responses(
        (status = 200, description = "List of rules in pack", body = PaginatedResponse<RuleSummary>),
        (status = 404, description = "Pack not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_rules_by_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(query): Query<PaginationSearchParams>,
) -> ApiResult<impl IntoResponse> {
    let pagination = query.pagination();
    ensure_sensor_trigger_scoped_rule_endpoint(&user, "/api/v1/packs/{pack_ref}/rules")?;
    let visibility = rule_read_visibility(&state, &user).await?;

    // Verify pack exists
    let pack = PackRepository::find_by_ref(&state.db, &pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", pack_ref)))?;

    let filters = RuleSearchFilters {
        pack: Some(pack.id),
        pack_ref: None,
        action: None,
        action_ref: None,
        trigger: None,
        trigger_ref: None,
        enabled: None,
        query: query.q,
        visibility,
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = RuleRepository::list_search(&state.db, &filters).await?;

    let paginated_rules: Vec<RuleSummary> =
        result.rows.into_iter().map(RuleSummary::from).collect();

    let response = PaginatedResponse::new(paginated_rules, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// List rules by action reference
#[utoipa::path(
    get,
    path = "/api/v1/actions/{action_ref}/rules",
    tag = "rules",
    params(
        ("action_ref" = String, Path, description = "Action reference"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of rules using this action", body = PaginatedResponse<RuleSummary>),
        (status = 404, description = "Action not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_rules_by_action(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(action_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    ensure_sensor_trigger_scoped_rule_endpoint(&user, "/api/v1/actions/{action_ref}/rules")?;
    let visibility = rule_read_visibility(&state, &user).await?;

    // Verify action exists
    let action = ActionRepository::find_by_ref(&state.db, &action_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", action_ref)))?;

    let filters = RuleSearchFilters {
        pack: None,
        pack_ref: None,
        action: Some(action.id),
        action_ref: None,
        trigger: None,
        trigger_ref: None,
        enabled: None,
        query: None,
        visibility,
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = RuleRepository::list_search(&state.db, &filters).await?;

    let paginated_rules: Vec<RuleSummary> =
        result.rows.into_iter().map(RuleSummary::from).collect();

    let response = PaginatedResponse::new(paginated_rules, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// List rules by trigger reference
#[utoipa::path(
    get,
    path = "/api/v1/triggers/{trigger_ref}/rules",
    tag = "rules",
    params(
        ("trigger_ref" = String, Path, description = "Trigger reference"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of rules using this trigger", body = PaginatedResponse<RuleSummary>),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_rules_by_trigger(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(trigger_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    ensure_sensor_trigger_scope_for_rule_listing(&user, Some(trigger_ref.as_str()))?;
    let visibility = rule_read_visibility(&state, &user).await?;

    // Verify trigger exists
    let trigger = TriggerRepository::find_by_ref(&state.db, &trigger_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;

    let filters = RuleSearchFilters {
        pack: None,
        pack_ref: None,
        action: None,
        action_ref: None,
        trigger: Some(trigger.id),
        trigger_ref: None,
        enabled: None,
        query: None,
        visibility,
        limit: pagination.limit(),
        offset: pagination.offset(),
    };

    let result = RuleRepository::list_search(&state.db, &filters).await?;

    let paginated_rules: Vec<RuleSummary> =
        result.rows.into_iter().map(RuleSummary::from).collect();

    let response = PaginatedResponse::new(paginated_rules, &pagination, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// Get a single rule by reference
#[utoipa::path(
    get,
    path = "/api/v1/rules/{ref}",
    tag = "rules",
    params(
        ("ref" = String, Path, description = "Rule reference")
    ),
    responses(
        (status = 200, description = "Rule details", body = ApiResponse<RuleResponse>),
        (status = 404, description = "Rule not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_rule(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(rule_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let rule = RuleRepository::find_by_ref(&state.db, &rule_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Rule '{}' not found", rule_ref)))?;
    ensure_sensor_trigger_scope_for_rule_read(&user, &rule.r#ref, &rule.trigger_ref)?;

    // Rules are private-scoped metadata: scoped-identity callers must hold a
    // rule read grant covering this rule. Deny as NotFound to avoid leaking
    // rule existence to unauthorized callers.
    if let Some(scope) = rule_read_visibility(&state, &user).await? {
        if !scope_allows_resource_ref(&scope, Some(rule.id), Some(rule.r#ref.as_str())) {
            return Err(ApiError::NotFound(format!("Rule '{}' not found", rule_ref)));
        }
    }

    let response = ApiResponse::new(RuleResponse::from(rule));

    Ok((StatusCode::OK, Json(response)))
}

/// Create a new rule
#[utoipa::path(
    post,
    path = "/api/v1/rules",
    tag = "rules",
    request_body = CreateRuleRequest,
    responses(
        (status = 201, description = "Rule created successfully", body = ApiResponse<RuleResponse>),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Pack, action, or trigger not found"),
        (status = 409, description = "Rule with same ref already exists"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn create_rule(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreateRuleRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Verify pack exists and get its ID
    let pack = PackRepository::find_by_ref(&state.db, &request.pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", request.pack_ref)))?;

    // Verify action exists and get its ID
    let action = ActionRepository::find_by_ref(&state.db, &request.action_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", request.action_ref)))?;
    ensure_action_reference_allowed(&action, Some(&pack.r#ref), "rule", &request.r#ref)?;

    // Verify trigger exists and get its ID
    let trigger = TriggerRepository::find_by_ref(&state.db, &request.trigger_ref)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Trigger '{}' not found", request.trigger_ref))
        })?;
    ensure_trigger_reference_allowed(&trigger, Some(&pack.r#ref), "rule", &request.r#ref)?;

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = state.authorization_service();
        let mut ctx = AuthorizationContext::new(identity_id);
        ctx.pack_ref = Some(pack.r#ref.clone());
        ctx.target_ref = Some(request.r#ref.clone());
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Rules,
                    action: Action::Create,
                    context: ctx,
                },
            )
            .await?;
    }

    // Validate trigger parameters against schema
    validate_trigger_params(&trigger, &request.trigger_params)?;

    // Validate action parameters against schema
    validate_action_params(&action, &request.action_params)?;

    let effective_permission_set_refs = request
        .permission_set_refs
        .clone()
        .unwrap_or_else(|| action.default_execution_permission_set_refs.clone());
    if !effective_permission_set_refs.is_empty()
        && !state
            .authorization_service()
            .can_delegate_permission_sets(&user, &effective_permission_set_refs)
            .await?
    {
        return Err(ApiError::Forbidden(
            "Cannot create rule with execution permission sets beyond current access".to_string(),
        ));
    }

    // Capture the authenticated identity to attribute rule-triggered
    // executions back to the user who registered the rule. For service-account
    // / token flows where `identity_id()` is not available we fall back to
    // None, which defers to the system identity at execution-creation time.
    let owner_identity = user.identity_id().ok();
    let trace_tag_template = request.trace_tag_template.clone();
    let authorized_component_ids = (pack.id, action.id, trigger.id);
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_mutations(&mut tx).await?;
    if RuleRepository::find_by_ref(&mut *tx, &request.r#ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Rule with ref '{}' already exists",
            request.r#ref
        )));
    }
    let pack = PackRepository::find_by_ref(&mut *tx, &request.pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", request.pack_ref)))?;
    let action = ActionRepository::find_by_ref(&mut *tx, &request.action_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", request.action_ref)))?;
    let trigger = TriggerRepository::find_by_ref(&mut *tx, &request.trigger_ref)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Trigger '{}' not found", request.trigger_ref))
        })?;
    if (pack.id, action.id, trigger.id) != authorized_component_ids {
        return Err(ApiError::Conflict(
            "A rule component changed while creation was being authorized".to_string(),
        ));
    }
    let current_permission_set_refs = request
        .permission_set_refs
        .clone()
        .unwrap_or_else(|| action.default_execution_permission_set_refs.clone());
    if current_permission_set_refs != effective_permission_set_refs {
        return Err(ApiError::Conflict(
            "Action permission defaults changed while rule creation was being authorized"
                .to_string(),
        ));
    }
    ensure_action_reference_allowed(&action, Some(&pack.r#ref), "rule", &request.r#ref)?;
    ensure_trigger_reference_allowed(&trigger, Some(&pack.r#ref), "rule", &request.r#ref)?;
    validate_trigger_params(&trigger, &request.trigger_params)?;
    validate_action_params(&action, &request.action_params)?;
    parse_rule_sensor_placement(
        &request.sensor_worker_selector,
        &request.sensor_worker_tolerations,
        &request.sensor_worker_affinity,
    )?;
    let sensor_placement = RuleSensorPlacementInput {
        selector: request.sensor_worker_selector,
        tolerations: request.sensor_worker_tolerations,
        affinity: request.sensor_worker_affinity,
    };

    // Create rule input
    let rule_input = CreateRuleInput {
        r#ref: request.r#ref,
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: request.label,
        description: request.description,
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: request.conditions,
        action_params: request.action_params,
        trigger_params: request.trigger_params,
        trace_tag_template,
        permission_set_refs: request.permission_set_refs,
        enabled: request.enabled,
        is_adhoc: true, // Rules created via API are ad-hoc (not from pack installation)
        owner_identity,
    };

    let rule = RuleRepository::create_with_sensor_placement(&mut *tx, rule_input, sensor_placement)
        .await?;
    let requirement = if rule.enabled {
        SensorAdmissionRequirement::Live
    } else {
        SensorAdmissionRequirement::Structural
    };
    reject_sensor_admission(
        SensorAdmissionRepository::assess_rule(&mut tx, rule.id, requirement).await?,
    )?;
    tx.commit().await?;

    // Publish RuleCreated message to notify sensor service
    if let Some(publisher) = state.get_publisher().await {
        let payload = RuleCreatedPayload {
            rule_id: rule.id,
            rule_ref: rule.r#ref.clone(),
            trigger_id: rule.trigger,
            trigger_ref: rule.trigger_ref.clone(),
            action_id: rule.action,
            action_ref: rule.action_ref.clone(),
            trigger_params: Some(rule.trigger_params.clone()),
            enabled: rule.enabled,
        };

        let envelope =
            MessageEnvelope::new(MessageType::RuleCreated, payload).with_source("api-service");

        if let Err(e) = publisher.publish_envelope(&envelope).await {
            warn!(
                "Failed to publish RuleCreated message for rule {}: {}",
                rule.r#ref, e
            );
        } else {
            info!("Published RuleCreated message for rule {}", rule.r#ref);
        }
    }
    if let Err(e) = notify_rule_lifecycle_changed(
        &state.db,
        "rule.created",
        rule.id,
        &rule.r#ref,
        &rule.trigger_ref,
        Some(&rule.trigger_params),
        rule.enabled,
    )
    .await
    {
        warn!(
            "Failed to emit notifier rule.created update for rule {}: {}",
            rule.r#ref, e
        );
    }

    let response = ApiResponse::with_message(RuleResponse::from(rule), "Rule created successfully");

    Ok((StatusCode::CREATED, Json(response)))
}

/// Update an existing rule
#[utoipa::path(
    put,
    path = "/api/v1/rules/{ref}",
    tag = "rules",
    params(
        ("ref" = String, Path, description = "Rule reference")
    ),
    request_body = UpdateRuleRequest,
    responses(
        (status = 200, description = "Rule updated successfully", body = ApiResponse<RuleResponse>),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Rule not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn update_rule(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(rule_ref): Path<String>,
    Json(request): Json<UpdateRuleRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate request
    request.validate()?;

    // Check if rule exists
    let existing_rule = RuleRepository::find_by_ref(&state.db, &rule_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Rule '{}' not found", rule_ref)))?;

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = state.authorization_service();
        let mut ctx = AuthorizationContext::new(identity_id);
        ctx.target_id = Some(existing_rule.id);
        ctx.target_ref = Some(existing_rule.r#ref.clone());
        ctx.pack_ref = Some(existing_rule.pack_ref.clone());
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Rules,
                    action: Action::Update,
                    context: ctx,
                },
            )
            .await?;
    }

    let action_ref = request
        .action_ref
        .as_deref()
        .unwrap_or(&existing_rule.action_ref);
    let action = ActionRepository::find_by_ref(&state.db, action_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", action_ref)))?;
    ensure_action_reference_allowed(
        &action,
        Some(&existing_rule.pack_ref),
        "rule",
        &existing_rule.r#ref,
    )?;

    let trigger_ref = request
        .trigger_ref
        .as_deref()
        .unwrap_or(&existing_rule.trigger_ref);
    let trigger = TriggerRepository::find_by_ref(&state.db, trigger_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;
    ensure_trigger_reference_allowed(
        &trigger,
        Some(&existing_rule.pack_ref),
        "rule",
        &existing_rule.r#ref,
    )?;

    if request.action_params.is_some() || request.action_ref.is_some() {
        validate_action_params(
            &action,
            request
                .action_params
                .as_ref()
                .unwrap_or(&existing_rule.action_params),
        )?;
    }

    if request.trigger_params.is_some() || request.trigger_ref.is_some() {
        validate_trigger_params(
            &trigger,
            request
                .trigger_params
                .as_ref()
                .unwrap_or(&existing_rule.trigger_params),
        )?;
    }

    let permission_refs_to_validate = match &request.permission_set_refs {
        Some(Some(refs)) => Some(refs.clone()),
        Some(None) => Some(action.default_execution_permission_set_refs.clone()),
        None if request.action_ref.is_some() => {
            // Action is changing — re-validate the effective permissions (either the
            // rule's explicit override or the new action's defaults) to prevent a user
            // from redirecting privileged permissions to an action they control.
            Some(
                existing_rule
                    .permission_set_refs
                    .clone()
                    .unwrap_or_else(|| action.default_execution_permission_set_refs.clone()),
            )
        }
        None => None,
    };
    if let Some(permission_refs_to_validate) = permission_refs_to_validate {
        if !permission_refs_to_validate.is_empty()
            && !state
                .authorization_service()
                .can_delegate_permission_sets(&user, &permission_refs_to_validate)
                .await?
        {
            return Err(ApiError::Forbidden(
                "Cannot update rule with execution permission sets beyond current access"
                    .to_string(),
            ));
        }
    }

    let authorized_rule_id = existing_rule.id;
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_mutations(&mut tx).await?;
    let existing_rule = RuleRepository::find_by_ref(&mut *tx, &rule_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Rule '{}' not found", rule_ref)))?;
    if existing_rule.id != authorized_rule_id {
        return Err(ApiError::Conflict(format!(
            "Rule '{}' changed while the update was being authorized",
            rule_ref
        )));
    }
    let action_ref = request
        .action_ref
        .as_deref()
        .unwrap_or(&existing_rule.action_ref);
    let action = ActionRepository::find_by_ref(&mut *tx, action_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", action_ref)))?;
    let trigger_ref = request
        .trigger_ref
        .as_deref()
        .unwrap_or(&existing_rule.trigger_ref);
    let trigger = TriggerRepository::find_by_ref(&mut *tx, trigger_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;
    ensure_action_reference_allowed(
        &action,
        Some(&existing_rule.pack_ref),
        "rule",
        &existing_rule.r#ref,
    )?;
    ensure_trigger_reference_allowed(
        &trigger,
        Some(&existing_rule.pack_ref),
        "rule",
        &existing_rule.r#ref,
    )?;
    if request.action_params.is_some() || request.action_ref.is_some() {
        validate_action_params(
            &action,
            request
                .action_params
                .as_ref()
                .unwrap_or(&existing_rule.action_params),
        )?;
    }
    if request.trigger_params.is_some() || request.trigger_ref.is_some() {
        validate_trigger_params(
            &trigger,
            request
                .trigger_params
                .as_ref()
                .unwrap_or(&existing_rule.trigger_params),
        )?;
    }

    let trigger_ref_changed = request
        .trigger_ref
        .as_ref()
        .is_some_and(|value| value != &existing_rule.trigger_ref);
    let trigger_params_changed = request
        .trigger_params
        .as_ref()
        .is_some_and(|value| value != &existing_rule.trigger_params);
    let enabled_after_update = request.enabled.unwrap_or(existing_rule.enabled);
    let was_enabled = existing_rule.enabled;
    let became_enabled = !was_enabled && enabled_after_update;
    let became_disabled = was_enabled && !enabled_after_update;
    let sensor_placement = if request.sensor_worker_selector.is_some()
        || request.sensor_worker_tolerations.is_some()
        || request.sensor_worker_affinity.is_some()
    {
        let placement = RuleSensorPlacementInput {
            selector: request
                .sensor_worker_selector
                .clone()
                .unwrap_or_else(|| existing_rule.sensor_worker_selector.clone()),
            tolerations: request
                .sensor_worker_tolerations
                .clone()
                .unwrap_or_else(|| existing_rule.sensor_worker_tolerations.clone()),
            affinity: request
                .sensor_worker_affinity
                .clone()
                .unwrap_or_else(|| existing_rule.sensor_worker_affinity.clone()),
        };
        parse_rule_sensor_placement(
            &placement.selector,
            &placement.tolerations,
            &placement.affinity,
        )?;
        Some(placement)
    } else {
        None
    };
    let trigger_config_changed =
        trigger_ref_changed || trigger_params_changed || sensor_placement.is_some();
    let admission_changed = trigger_ref_changed || sensor_placement.is_some() || became_enabled;

    // Create update input
    let update_input = UpdateRuleInput {
        label: request.label,
        description: request.description.map(Patch::Set),
        action: request.action_ref.as_ref().map(|_| action.id),
        action_ref: request.action_ref,
        trigger: request.trigger_ref.as_ref().map(|_| trigger.id),
        trigger_ref: request.trigger_ref,
        conditions: request.conditions,
        action_params: request.action_params,
        trigger_params: request.trigger_params,
        trace_tag_template: request.trace_tag_template.map(|template| match template {
            Some(template) => Patch::Set(template),
            None => Patch::Clear,
        }),
        permission_set_refs: request.permission_set_refs.map(|refs| match refs {
            Some(refs) => Patch::Set(refs),
            None => Patch::Clear,
        }),
        enabled: request.enabled,
        ..Default::default()
    };

    let mut rule = RuleRepository::update(&mut *tx, existing_rule.id, update_input).await?;
    if let Some(sensor_placement) = sensor_placement {
        rule =
            RuleRepository::update_sensor_placement(&mut *tx, existing_rule.id, sensor_placement)
                .await?;
    }
    if admission_changed {
        let requirement = if rule.enabled {
            SensorAdmissionRequirement::Live
        } else {
            SensorAdmissionRequirement::Structural
        };
        reject_sensor_admission(
            SensorAdmissionRepository::assess_rule(&mut tx, rule.id, requirement).await?,
        )?;
    }
    tx.commit().await?;

    if let Some(publisher) = state.get_publisher().await {
        if became_disabled || (was_enabled && trigger_ref_changed) {
            let payload = RuleDisabledPayload {
                rule_id: rule.id,
                rule_ref: rule.r#ref.clone(),
                trigger_ref: if trigger_ref_changed {
                    existing_rule.trigger_ref.clone()
                } else {
                    rule.trigger_ref.clone()
                },
            };

            let envelope =
                MessageEnvelope::new(MessageType::RuleDisabled, payload).with_source("api-service");

            if let Err(e) = publisher.publish_envelope(&envelope).await {
                warn!(
                    "Failed to publish RuleDisabled message for updated rule {}: {}",
                    rule.r#ref, e
                );
            } else {
                info!(
                    "Published RuleDisabled message for updated rule {}",
                    rule.r#ref
                );
            }
        }

        if became_enabled || (rule.enabled && trigger_config_changed) {
            let payload = RuleEnabledPayload {
                rule_id: rule.id,
                rule_ref: rule.r#ref.clone(),
                trigger_ref: rule.trigger_ref.clone(),
                trigger_params: Some(rule.trigger_params.clone()),
            };

            let envelope =
                MessageEnvelope::new(MessageType::RuleEnabled, payload).with_source("api-service");

            if let Err(e) = publisher.publish_envelope(&envelope).await {
                warn!(
                    "Failed to publish RuleEnabled message for updated rule {}: {}",
                    rule.r#ref, e
                );
            } else {
                info!(
                    "Published RuleEnabled message for updated rule {}",
                    rule.r#ref
                );
            }
        }
    }

    if became_disabled || (was_enabled && trigger_ref_changed) {
        if let Err(e) = notify_rule_lifecycle_changed(
            &state.db,
            "rule.disabled",
            rule.id,
            &rule.r#ref,
            if trigger_ref_changed {
                &existing_rule.trigger_ref
            } else {
                &rule.trigger_ref
            },
            None,
            false,
        )
        .await
        {
            warn!(
                "Failed to emit notifier rule.disabled update for rule {}: {}",
                rule.r#ref, e
            );
        }
    }

    if became_enabled || (rule.enabled && trigger_config_changed) {
        if let Err(e) = notify_rule_lifecycle_changed(
            &state.db,
            "rule.enabled",
            rule.id,
            &rule.r#ref,
            &rule.trigger_ref,
            Some(&rule.trigger_params),
            true,
        )
        .await
        {
            warn!(
                "Failed to emit notifier rule.enabled update for rule {}: {}",
                rule.r#ref, e
            );
        }
    }

    let response = ApiResponse::with_message(RuleResponse::from(rule), "Rule updated successfully");

    Ok((StatusCode::OK, Json(response)))
}

/// Delete a rule
#[utoipa::path(
    delete,
    path = "/api/v1/rules/{ref}",
    tag = "rules",
    params(
        ("ref" = String, Path, description = "Rule reference")
    ),
    responses(
        (status = 200, description = "Rule deleted successfully", body = SuccessResponse),
        (status = 404, description = "Rule not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(rule_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // Check if rule exists
    let rule = RuleRepository::find_by_ref(&state.db, &rule_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Rule '{}' not found", rule_ref)))?;

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = state.authorization_service();
        let mut ctx = AuthorizationContext::new(identity_id);
        ctx.target_id = Some(rule.id);
        ctx.target_ref = Some(rule.r#ref.clone());
        ctx.pack_ref = Some(rule.pack_ref.clone());
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Rules,
                    action: Action::Delete,
                    context: ctx,
                },
            )
            .await?;
    }

    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_mutations(&mut tx).await?;
    let deleted = RuleRepository::delete(&mut *tx, rule.id).await?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Rule '{}' not found", rule_ref)));
    }
    tx.commit().await?;

    if let Some(publisher) = state.get_publisher().await {
        let payload = RuleDeletedPayload {
            rule_id: rule.id,
            rule_ref: rule.r#ref.clone(),
            trigger_id: rule.trigger,
            trigger_ref: rule.trigger_ref.clone(),
        };

        let envelope =
            MessageEnvelope::new(MessageType::RuleDeleted, payload).with_source("api-service");

        if let Err(e) = publisher.publish_envelope(&envelope).await {
            warn!(
                "Failed to publish RuleDeleted message for rule {}: {}",
                rule.r#ref, e
            );
        } else {
            info!("Published RuleDeleted message for rule {}", rule.r#ref);
        }
    }

    if let Err(e) = notify_rule_lifecycle_changed(
        &state.db,
        "rule.deleted",
        rule.id,
        &rule.r#ref,
        &rule.trigger_ref,
        None,
        false,
    )
    .await
    {
        warn!(
            "Failed to emit notifier rule.deleted update for rule {}: {}",
            rule.r#ref, e
        );
    }

    let response = SuccessResponse::new(format!("Rule '{}' deleted successfully", rule_ref));

    Ok((StatusCode::OK, Json(response)))
}

/// Enable a rule
#[utoipa::path(
    post,
    path = "/api/v1/rules/{ref}/enable",
    tag = "rules",
    params(
        ("ref" = String, Path, description = "Rule reference")
    ),
    responses(
        (status = 200, description = "Rule enabled successfully", body = ApiResponse<RuleResponse>),
        (status = 404, description = "Rule not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn enable_rule(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(rule_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_mutations(&mut tx).await?;
    // Check if rule exists
    let existing_rule = RuleRepository::find_by_ref(&mut *tx, &rule_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Rule '{}' not found", rule_ref)))?;

    // Update rule to enabled
    let update_input = UpdateRuleInput {
        enabled: Some(true),
        ..Default::default()
    };

    let rule = RuleRepository::update(&mut *tx, existing_rule.id, update_input).await?;
    reject_sensor_admission(
        SensorAdmissionRepository::assess_rule(&mut tx, rule.id, SensorAdmissionRequirement::Live)
            .await?,
    )?;
    tx.commit().await?;

    // Publish RuleEnabled message to notify sensor service
    if let Some(publisher) = state.get_publisher().await {
        let payload = RuleEnabledPayload {
            rule_id: rule.id,
            rule_ref: rule.r#ref.clone(),
            trigger_ref: rule.trigger_ref.clone(),
            trigger_params: Some(rule.trigger_params.clone()),
        };

        let envelope =
            MessageEnvelope::new(MessageType::RuleEnabled, payload).with_source("api-service");

        if let Err(e) = publisher.publish_envelope(&envelope).await {
            warn!(
                "Failed to publish RuleEnabled message for rule {}: {}",
                rule.r#ref, e
            );
        } else {
            info!("Published RuleEnabled message for rule {}", rule.r#ref);
        }
    }
    if let Err(e) = notify_rule_lifecycle_changed(
        &state.db,
        "rule.enabled",
        rule.id,
        &rule.r#ref,
        &rule.trigger_ref,
        Some(&rule.trigger_params),
        true,
    )
    .await
    {
        warn!(
            "Failed to emit notifier rule.enabled update for rule {}: {}",
            rule.r#ref, e
        );
    }

    let response = ApiResponse::with_message(RuleResponse::from(rule), "Rule enabled successfully");

    Ok((StatusCode::OK, Json(response)))
}

/// Disable a rule
#[utoipa::path(
    post,
    path = "/api/v1/rules/{ref}/disable",
    tag = "rules",
    params(
        ("ref" = String, Path, description = "Rule reference")
    ),
    responses(
        (status = 200, description = "Rule disabled successfully", body = ApiResponse<RuleResponse>),
        (status = 404, description = "Rule not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn disable_rule(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(rule_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let mut tx = state.db.begin().await?;
    SensorAdmissionRepository::lock_mutations(&mut tx).await?;
    // Check if rule exists
    let existing_rule = RuleRepository::find_by_ref(&mut *tx, &rule_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Rule '{}' not found", rule_ref)))?;

    // Update rule to disabled
    let update_input = UpdateRuleInput {
        enabled: Some(false),
        ..Default::default()
    };

    let rule = RuleRepository::update(&mut *tx, existing_rule.id, update_input).await?;
    tx.commit().await?;

    // Publish RuleDisabled message to notify sensor service
    if let Some(publisher) = state.get_publisher().await {
        let payload = RuleDisabledPayload {
            rule_id: rule.id,
            rule_ref: rule.r#ref.clone(),
            trigger_ref: rule.trigger_ref.clone(),
        };

        let envelope =
            MessageEnvelope::new(MessageType::RuleDisabled, payload).with_source("api-service");

        if let Err(e) = publisher.publish_envelope(&envelope).await {
            warn!(
                "Failed to publish RuleDisabled message for rule {}: {}",
                rule.r#ref, e
            );
        } else {
            info!("Published RuleDisabled message for rule {}", rule.r#ref);
        }
    }
    if let Err(e) = notify_rule_lifecycle_changed(
        &state.db,
        "rule.disabled",
        rule.id,
        &rule.r#ref,
        &rule.trigger_ref,
        None,
        false,
    )
    .await
    {
        warn!(
            "Failed to emit notifier rule.disabled update for rule {}: {}",
            rule.r#ref, e
        );
    }

    let response =
        ApiResponse::with_message(RuleResponse::from(rule), "Rule disabled successfully");

    Ok((StatusCode::OK, Json(response)))
}

/// Create rule routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rules", get(list_rules).post(create_rule))
        .route("/rules/enabled", get(list_enabled_rules))
        .route(
            "/rules/{ref}",
            get(get_rule).put(update_rule).delete(delete_rule),
        )
        .route("/rules/{ref}/enable", post(enable_rule))
        .route("/rules/{ref}/disable", post(disable_rule))
        .route("/packs/{pack_ref}/rules", get(list_rules_by_pack))
        .route("/actions/{action_ref}/rules", get(list_rules_by_action))
        .route("/triggers/{trigger_ref}/rules", get(list_rules_by_trigger))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::middleware::AuthenticatedUser;
    use attune_common::auth::jwt::Claims;

    #[test]
    fn test_rule_routes_structure() {
        // Just verify the router can be constructed
        let _router = routes();
    }

    fn sensor_user(trigger_types: serde_json::Value) -> AuthenticatedUser {
        AuthenticatedUser {
            claims: Claims {
                sub: "1".to_string(),
                login: "sensor.demo".to_string(),
                iat: 1,
                exp: 2,
                token_type: TokenType::Sensor,
                scope: Some("sensor".to_string()),
                metadata: Some(serde_json::json!({
                    "trigger_types": trigger_types
                })),
            },
        }
    }

    #[test]
    fn sensor_tokens_require_explicit_trigger_scope_for_rule_listings() {
        let result = ensure_sensor_trigger_scope_for_rule_listing(
            &sensor_user(serde_json::json!(["core.timer"])),
            None,
        );
        assert!(matches!(result, Err(ApiError::Forbidden(_))));
    }

    #[test]
    fn sensor_tokens_reject_out_of_scope_trigger_rule_listings() {
        let result = ensure_sensor_trigger_scope_for_rule_listing(
            &sensor_user(serde_json::json!(["core.timer"])),
            Some("core.webhook"),
        );
        assert!(matches!(result, Err(ApiError::Forbidden(_))));
    }

    #[test]
    fn sensor_tokens_allow_in_scope_trigger_rule_listings() {
        let result = ensure_sensor_trigger_scope_for_rule_listing(
            &sensor_user(serde_json::json!(["core.timer"])),
            Some("core.timer"),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn sensor_tokens_cannot_use_non_trigger_scoped_rule_endpoints() {
        let result = ensure_sensor_trigger_scoped_rule_endpoint(
            &sensor_user(serde_json::json!(["core.timer"])),
            "/api/v1/rules/enabled",
        );

        match result {
            Err(ApiError::Forbidden(message)) => {
                assert!(message.contains("trigger-scoped endpoints"));
                assert!(message.contains("/api/v1/rules?trigger_ref="));
            }
            other => panic!("expected forbidden error, got {other:?}"),
        }
    }

    #[test]
    fn sensor_tokens_can_read_rule_details_only_within_trigger_scope() {
        let allowed = ensure_sensor_trigger_scope_for_rule_read(
            &sensor_user(serde_json::json!(["core.timer"])),
            "core.demo_rule",
            "core.timer",
        );
        assert!(allowed.is_ok());

        let forbidden = ensure_sensor_trigger_scope_for_rule_read(
            &sensor_user(serde_json::json!(["core.timer"])),
            "core.demo_rule",
            "core.webhook",
        );
        match forbidden {
            Err(ApiError::Forbidden(message)) => {
                assert!(message.contains("core.demo_rule"));
                assert!(message.contains("core.webhook"));
                assert!(message.contains("Allowed trigger_refs: core.timer"));
            }
            other => panic!("expected forbidden error, got {other:?}"),
        }
    }

    #[test]
    fn non_sensor_tokens_preserve_existing_rule_list_access() {
        let user = AuthenticatedUser {
            claims: Claims {
                sub: "1".to_string(),
                login: "testuser".to_string(),
                iat: 1,
                exp: 2,
                token_type: TokenType::Access,
                scope: None,
                metadata: None,
            },
        };

        assert!(ensure_sensor_trigger_scope_for_rule_listing(&user, None).is_ok());
        assert!(ensure_sensor_trigger_scope_for_rule_listing(&user, Some("core.timer")).is_ok());
        assert!(ensure_sensor_trigger_scoped_rule_endpoint(&user, "/api/v1/rules/enabled").is_ok());
        assert!(
            ensure_sensor_trigger_scope_for_rule_read(&user, "core.rule", "core.timer").is_ok()
        );
    }
}
