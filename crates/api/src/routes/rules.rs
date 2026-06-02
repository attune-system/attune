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

use attune_common::mq::{
    MessageEnvelope, MessageType, RuleCreatedPayload, RuleDisabledPayload, RuleEnabledPayload,
};
use attune_common::rbac::{Action, AuthorizationContext, Resource};
use attune_common::repositories::{
    action::ActionRepository,
    pack::PackRepository,
    rule::{CreateRuleInput, RuleRepository, RuleSearchFilters, UpdateRuleInput},
    trigger::TriggerRepository,
    Create, Delete, FindByRef, Patch, Update,
};

use crate::{
    auth::middleware::RequireAuth,
    authz::{AuthorizationCheck, AuthorizationService},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        rule::{CreateRuleRequest, RuleListParams, RuleResponse, RuleSummary, UpdateRuleRequest},
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
    validation::{validate_action_params, validate_trigger_params},
};

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
    RequireAuth(_user): RequireAuth,
    Query(query): Query<RuleListParams>,
) -> ApiResult<impl IntoResponse> {
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
    RequireAuth(_user): RequireAuth,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    let filters = RuleSearchFilters {
        pack: None,
        pack_ref: None,
        action: None,
        action_ref: None,
        trigger: None,
        trigger_ref: None,
        enabled: Some(true),
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
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of rules in pack", body = PaginatedResponse<RuleSummary>),
        (status = 404, description = "Pack not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_rules_by_pack(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
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
    RequireAuth(_user): RequireAuth,
    Path(action_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
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
    RequireAuth(_user): RequireAuth,
    Path(trigger_ref): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
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
    RequireAuth(_user): RequireAuth,
    Path(rule_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let rule = RuleRepository::find_by_ref(&state.db, &rule_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Rule '{}' not found", rule_ref)))?;

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

    // Check if rule with same ref already exists
    if RuleRepository::find_by_ref(&state.db, &request.r#ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Rule with ref '{}' already exists",
            request.r#ref
        )));
    }

    // Verify pack exists and get its ID
    let pack = PackRepository::find_by_ref(&state.db, &request.pack_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Pack '{}' not found", request.pack_ref)))?;

    // Verify action exists and get its ID
    let action = ActionRepository::find_by_ref(&state.db, &request.action_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", request.action_ref)))?;

    // Verify trigger exists and get its ID
    let trigger = TriggerRepository::find_by_ref(&state.db, &request.trigger_ref)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Trigger '{}' not found", request.trigger_ref))
        })?;

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = AuthorizationService::new(state.db.clone());
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
        && !AuthorizationService::new(state.db.clone())
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
        permission_set_refs: request.permission_set_refs,
        enabled: request.enabled,
        is_adhoc: true, // Rules created via API are ad-hoc (not from pack installation)
        owner_identity,
    };

    let rule = RuleRepository::create(&state.db, rule_input).await?;

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
        let authz = AuthorizationService::new(state.db.clone());
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

    let trigger_ref = request
        .trigger_ref
        .as_deref()
        .unwrap_or(&existing_rule.trigger_ref);
    let trigger = TriggerRepository::find_by_ref(&state.db, trigger_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;

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
            && !AuthorizationService::new(state.db.clone())
                .can_delegate_permission_sets(&user, &permission_refs_to_validate)
                .await?
        {
            return Err(ApiError::Forbidden(
                "Cannot update rule with execution permission sets beyond current access"
                    .to_string(),
            ));
        }
    }

    let trigger_ref_changed = request
        .trigger_ref
        .as_ref()
        .is_some_and(|value| value != &existing_rule.trigger_ref);
    let trigger_params_changed = request
        .trigger_params
        .as_ref()
        .is_some_and(|value| value != &existing_rule.trigger_params);
    let trigger_config_changed = trigger_ref_changed || trigger_params_changed;
    let enabled_after_update = request.enabled.unwrap_or(existing_rule.enabled);
    let was_enabled = existing_rule.enabled;
    let became_enabled = !was_enabled && enabled_after_update;
    let became_disabled = was_enabled && !enabled_after_update;

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
        permission_set_refs: request.permission_set_refs.map(|refs| match refs {
            Some(refs) => Patch::Set(refs),
            None => Patch::Clear,
        }),
        enabled: request.enabled,
        ..Default::default()
    };

    let rule = RuleRepository::update(&state.db, existing_rule.id, update_input).await?;

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
        let authz = AuthorizationService::new(state.db.clone());
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

    // Delete the rule
    let deleted = RuleRepository::delete(&state.db, rule.id).await?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Rule '{}' not found", rule_ref)));
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
    // Check if rule exists
    let existing_rule = RuleRepository::find_by_ref(&state.db, &rule_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Rule '{}' not found", rule_ref)))?;

    // Update rule to enabled
    let update_input = UpdateRuleInput {
        enabled: Some(true),
        ..Default::default()
    };

    let rule = RuleRepository::update(&state.db, existing_rule.id, update_input).await?;

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
    // Check if rule exists
    let existing_rule = RuleRepository::find_by_ref(&state.db, &rule_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Rule '{}' not found", rule_ref)))?;

    // Update rule to disabled
    let update_input = UpdateRuleInput {
        enabled: Some(false),
        ..Default::default()
    };

    let rule = RuleRepository::update(&state.db, existing_rule.id, update_input).await?;

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

    #[test]
    fn test_rule_routes_structure() {
        // Just verify the router can be constructed
        let _router = routes();
    }
}
