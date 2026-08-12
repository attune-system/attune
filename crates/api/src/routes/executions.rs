//! Execution management API routes

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::get,
    Json, Router,
};
use chrono::Utc;
use futures::stream::{Stream, StreamExt};
use sqlx::{Postgres, QueryBuilder};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_stream::wrappers::BroadcastStream;

use attune_common::models::enums::ActionReferenceVisibility;
use attune_common::models::enums::ExecutionStatus;
use attune_common::models::enums::RetentionPolicyType;
use attune_common::mq::{
    ExecutionCancelRequestedPayload, ExecutionRequestedPayload, MessageEnvelope, MessageType,
    Publisher,
};
use attune_common::repositories::{
    action::ActionRepository,
    artifact::{ArtifactRepository, ArtifactVersionRepository},
    execution::{
        CreateExecutionInput, ExecutionRepository, ExecutionSearchFilters, ExecutionSearchResult,
        ExecutionWithRefs, UpdateExecutionInput,
    },
    execution_secret_value::ExecutionSecretValueRepository,
    maintenance::MaintenanceRepository,
    workflow::{WorkflowDefinitionRepository, WorkflowExecutionRepository},
    Create, FindById, FindByRef, Update, WorkflowCacheIterationRepository,
};
use attune_common::scheduling::{
    parse_worker_affinity, parse_worker_selector, parse_worker_tolerations,
};
use attune_common::secret_values::{
    prepare_secret_values, redact_secret_parameters, redacted_paths, restore_secret_values,
    ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT,
};
use attune_common::trace_tag::manual_trace_tag;
use attune_common::workflow::{CancellationPolicy, WorkflowDefinition};

use crate::{
    auth::{
        jwt::{Claims, TokenType},
        middleware::{AuthenticatedUser, RequireAuth},
    },
    authz::{AuthorizationCheck, AuthorizationSnapshot},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        execution::{
            CreateExecutionRequest, ExecutionDetailQueryParams, ExecutionQueryParams,
            ExecutionRescheduleResponse, ExecutionResponse, ExecutionSummary,
            WorkflowCacheIterationResponse,
        },
        ApiResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::rbac::{
    Action, AuthorizationContext, ExecutionScopeConstraint, Grant, GrantConstraints,
    OwnerConstraint, Resource,
};

const LOG_STREAM_POLL_INTERVAL: Duration = Duration::from_millis(250);
const LOG_STREAM_READ_CHUNK_SIZE: usize = 64 * 1024;

/// Create a new execution (manual execution)
///
/// This endpoint allows directly executing an action without a trigger or rule.
/// The execution is queued and will be picked up by the executor service.
#[utoipa::path(
    post,
    path = "/api/v1/executions/execute",
    tag = "executions",
    request_body = CreateExecutionRequest,
    responses(
        (status = 201, description = "Execution created and queued", body = ExecutionResponse),
        (status = 404, description = "Action not found"),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_execution(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Json(request): Json<CreateExecutionRequest>,
) -> ApiResult<impl IntoResponse> {
    // Validate that the action exists
    let action = ActionRepository::find_by_ref(&state.db, &request.action_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Action '{}' not found", request.action_ref)))?;
    if !action.enabled {
        return Err(ApiError::BadRequest(format!(
            "Action '{}' is disabled",
            request.action_ref
        )));
    }

    let authz = state.authorization_service();
    // Load identity attributes + effective grants once (for Access/Execution
    // tokens) and reuse them below for both the action-execute check and the
    // permission-set delegation check, instead of re-fetching per check.
    // Returns `None` for token types not subject to identity-based RBAC,
    // matching the original per-check bypass behavior.
    let authz_snapshot = authz.load_snapshot(&user).await?;

    if matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;

        let mut action_ctx = AuthorizationContext::new(identity_id);
        action_ctx.target_id = Some(action.id);
        action_ctx.target_ref = Some(action.r#ref.clone());
        action_ctx.pack_ref = Some(action.pack_ref.clone());

        authz.authorize_with_snapshot(
            &user,
            authz_snapshot.as_ref(),
            AuthorizationCheck {
                resource: Resource::Actions,
                action: Action::Execute,
                context: action_ctx,
            },
        )?;
    }

    // When the request is authenticated with an execution-scoped token (e.g.,
    // an MCP client invoked from inside a running execution), automatically
    // attribute the new execution as a child of the originating execution.
    // The execution_id is encoded in the JWT claims at token-mint time and
    // cannot be forged or overridden by the caller.
    let parent_from_token = if user.claims.token_type == TokenType::Execution {
        user.claims
            .metadata
            .as_ref()
            .and_then(|m| m.get("execution_id"))
            .and_then(|v| v.as_i64())
    } else {
        None
    };
    let parent_execution = if let Some(parent_id) = parent_from_token {
        ExecutionRepository::find_by_id(&state.db, parent_id).await?
    } else {
        None
    };

    // SECURITY: Record the triggering identity on the execution so that the
    // worker mints the execution-scoped API token (`ATTUNE_API_TOKEN`) with
    // that identity's `sub` claim. This ensures callbacks from the action are
    // subject to the same RBAC as the user who triggered them.
    //
    // For execution-token callers (e.g., the MCP server inside a running
    // action), inherit the executor of the originating execution rather than
    // using the token's `sub` directly — this preserves the security context
    // across nested calls.
    let executor_identity = match user.claims.token_type {
        TokenType::Access => user.identity_id().ok(),
        TokenType::Execution => parent_execution
            .as_ref()
            .and_then(|p| p.executor)
            .or_else(|| user.identity_id().ok()),
        // Sensor / refresh tokens are not expected here; fall back to the
        // claimed identity if present.
        _ => user.identity_id().ok(),
    };

    let permission_set_refs = request
        .permission_set_refs
        .clone()
        .unwrap_or_else(|| action.default_execution_permission_set_refs.clone());
    if !permission_set_refs.is_empty()
        && !authz
            .can_delegate_permission_sets_with_snapshot(
                &user,
                authz_snapshot.as_ref(),
                &permission_set_refs,
            )
            .await?
    {
        return Err(ApiError::Forbidden(
            "Cannot execute action with permission sets beyond current access".to_string(),
        ));
    }

    if let Some(worker_selector) = &request.worker_selector {
        parse_worker_selector(worker_selector)
            .map_err(|e| ApiError::BadRequest(format!("Invalid worker_selector: {e}")))?;
    }
    if let Some(worker_tolerations) = &request.worker_tolerations {
        parse_worker_tolerations(worker_tolerations)
            .map_err(|e| ApiError::BadRequest(format!("Invalid worker_tolerations: {e}")))?;
    }
    if let Some(worker_affinity) = &request.worker_affinity {
        parse_worker_affinity(worker_affinity)
            .map_err(|e| ApiError::BadRequest(format!("Invalid worker_affinity: {e}")))?;
    }
    if let Some(limit) = request.artifact_retention_limit {
        if limit <= 0 {
            return Err(ApiError::BadRequest(
                "artifact_retention_limit must be greater than zero".to_string(),
            ));
        }
    }

    if let Some(timeout) = request.timeout_seconds {
        if timeout <= 0 {
            return Err(ApiError::BadRequest(
                "timeout_seconds must be greater than zero".to_string(),
            ));
        }
    }

    // Snapshot the resolved execution timeout: explicit request override ->
    // action default -> app-level default_execution_timeout_seconds.
    let timeout_seconds = Some(
        request
            .timeout_seconds
            .or(action.timeout_seconds)
            .unwrap_or(state.config.default_execution_timeout_seconds as i32),
    );
    let inherited_trace_tag = parent_execution
        .as_ref()
        .and_then(|parent| parent.trace_tag.clone());
    let manual_trace_fallback =
        if inherited_trace_tag.is_none() && user.claims.token_type == TokenType::Access {
            Some(
                manual_trace_tag(user.login(), Utc::now().timestamp_millis()).map_err(|e| {
                    ApiError::InternalServerError(format!("Failed to build manual trace tag: {e}"))
                })?,
            )
        } else {
            None
        };

    let artifact_retention_policy = request
        .artifact_retention_policy
        .or(action.artifact_retention_policy)
        .or(Some(RetentionPolicyType::Versions));
    let artifact_retention_limit = request
        .artifact_retention_limit
        .or(action.artifact_retention_limit)
        .or(Some(5));

    let raw_config = request
        .parameters
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    let (redacted_config, secret_inputs) =
        redact_secret_parameters(raw_config, action.param_schema.as_ref());
    let config_for_storage = if redacted_config.is_null()
        || redacted_config
            .as_object()
            .is_some_and(|obj| obj.is_empty())
    {
        None
    } else {
        Some(redacted_config.clone())
    };
    let prepared_secrets = if secret_inputs.is_empty() {
        Vec::new()
    } else {
        let encryption_key = state
            .config
            .security
            .encryption_key
            .as_ref()
            .ok_or_else(|| {
                ApiError::InternalServerError(
                    "Cannot store secret execution parameters without security.encryption_key"
                        .to_string(),
                )
            })?;
        prepare_secret_values(secret_inputs, encryption_key).map_err(|e| {
            ApiError::InternalServerError(format!(
                "Failed to encrypt secret execution parameters: {e}"
            ))
        })?
    };

    // Create execution input
    let execution_input = CreateExecutionInput {
        action: Some(action.id),
        action_ref: action.r#ref.clone(),
        config: config_for_storage,
        env_vars: request
            .env_vars
            .as_ref()
            .and_then(|e| serde_json::from_value(e.clone()).ok()),
        parent: parent_from_token,
        enforcement: None,
        executor: executor_identity,
        permission_set_refs,
        artifact_retention_policy,
        artifact_retention_limit,
        worker_selector: request.worker_selector.clone(),
        worker_tolerations: request.worker_tolerations.clone(),
        worker_affinity: request.worker_affinity.clone(),
        worker: None,
        status: ExecutionStatus::Requested,
        trace_tag: inherited_trace_tag.or(manual_trace_fallback),
        timeout_seconds,
        result: None,
        workflow_task: None, // Non-workflow execution
    };

    // Insert into database
    let created_execution = ExecutionRepository::create(&state.db, execution_input).await?;
    ExecutionSecretValueRepository::upsert_many(
        &state.db,
        ENTITY_EXECUTION_CONFIG,
        created_execution.id,
        &prepared_secrets,
    )
    .await?;

    // Publish ExecutionRequested message to queue
    let payload = ExecutionRequestedPayload {
        execution_id: created_execution.id,
        action_id: Some(action.id),
        action_ref: action.r#ref.clone(),
        parent_id: parent_from_token,
        enforcement_id: None,
        config: created_execution.config.clone(),
    };

    let message = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
        .with_source("api-service")
        .with_correlation_id(uuid::Uuid::new_v4());

    if let Some(publisher) = state.get_publisher().await {
        publisher.publish_envelope(&message).await.map_err(|e| {
            ApiError::InternalServerError(format!("Failed to publish message: {}", e))
        })?;
    }

    let response = ExecutionResponse::from(created_execution);

    // Audit: explicit semantic event for manual execution requests so the
    // audit log shows *what* action was kicked off, not just "POST /api/v1/
    // executions/execute".
    {
        use attune_common::audit::{AuditCategory, AuditEventBuilder, AuditOutcome};
        let mut builder = AuditEventBuilder::new(
            AuditCategory::Execution,
            "execution.requested",
            AuditOutcome::Success,
        )
        .resource("executions")
        .resource_id(response.id)
        .resource_ref(response.action_ref.clone());
        if let Ok(id) = user.identity_id() {
            builder = builder.actor_identity(id);
        }
        builder = builder.actor_login(user.login().to_string());
        builder = builder.actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase());
        let details = serde_json::json!({
            "action_ref": response.action_ref,
            "action_id": action.id,
            "parent_execution_id": parent_from_token,
            "executor_identity": executor_identity,
        });
        builder = builder.with_details(details);
        state.audit_emitter.emit(builder.build());
    }

    Ok((StatusCode::CREATED, Json(ApiResponse::new(response))))
}

/// List all executions with pagination and optional filters
#[utoipa::path(
    get,
    path = "/api/v1/executions",
    tag = "executions",
    params(ExecutionQueryParams),
    responses(
        (status = 200, description = "List of executions", body = PaginatedResponse<ExecutionSummary>),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_executions(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<ExecutionQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let request_started = Instant::now();

    // Load identity attributes + effective grants once and reuse them for
    // both the collection-access check and the visibility-scoped search
    // below, instead of fetching them separately for each.
    let authz_snapshot = state.authorization_service().load_snapshot(&user).await?;
    authorize_execution_collection_access(&user, authz_snapshot.as_ref(), Action::Read).await?;

    let filters = ExecutionSearchFilters {
        status: query.status,
        action_ref: query.action_ref.clone(),
        pack_name: query.pack_name.clone(),
        rule_ref: query.rule_ref.clone(),
        trigger_ref: query.trigger_ref.clone(),
        trace_tag: query.trace_tag.clone(),
        executor: query.executor,
        result_contains: query.result_contains.clone(),
        enforcement: query.enforcement,
        parent: query.parent,
        top_level_only: query.top_level_only == Some(true),
        include_total: query.include_total == Some(true),
        limit: query.limit(),
        offset: query.offset(),
    };
    let pagination_params = PaginationParams {
        page: query.page,
        page_size: query.per_page,
    };
    let result = search_authorized_executions(
        &state,
        &user,
        authz_snapshot.as_ref(),
        &filters,
        Action::Read,
    )
    .await?;
    let items: Vec<ExecutionSummary> = result
        .rows
        .into_iter()
        .map(ExecutionSummary::from)
        .collect();
    let response = if let Some(total) = result.total {
        PaginatedResponse::new(items, &pagination_params, total)
    } else {
        PaginatedResponse::without_totals(items, &pagination_params, result.has_next)
    };

    let elapsed = request_started.elapsed();
    if elapsed > Duration::from_millis(1500) {
        tracing::warn!(
            elapsed_ms = elapsed.as_millis(),
            include_total = query.include_total.unwrap_or(false),
            page = query.page,
            per_page = query.per_page,
            constrained = true,
            "slow list_executions request"
        );
    }

    Ok((StatusCode::OK, Json(response)))
}

fn grants_include_execution_action(grants: &[Grant], action: Action) -> bool {
    grants
        .iter()
        .any(|grant| grant.resource == Resource::Executions && grant.actions.contains(&action))
}

fn has_unconstrained_execution_access(grants: &[Grant], action: Action) -> bool {
    grants.iter().any(|grant| {
        grant.resource == Resource::Executions
            && grant.actions.contains(&action)
            && grant.constraints.is_none()
    })
}

async fn authorize_execution_collection_access(
    user: &AuthenticatedUser,
    snapshot: Option<&AuthorizationSnapshot>,
    action: Action,
) -> Result<(), ApiError> {
    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return Ok(());
    }

    let Some(snapshot) = snapshot else {
        return Err(ApiError::Unauthorized(
            "Invalid authentication subject in token".to_string(),
        ));
    };

    if matches!(action, Action::Read) && user.claims.token_type == TokenType::Access {
        return Ok(());
    }

    if grants_include_execution_action(&snapshot.grants, action) {
        return Ok(());
    }

    Err(ApiError::Forbidden(format!(
        "Insufficient permissions: executions:{}",
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
    )))
}

#[derive(Debug, Clone)]
struct ExecutionVisibilityGrant {
    pack_refs: Option<Vec<String>>,
    refs: Option<Vec<String>>,
    ids: Option<Vec<i64>>,
    owner: Option<OwnerConstraint>,
    execution_scope: Option<ExecutionScopeConstraint>,
}

#[derive(Debug, sqlx::FromRow)]
struct ExecutionStatusCountRow {
    status: ExecutionStatus,
    count: i64,
}

fn execution_ref_filter_like_pattern(filter: &str) -> Option<String> {
    if !filter.contains('*') {
        return None;
    }

    let mut pattern = String::with_capacity(filter.len());
    for ch in filter.chars() {
        match ch {
            '*' => pattern.push('%'),
            '\\' => pattern.push_str(r"\\"),
            '%' => pattern.push_str(r"\%"),
            '_' => pattern.push_str(r"\_"),
            ch => pattern.push(ch),
        }
    }

    Some(pattern)
}

fn execution_grant_constraints_supported(constraints: &GrantConstraints) -> bool {
    constraints.owner_types.is_none()
        && constraints.owner_refs.is_none()
        && constraints.visibility.is_none()
        && constraints.encrypted.is_none()
}

fn grant_attributes_match(
    constraints: &GrantConstraints,
    identity_attributes: &HashMap<String, serde_json::Value>,
) -> bool {
    let Some(expected) = &constraints.attributes else {
        return true;
    };
    expected
        .iter()
        .all(|(key, value)| identity_attributes.get(key) == Some(value))
}

fn collect_execution_visibility_grants(
    grants: &[Grant],
    action: Action,
    identity_attributes: &HashMap<String, serde_json::Value>,
) -> Vec<ExecutionVisibilityGrant> {
    grants
        .iter()
        .filter(|grant| grant.resource == Resource::Executions && grant.actions.contains(&action))
        .filter_map(|grant| {
            let Some(constraints) = &grant.constraints else {
                return Some(ExecutionVisibilityGrant {
                    pack_refs: None,
                    refs: None,
                    ids: None,
                    owner: None,
                    execution_scope: None,
                });
            };

            if !execution_grant_constraints_supported(constraints)
                || !grant_attributes_match(constraints, identity_attributes)
            {
                return None;
            }

            Some(ExecutionVisibilityGrant {
                pack_refs: constraints.pack_refs.clone(),
                refs: constraints.refs.clone(),
                ids: constraints.ids.clone(),
                owner: constraints.owner,
                execution_scope: constraints.execution_scope,
            })
        })
        .collect()
}

fn push_root_visibility_predicate(
    qb: &mut QueryBuilder<'_, Postgres>,
    grants: &[ExecutionVisibilityGrant],
    identity_id: i64,
    include_public_actions: bool,
) {
    qb.push("(");
    let mut has_clause = false;

    for grant in grants {
        if has_clause {
            qb.push(" OR ");
        }
        has_clause = true;
        qb.push("(TRUE");

        if let Some(pack_refs) = &grant.pack_refs {
            if pack_refs.is_empty() {
                qb.push(" AND FALSE");
            } else {
                qb.push(" AND split_part(root.action_ref, '.', 1) = ANY(");
                qb.push_bind(pack_refs.clone());
                qb.push(")");
            }
        }
        if let Some(refs) = &grant.refs {
            if refs.is_empty() {
                qb.push(" AND FALSE");
            } else {
                qb.push(" AND root.action_ref = ANY(");
                qb.push_bind(refs.clone());
                qb.push(")");
            }
        }
        if let Some(ids) = &grant.ids {
            if ids.is_empty() {
                qb.push(" AND FALSE");
            } else {
                qb.push(" AND root.id = ANY(");
                qb.push_bind(ids.clone());
                qb.push(")");
            }
        }

        if let Some(owner) = grant.owner {
            match owner {
                OwnerConstraint::SelfOnly => {
                    qb.push(" AND root.executor = ");
                    qb.push_bind(identity_id);
                }
                OwnerConstraint::Any => {}
                OwnerConstraint::None => {
                    qb.push(" AND root.executor IS NULL");
                }
            }
        }

        if matches!(
            grant.execution_scope,
            Some(ExecutionScopeConstraint::SelfOnly | ExecutionScopeConstraint::Descendants)
        ) {
            qb.push(" AND root.executor = ");
            qb.push_bind(identity_id);
        }

        qb.push(")");
    }

    if include_public_actions {
        if has_clause {
            qb.push(" OR ");
        }
        has_clause = true;
        qb.push(
            "EXISTS (\
                SELECT 1 \
                FROM action a \
                WHERE a.ref = root.action_ref \
                  AND a.reference_visibility = ",
        );
        qb.push_bind(ActionReferenceVisibility::Public);
        qb.push(")");
    }

    if !has_clause {
        qb.push("FALSE");
    }

    qb.push(")");
}

fn append_execution_search_filters(
    qb: &mut QueryBuilder<'_, Postgres>,
    filters: &ExecutionSearchFilters,
) {
    let mut has_where = false;
    macro_rules! push_condition {
        ($sql:expr, $value:expr) => {{
            if !has_where {
                qb.push(" WHERE ");
                has_where = true;
            } else {
                qb.push(" AND ");
            }
            qb.push($sql);
            qb.push_bind($value);
        }};
    }

    macro_rules! push_like_condition {
        ($sql:expr, $value:expr) => {{
            if !has_where {
                qb.push(" WHERE ");
                has_where = true;
            } else {
                qb.push(" AND ");
            }
            qb.push($sql);
            qb.push_bind($value);
            qb.push(r" ESCAPE '\'");
        }};
    }

    macro_rules! push_raw_condition {
        ($sql:expr) => {{
            if !has_where {
                qb.push(" WHERE ");
                has_where = true;
            } else {
                qb.push(" AND ");
            }
            qb.push($sql);
        }};
    }

    if let Some(status) = &filters.status {
        push_condition!("e.status = ", *status);
    }
    if let Some(action_ref) = &filters.action_ref {
        if let Some(pattern) = execution_ref_filter_like_pattern(action_ref) {
            push_like_condition!("e.action_ref LIKE ", pattern);
        } else {
            push_condition!("e.action_ref = ", action_ref.clone());
        }
    }
    if let Some(pack_name) = &filters.pack_name {
        push_condition!("split_part(e.action_ref, '.', 1) = ", pack_name.clone());
    }
    if let Some(enforcement_id) = filters.enforcement {
        push_condition!("e.enforcement = ", enforcement_id);
    }
    if let Some(parent_id) = filters.parent {
        push_condition!("e.parent = ", parent_id);
    }
    if filters.top_level_only {
        push_raw_condition!("e.parent IS NULL");
    }
    if let Some(executor_id) = filters.executor {
        push_condition!("e.executor = ", executor_id);
    }
    if let Some(rule_ref) = &filters.rule_ref {
        if let Some(pattern) = execution_ref_filter_like_pattern(rule_ref) {
            push_like_condition!("enf.rule_ref LIKE ", pattern);
        } else {
            push_condition!("enf.rule_ref = ", rule_ref.clone());
        }
    }
    if let Some(trigger_ref) = &filters.trigger_ref {
        if let Some(pattern) = execution_ref_filter_like_pattern(trigger_ref) {
            push_like_condition!("enf.trigger_ref LIKE ", pattern);
        } else {
            push_condition!("enf.trigger_ref = ", trigger_ref.clone());
        }
    }
    if let Some(trace_tag) = &filters.trace_tag {
        push_condition!("e.trace_tag = ", trace_tag.clone());
    }
    if let Some(search) = &filters.result_contains {
        push_condition!(
            "LOWER(e.result::text) LIKE ",
            format!("%{}%", search.to_lowercase())
        );
    }

    // Keep `has_where` as a true state variable for macros without unused-assignment warnings.
    let _ = has_where;
}

async fn search_authorized_executions(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    snapshot: Option<&AuthorizationSnapshot>,
    filters: &ExecutionSearchFilters,
    action: Action,
) -> Result<ExecutionSearchResult, ApiError> {
    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return ExecutionRepository::search(&state.db, filters)
            .await
            .map_err(Into::into);
    }

    let Some(snapshot) = snapshot else {
        return Err(ApiError::Unauthorized(
            "Invalid authentication subject in token".to_string(),
        ));
    };
    let identity_id = snapshot.identity_id;
    let identity_attributes = &snapshot.identity_attributes;
    let grants = &snapshot.grants;
    let include_public_actions =
        user.claims.token_type == TokenType::Access && matches!(action, Action::Read);

    if has_unconstrained_execution_access(grants, action) {
        return ExecutionRepository::search(&state.db, filters)
            .await
            .map_err(Into::into);
    }

    let visibility_grants =
        collect_execution_visibility_grants(grants, action, identity_attributes);
    if visibility_grants.is_empty() && !include_public_actions {
        return Ok(ExecutionSearchResult {
            rows: Vec::new(),
            total: if filters.include_total { Some(0) } else { None },
            has_next: false,
        });
    }

    let prefixed_select = attune_common::repositories::execution::SELECT_COLUMNS
        .split(", ")
        .map(|col| format!("e.{col}"))
        .collect::<Vec<_>>()
        .join(", ");
    let select_clause =
        format!("{prefixed_select}, enf.rule_ref AS rule_ref, enf.trigger_ref AS trigger_ref");

    let cte_prefix = "WITH RECURSIVE visible_roots AS (\
            SELECT root.id \
            FROM execution root \
            WHERE root.parent IS NULL AND ";
    let cte_suffix = "), visible_execs AS (\
            SELECT id FROM visible_roots \
            UNION ALL \
            SELECT child.id FROM execution child \
            INNER JOIN visible_execs visible ON child.parent = visible.id\
        ) ";

    let mut data_qb: QueryBuilder<'_, Postgres> = QueryBuilder::new(cte_prefix);
    push_root_visibility_predicate(
        &mut data_qb,
        &visibility_grants,
        identity_id,
        include_public_actions,
    );
    data_qb.push(cte_suffix);
    data_qb.push(format!(
        "SELECT {select_clause} \
         FROM execution e \
         INNER JOIN visible_execs ve ON ve.id = e.id \
         LEFT JOIN enforcement enf ON e.enforcement = enf.id"
    ));
    append_execution_search_filters(&mut data_qb, filters);
    data_qb.push(" ORDER BY e.created DESC");
    data_qb.push(" LIMIT ");
    let query_limit = if filters.include_total {
        filters.limit
    } else {
        filters.limit.saturating_add(1)
    };
    data_qb.push_bind(query_limit as i64);
    data_qb.push(" OFFSET ");
    data_qb.push_bind(filters.offset as i64);
    let mut rows: Vec<ExecutionWithRefs> = data_qb.build_query_as().fetch_all(&state.db).await?;

    let total = if filters.include_total {
        let mut count_qb: QueryBuilder<'_, Postgres> = QueryBuilder::new(cte_prefix);
        push_root_visibility_predicate(
            &mut count_qb,
            &visibility_grants,
            identity_id,
            include_public_actions,
        );
        count_qb.push(cte_suffix);
        count_qb.push(
            "SELECT COUNT(*) AS total \
             FROM execution e \
             INNER JOIN visible_execs ve ON ve.id = e.id \
             LEFT JOIN enforcement enf ON e.enforcement = enf.id",
        );
        append_execution_search_filters(&mut count_qb, filters);
        let (total,) = count_qb
            .build_query_as::<(i64,)>()
            .fetch_one(&state.db)
            .await?;
        Some(total.max(0) as u64)
    } else {
        None
    };

    let has_next = if let Some(total) = total {
        filters.offset as u64 + (rows.len() as u64) < total
    } else if rows.len() > filters.limit as usize {
        rows.truncate(filters.limit as usize);
        true
    } else {
        false
    };

    Ok(ExecutionSearchResult {
        rows,
        total,
        has_next,
    })
}

async fn load_authorized_execution_status_counts(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    snapshot: Option<&AuthorizationSnapshot>,
) -> Result<Vec<ExecutionStatusCountRow>, ApiError> {
    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return sqlx::query_as::<_, ExecutionStatusCountRow>(
            "SELECT status, COUNT(*)::BIGINT AS count FROM execution GROUP BY status",
        )
        .fetch_all(&state.db)
        .await
        .map_err(Into::into);
    }

    let Some(snapshot) = snapshot else {
        return Err(ApiError::Unauthorized(
            "Invalid authentication subject in token".to_string(),
        ));
    };
    let identity_id = snapshot.identity_id;
    let identity_attributes = &snapshot.identity_attributes;
    let grants = &snapshot.grants;
    let include_public_actions = user.claims.token_type == TokenType::Access;

    if has_unconstrained_execution_access(grants, Action::Read) {
        return sqlx::query_as::<_, ExecutionStatusCountRow>(
            "SELECT status, COUNT(*)::BIGINT AS count FROM execution GROUP BY status",
        )
        .fetch_all(&state.db)
        .await
        .map_err(Into::into);
    }

    let visibility_grants =
        collect_execution_visibility_grants(grants, Action::Read, identity_attributes);
    if visibility_grants.is_empty() && !include_public_actions {
        return Ok(Vec::new());
    }

    let mut qb: QueryBuilder<'_, Postgres> = QueryBuilder::new(
        "WITH RECURSIVE visible_roots AS (\
            SELECT root.id \
            FROM execution root \
            WHERE root.parent IS NULL AND ",
    );
    push_root_visibility_predicate(
        &mut qb,
        &visibility_grants,
        identity_id,
        include_public_actions,
    );
    qb.push(
        "), visible_execs AS (\
            SELECT id FROM visible_roots \
            UNION ALL \
            SELECT child.id FROM execution child \
            INNER JOIN visible_execs visible ON child.parent = visible.id\
        ) \
        SELECT e.status, COUNT(*)::BIGINT AS count \
        FROM execution e \
        INNER JOIN visible_execs ve ON ve.id = e.id \
        GROUP BY e.status",
    );

    qb.build_query_as::<ExecutionStatusCountRow>()
        .fetch_all(&state.db)
        .await
        .map_err(Into::into)
}

/// Get a single execution by ID
#[utoipa::path(
    get,
    path = "/api/v1/executions/{id}",
    tag = "executions",
    params(
        ("id" = i64, Path, description = "Execution ID"),
        ExecutionDetailQueryParams
    ),
    responses(
        (status = 200, description = "Execution details", body = inline(ApiResponse<ExecutionResponse>)),
        (status = 404, description = "Execution not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_execution(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i64>,
    Query(query): Query<ExecutionDetailQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let execution = ExecutionRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Execution with ID {} not found", id)))?;

    // Load identity attributes + effective grants once, and memoize the
    // (potentially recursive) visibility-anchor and ancestor-chain lookups,
    // so that the Read and conditional Decrypt authorization checks below
    // share all of them instead of each independently reloading
    // identity/grants and recomputing the anchor/ancestor chain.
    let authz_snapshot = state.authorization_service().load_snapshot(&user).await?;
    let mut visibility_cache = ExecutionVisibilityCache::default();

    authorize_execution_access(
        &state,
        &user,
        &execution,
        Action::Read,
        authz_snapshot.as_ref(),
        &mut visibility_cache,
    )
    .await?;

    let reveal_paths = if query.include_secret_values {
        authorize_execution_access(
            &state,
            &user,
            &execution,
            Action::Decrypt,
            authz_snapshot.as_ref(),
            &mut visibility_cache,
        )
        .await?;
        redacted_paths(&execution.config.clone().unwrap_or(serde_json::Value::Null))
    } else {
        Vec::new()
    };

    let mut response = ExecutionResponse::from(execution.clone());
    if query.include_secret_values {
        response.config = reveal_execution_secret_entity(
            &state,
            response.config,
            ENTITY_EXECUTION_CONFIG,
            execution.id,
        )
        .await?;
        response.result = reveal_execution_secret_entity(
            &state,
            response.result,
            ENTITY_EXECUTION_RESULT,
            execution.id,
        )
        .await?;
        emit_execution_secret_disclosure_audit(&state, &user, &execution, reveal_paths);
    }

    let response = ApiResponse::new(response);

    Ok((StatusCode::OK, Json(response)))
}

/// List safe workflow cache iteration status for an execution.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{id}/workflow-cache-iterations",
    tag = "executions",
    params(("id" = i64, Path, description = "Execution ID")),
    responses(
        (status = 200, description = "Workflow cache iteration status", body = inline(ApiResponse<Vec<WorkflowCacheIterationResponse>>)),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Execution is not visible to the caller"),
        (status = 404, description = "Execution not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_workflow_cache_iterations(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let execution = ExecutionRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Execution with ID {id} not found")))?;

    let authz_snapshot = state.authorization_service().load_snapshot(&user).await?;
    authorize_execution_access(
        &state,
        &user,
        &execution,
        Action::Read,
        authz_snapshot.as_ref(),
        &mut ExecutionVisibilityCache::default(),
    )
    .await?;

    let iterations: Vec<WorkflowCacheIterationResponse> =
        WorkflowCacheIterationRepository::list_by_execution(&state.db, id)
            .await?
            .into_iter()
            .map(WorkflowCacheIterationResponse::from)
            .collect();

    Ok((StatusCode::OK, Json(ApiResponse::new(iterations))))
}

/// List executions by status
#[utoipa::path(
    get,
    path = "/api/v1/executions/status/{status}",
    tag = "executions",
    params(
        ("status" = String, Path, description = "Execution status (requested, scheduling, scheduled, running, completed, failed, canceling, cancelled, timeout, abandoned)"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of executions with specified status", body = PaginatedResponse<ExecutionSummary>),
        (status = 400, description = "Invalid status"),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_executions_by_status(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(status_str): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    // Load identity attributes + effective grants once and reuse them for
    // both the collection-access check and the visibility-scoped search
    // below, instead of fetching them separately for each.
    let authz_snapshot = state.authorization_service().load_snapshot(&user).await?;
    authorize_execution_collection_access(&user, authz_snapshot.as_ref(), Action::Read).await?;

    // Parse status from string
    let status = match status_str.to_lowercase().as_str() {
        "requested" => attune_common::models::enums::ExecutionStatus::Requested,
        "scheduling" => attune_common::models::enums::ExecutionStatus::Scheduling,
        "scheduled" => attune_common::models::enums::ExecutionStatus::Scheduled,
        "running" => attune_common::models::enums::ExecutionStatus::Running,
        "completed" => attune_common::models::enums::ExecutionStatus::Completed,
        "failed" => attune_common::models::enums::ExecutionStatus::Failed,
        "canceling" => attune_common::models::enums::ExecutionStatus::Canceling,
        "cancelled" => attune_common::models::enums::ExecutionStatus::Cancelled,
        "timeout" => attune_common::models::enums::ExecutionStatus::Timeout,
        "abandoned" => attune_common::models::enums::ExecutionStatus::Abandoned,
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Invalid execution status: {}",
                status_str
            )))
        }
    };

    let filters = ExecutionSearchFilters {
        status: Some(status),
        include_total: true,
        limit: pagination.limit(),
        offset: pagination.offset(),
        ..Default::default()
    };

    let result = search_authorized_executions(
        &state,
        &user,
        authz_snapshot.as_ref(),
        &filters,
        Action::Read,
    )
    .await?;
    let total = result.total.unwrap_or(0);
    let paginated_executions: Vec<ExecutionSummary> = result
        .rows
        .into_iter()
        .map(ExecutionSummary::from)
        .collect();

    let response = PaginatedResponse::new(paginated_executions, &pagination, total);

    Ok((StatusCode::OK, Json(response)))
}

/// List executions by enforcement ID
#[utoipa::path(
    get,
    path = "/api/v1/enforcements/{enforcement_id}/executions",
    tag = "executions",
    params(
        ("enforcement_id" = i64, Path, description = "Enforcement ID"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "List of executions for enforcement", body = PaginatedResponse<ExecutionSummary>),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_executions_by_enforcement(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(enforcement_id): Path<i64>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<impl IntoResponse> {
    // Load identity attributes + effective grants once and reuse them for
    // both the collection-access check and the visibility-scoped search
    // below, instead of fetching them separately for each.
    let authz_snapshot = state.authorization_service().load_snapshot(&user).await?;
    authorize_execution_collection_access(&user, authz_snapshot.as_ref(), Action::Read).await?;

    let filters = ExecutionSearchFilters {
        enforcement: Some(enforcement_id),
        include_total: true,
        limit: pagination.limit(),
        offset: pagination.offset(),
        ..Default::default()
    };

    let result = search_authorized_executions(
        &state,
        &user,
        authz_snapshot.as_ref(),
        &filters,
        Action::Read,
    )
    .await?;
    let total = result.total.unwrap_or(0);
    let paginated_executions: Vec<ExecutionSummary> = result
        .rows
        .into_iter()
        .map(ExecutionSummary::from)
        .collect();

    let response = PaginatedResponse::new(paginated_executions, &pagination, total);

    Ok((StatusCode::OK, Json(response)))
}

/// Get execution statistics
#[utoipa::path(
    get,
    path = "/api/v1/executions/stats",
    tag = "executions",
    responses(
        (status = 200, description = "Execution statistics", body = inline(Object)),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_execution_stats(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
) -> ApiResult<impl IntoResponse> {
    // Load identity attributes + effective grants once and reuse them for
    // both the collection-access check and the visibility-scoped status
    // count query below, instead of fetching them separately for each.
    let authz_snapshot = state.authorization_service().load_snapshot(&user).await?;
    authorize_execution_collection_access(&user, authz_snapshot.as_ref(), Action::Read).await?;

    let rows =
        load_authorized_execution_status_counts(&state, &user, authz_snapshot.as_ref()).await?;
    let mut completed: i64 = 0;
    let mut failed: i64 = 0;
    let mut running: i64 = 0;
    let mut pending: i64 = 0;
    let mut cancelled: i64 = 0;
    let mut timeout: i64 = 0;
    let mut abandoned: i64 = 0;
    let mut total: i64 = 0;
    for row in rows {
        total += row.count;
        match row.status {
            ExecutionStatus::Completed => completed += row.count,
            ExecutionStatus::Failed => failed += row.count,
            ExecutionStatus::Running => running += row.count,
            ExecutionStatus::Requested
            | ExecutionStatus::Scheduling
            | ExecutionStatus::Scheduled => pending += row.count,
            ExecutionStatus::Cancelled | ExecutionStatus::Canceling => cancelled += row.count,
            ExecutionStatus::Timeout => timeout += row.count,
            ExecutionStatus::Abandoned => abandoned += row.count,
        }
    }
    let stats = serde_json::json!({
        "total": total,
        "completed": completed,
        "failed": failed,
        "running": running,
        "pending": pending,
        "cancelled": cancelled,
        "timeout": timeout,
        "abandoned": abandoned,
    });

    let response = ApiResponse::new(stats);

    Ok((StatusCode::OK, Json(response)))
}

/// Cancel a running execution
///
/// This endpoint requests cancellation of an execution. The execution must be in a
/// cancellable state (requested, scheduling, scheduled, running, or canceling).
/// For running executions, the worker will send SIGINT to the process, then SIGTERM
/// after a 10-second grace period if it hasn't stopped.
///
/// **Workflow cascading**: When a workflow (parent) execution is cancelled, all of
/// its incomplete child task executions are also cancelled. Children that haven't
/// reached a worker yet are set to Cancelled immediately; children that are running
/// receive a cancel MQ message so their worker can gracefully stop the process.
/// The workflow_execution record is also marked as Cancelled to prevent the
/// scheduler from dispatching any further tasks.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{id}/cancel",
    tag = "executions",
    params(
        ("id" = i64, Path, description = "Execution ID")
    ),
    responses(
        (status = 200, description = "Cancellation requested", body = inline(ApiResponse<ExecutionResponse>)),
        (status = 404, description = "Execution not found"),
        (status = 409, description = "Execution is not in a cancellable state"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn cancel_execution(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    // Load the execution
    let execution = ExecutionRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Execution with ID {} not found", id)))?;

    // Check if the execution is in a cancellable state
    let cancellable = matches!(
        execution.status,
        ExecutionStatus::Requested
            | ExecutionStatus::Scheduling
            | ExecutionStatus::Scheduled
            | ExecutionStatus::Running
            | ExecutionStatus::Canceling
    );

    if !cancellable {
        return Err(ApiError::Conflict(format!(
            "Execution {} is in status '{}' and cannot be cancelled",
            id,
            format!("{:?}", execution.status).to_lowercase()
        )));
    }

    // If already canceling, just return the current state
    if execution.status == ExecutionStatus::Canceling {
        let response = ApiResponse::new(ExecutionResponse::from(execution));
        return Ok((StatusCode::OK, Json(response)));
    }

    let publisher = state.get_publisher().await;

    // For executions that haven't reached a worker yet, cancel immediately
    if matches!(
        execution.status,
        ExecutionStatus::Requested | ExecutionStatus::Scheduling | ExecutionStatus::Scheduled
    ) {
        let update = UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            result: Some(
                serde_json::json!({"error": "Cancelled by user before execution started"}),
            ),
            ..Default::default()
        };
        let updated = ExecutionRepository::update(&state.db, id, update).await?;
        let delegated_to_executor = publish_status_change_to_executor(
            publisher.as_deref(),
            &execution,
            ExecutionStatus::Cancelled,
            "api-service",
        )
        .await;

        if !delegated_to_executor {
            cancel_workflow_children(&state.db, publisher.as_deref(), id).await;
        }

        let response = ApiResponse::new(ExecutionResponse::from(updated));
        return Ok((StatusCode::OK, Json(response)));
    }

    // For running executions, set status to Canceling and send cancel message to the worker
    let update = UpdateExecutionInput {
        status: Some(ExecutionStatus::Canceling),
        ..Default::default()
    };
    let updated = ExecutionRepository::update(&state.db, id, update).await?;
    let delegated_to_executor = publish_status_change_to_executor(
        publisher.as_deref(),
        &execution,
        ExecutionStatus::Canceling,
        "api-service",
    )
    .await;

    // Send cancel request to the worker via MQ
    if let Some(worker_id) = execution.worker {
        send_cancel_to_worker(publisher.as_deref(), id, worker_id).await;
    } else {
        tracing::warn!(
            "Execution {} has no worker assigned; marked as canceling but no MQ message sent",
            id
        );
    }

    if !delegated_to_executor {
        cancel_workflow_children(&state.db, publisher.as_deref(), id).await;
    }

    let response = ApiResponse::new(ExecutionResponse::from(updated));
    Ok((StatusCode::OK, Json(response)))
}

/// Republish a Requested execution's scheduler message.
///
/// This is a recovery control for executions that are still `requested` after
/// their original `ExecutionRequested` message may have been consumed during a
/// transient scheduler failure. It does not restart running work.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{id}/reschedule",
    tag = "executions",
    params(
        ("id" = i64, Path, description = "Execution ID")
    ),
    responses(
        (status = 200, description = "Execution request republished", body = inline(ApiResponse<ExecutionRescheduleResponse>)),
        (status = 404, description = "Execution not found"),
        (status = 409, description = "Execution is not eligible for reschedule"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn reschedule_execution(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let execution = ExecutionRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Execution with ID {} not found", id)))?;

    authorize_execution_access(
        &state,
        &user,
        &execution,
        Action::Cancel,
        None,
        &mut ExecutionVisibilityCache::default(),
    )
    .await?;

    if execution.status != ExecutionStatus::Requested {
        return Err(ApiError::Conflict(format!(
            "Execution {} is in status '{}' and cannot be rescheduled; only requested executions are eligible",
            id,
            format!("{:?}", execution.status).to_lowercase()
        )));
    }

    let Some(publisher) = state.get_publisher().await else {
        return Err(ApiError::InternalServerError(
            "Message queue publisher is unavailable; execution was not republished".to_string(),
        ));
    };

    let attempt = MaintenanceRepository::mark_execution_reschedule_attempt(
        &state.db,
        id,
        "api-service",
        "manual execution reschedule requested",
        state
            .config
            .maintenance
            .execution_reschedule_max_attempts,
        state
            .config
            .maintenance
            .execution_reschedule_grace_seconds,
        true,
    )
    .await?
    .ok_or_else(|| {
        ApiError::Conflict(format!(
            "Execution {} is not eligible for reschedule because it is no longer requested, is admission-queued, or has reached the reschedule attempt limit",
            id
        ))
    })?;

    let payload = ExecutionRequestedPayload {
        execution_id: attempt.execution_id,
        action_id: attempt.action_id,
        action_ref: attempt.action_ref.clone(),
        parent_id: attempt.parent_id,
        enforcement_id: attempt.enforcement_id,
        config: attempt.config.clone(),
    };
    let message = MessageEnvelope::new(MessageType::ExecutionRequested, payload)
        .with_source("api-service")
        .with_correlation_id(uuid::Uuid::new_v4());

    publisher
        .publish_envelope(&message)
        .await
        .map_err(|e| ApiError::InternalServerError(format!("Failed to publish message: {}", e)))?;

    emit_execution_reschedule_audit(&state, &user, &execution, &attempt);

    let current = ExecutionRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Execution with ID {} not found", id)))?;
    let response = ApiResponse::new(ExecutionRescheduleResponse {
        message: "Execution request republished; pending scheduling".to_string(),
        attempt_count: attempt.attempt_count,
        last_attempt_at: attempt.last_attempt_at,
        execution: ExecutionResponse::from(current),
    });
    Ok((StatusCode::OK, Json(response)))
}

fn emit_execution_reschedule_audit(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    execution: &attune_common::models::Execution,
    attempt: &attune_common::repositories::maintenance::ExecutionRescheduleAttempt,
) {
    use attune_common::audit::{AuditCategory, AuditEventBuilder, AuditOutcome};

    let mut builder = AuditEventBuilder::new(
        AuditCategory::Execution,
        "execution.reschedule_requested",
        AuditOutcome::Success,
    )
    .resource("executions")
    .resource_id(execution.id)
    .resource_ref(execution.action_ref.clone())
    .actor_login(user.login().to_string())
    .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase())
    .with_details(serde_json::json!({
        "execution_id": execution.id,
        "action_ref": execution.action_ref,
        "attempt_count": attempt.attempt_count,
        "last_attempt_at": attempt.last_attempt_at,
        "source": attempt.last_source,
        "reason": attempt.last_reason,
    }));
    if let Ok(id) = user.identity_id() {
        builder = builder.actor_identity(id);
    }
    state.audit_emitter.emit(builder.build());
}

/// Send a cancel MQ message to a specific worker for a specific execution.
async fn send_cancel_to_worker(publisher: Option<&Publisher>, execution_id: i64, worker_id: i64) {
    let payload = ExecutionCancelRequestedPayload {
        execution_id,
        worker_id,
    };

    let envelope = MessageEnvelope::new(MessageType::ExecutionCancelRequested, payload)
        .with_source("api-service")
        .with_correlation_id(uuid::Uuid::new_v4());

    if let Some(publisher) = publisher {
        let routing_key = format!("execution.cancel.worker.{}", worker_id);
        let exchange = "attune.executions";
        if let Err(e) = publisher
            .publish_envelope_with_routing(&envelope, exchange, &routing_key)
            .await
        {
            tracing::error!(
                "Failed to publish cancel request for execution {}: {}",
                execution_id,
                e
            );
        }
    } else {
        tracing::warn!(
            "No MQ publisher available to send cancel request for execution {}",
            execution_id
        );
    }
}

async fn publish_status_change_to_executor(
    publisher: Option<&Publisher>,
    execution: &attune_common::models::Execution,
    new_status: ExecutionStatus,
    source: &str,
) -> bool {
    let Some(publisher) = publisher else {
        return false;
    };

    let new_status = match new_status {
        ExecutionStatus::Requested => "requested",
        ExecutionStatus::Scheduling => "scheduling",
        ExecutionStatus::Scheduled => "scheduled",
        ExecutionStatus::Running => "running",
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Canceling => "canceling",
        ExecutionStatus::Cancelled => "cancelled",
        ExecutionStatus::Timeout => "timeout",
        ExecutionStatus::Abandoned => "abandoned",
    };

    let payload = attune_common::mq::ExecutionStatusChangedPayload {
        execution_id: execution.id,
        action_ref: execution.action_ref.clone(),
        previous_status: format!("{:?}", execution.status).to_lowercase(),
        new_status: new_status.to_string(),
        changed_at: Utc::now(),
    };

    let envelope = MessageEnvelope::new(MessageType::ExecutionStatusChanged, payload)
        .with_source(source)
        .with_correlation_id(uuid::Uuid::new_v4());

    if let Err(e) = publisher.publish_envelope(&envelope).await {
        tracing::error!(
            "Failed to publish status change for execution {} to executor: {}",
            execution.id,
            e
        );
        return false;
    }

    true
}

/// Resolve the [`CancellationPolicy`] for a workflow parent execution.
///
/// Looks up the `workflow_execution` → `workflow_definition` chain and
/// deserialises the stored definition to extract the policy.  Returns
/// [`CancellationPolicy::AllowFinish`] (the default) when any lookup
/// step fails so that the safest behaviour is used as a fallback.
async fn resolve_cancellation_policy(
    db: &sqlx::PgPool,
    parent_execution_id: i64,
) -> CancellationPolicy {
    let wf_exec =
        match WorkflowExecutionRepository::find_by_execution(db, parent_execution_id).await {
            Ok(Some(wf)) => wf,
            _ => return CancellationPolicy::default(),
        };

    let wf_def = match WorkflowDefinitionRepository::find_by_id(db, wf_exec.workflow_def).await {
        Ok(Some(def)) => def,
        _ => return CancellationPolicy::default(),
    };

    // Deserialise the stored JSON definition to extract the policy field.
    match serde_json::from_value::<WorkflowDefinition>(wf_def.definition) {
        Ok(def) => def.cancellation_policy,
        Err(e) => {
            tracing::warn!(
                "Failed to deserialise workflow definition for workflow_def {}: {}. \
                 Falling back to AllowFinish cancellation policy.",
                wf_exec.workflow_def,
                e
            );
            CancellationPolicy::default()
        }
    }
}

/// Cancel all incomplete child executions of a workflow parent execution.
///
/// This handles the workflow cascade: when a workflow execution is cancelled,
/// its child task executions must also be cancelled to prevent further work.
/// Additionally, the `workflow_execution` record is marked Cancelled so the
/// scheduler's `advance_workflow` will short-circuit and not dispatch new tasks.
///
/// Behaviour depends on the workflow's [`CancellationPolicy`]:
///
/// - **`AllowFinish`** (default): Children in pre-running states (Requested,
///   Scheduling, Scheduled) are set to Cancelled immediately.  Running children
///   are left alone and will complete naturally; `advance_workflow` sees the
///   cancelled `workflow_execution` and will not dispatch further tasks.
///
/// - **`CancelRunning`**: Pre-running children are cancelled as above.
///   Running children also receive a cancel MQ message so their worker can
///   gracefully stop the process (SIGINT → SIGTERM → SIGKILL).
async fn cancel_workflow_children(
    db: &sqlx::PgPool,
    publisher: Option<&Publisher>,
    parent_execution_id: i64,
) {
    // Determine the cancellation policy from the workflow definition.
    let policy = resolve_cancellation_policy(db, parent_execution_id).await;

    cancel_workflow_children_with_policy(db, publisher, parent_execution_id, policy).await;
}

/// Inner implementation that carries the resolved [`CancellationPolicy`]
/// through recursive calls so that nested child workflows inherit the
/// top-level policy.
async fn cancel_workflow_children_with_policy(
    db: &sqlx::PgPool,
    publisher: Option<&Publisher>,
    parent_execution_id: i64,
    policy: CancellationPolicy,
) {
    // Find all child executions that are still incomplete
    let children: Vec<attune_common::models::Execution> = match sqlx::query_as::<
        _,
        attune_common::models::Execution,
    >(&format!(
        "SELECT {} FROM execution WHERE parent = $1 AND status NOT IN ('completed', 'failed', 'timeout', 'cancelled', 'abandoned')",
        attune_common::repositories::execution::SELECT_COLUMNS
    ))
    .bind(parent_execution_id)
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(
                "Failed to fetch child executions for parent {}: {}",
                parent_execution_id,
                e
            );
            return;
        }
    };

    if children.is_empty() {
        return;
    }

    tracing::info!(
        "Cascading cancellation from execution {} to {} child execution(s) (policy: {:?})",
        parent_execution_id,
        children.len(),
        policy,
    );

    for child in &children {
        let child_id = child.id;

        if matches!(
            child.status,
            ExecutionStatus::Requested | ExecutionStatus::Scheduling | ExecutionStatus::Scheduled
        ) {
            // Pre-running: cancel immediately in DB (both policies)
            let update = UpdateExecutionInput {
                status: Some(ExecutionStatus::Cancelled),
                result: Some(serde_json::json!({
                    "error": "Cancelled: parent workflow execution was cancelled"
                })),
                ..Default::default()
            };
            if let Err(e) = ExecutionRepository::update(db, child_id, update).await {
                tracing::error!("Failed to cancel child execution {}: {}", child_id, e);
            } else {
                tracing::info!("Cancelled pre-running child execution {}", child_id);
            }
        } else if matches!(
            child.status,
            ExecutionStatus::Running | ExecutionStatus::Canceling
        ) {
            match policy {
                CancellationPolicy::CancelRunning => {
                    // Running: set to Canceling and send MQ message to the worker
                    if child.status != ExecutionStatus::Canceling {
                        let update = UpdateExecutionInput {
                            status: Some(ExecutionStatus::Canceling),
                            ..Default::default()
                        };
                        if let Err(e) = ExecutionRepository::update(db, child_id, update).await {
                            tracing::error!(
                                "Failed to set child execution {} to canceling: {}",
                                child_id,
                                e
                            );
                        }
                    }

                    if let Some(worker_id) = child.worker {
                        send_cancel_to_worker(publisher, child_id, worker_id).await;
                    }
                }
                CancellationPolicy::AllowFinish => {
                    // Running tasks are allowed to complete naturally.
                    // advance_workflow will see the cancelled workflow_execution
                    // and will not dispatch any further tasks.
                    tracing::info!(
                        "AllowFinish policy: leaving running child execution {} alone",
                        child_id
                    );
                }
            }
        }

        // Recursively cancel grandchildren (nested workflows)
        // Use Box::pin to allow the recursive async call
        Box::pin(cancel_workflow_children_with_policy(
            db, publisher, child_id, policy,
        ))
        .await;
    }

    // Also mark any associated workflow_execution record as Cancelled so that
    // advance_workflow short-circuits and does not dispatch new tasks.
    // A workflow_execution is linked to the parent execution via its `execution` column.
    if let Ok(Some(wf_exec)) =
        WorkflowExecutionRepository::find_by_execution(db, parent_execution_id).await
    {
        if !matches!(
            wf_exec.status,
            ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
        ) {
            let wf_update = attune_common::repositories::workflow::UpdateWorkflowExecutionInput {
                status: Some(ExecutionStatus::Cancelled),
                error_message: Some(
                    "Cancelled: parent workflow execution was cancelled".to_string(),
                ),
                current_tasks: Some(vec![]),
                completed_tasks: None,
                failed_tasks: None,
                skipped_tasks: None,
                variables: None,
                paused: None,
                pause_reason: None,
            };
            if let Err(e) = WorkflowExecutionRepository::update(db, wf_exec.id, wf_update).await {
                tracing::error!("Failed to cancel workflow_execution {}: {}", wf_exec.id, e);
            } else {
                tracing::info!(
                    "Cancelled workflow_execution {} for parent execution {}",
                    wf_exec.id,
                    parent_execution_id
                );
            }
        }
    }

    // If no children are still running (all were pre-running or were
    // cancelled), finalize the parent execution as Cancelled immediately.
    // Without this, the parent would stay stuck in "Canceling" because no
    // task completion would trigger advance_workflow to finalize it.
    let still_running: Vec<attune_common::models::Execution> = match sqlx::query_as::<
        _,
        attune_common::models::Execution,
    >(&format!(
        "SELECT {} FROM execution WHERE parent = $1 AND status IN ('running', 'canceling', 'scheduling', 'scheduled', 'requested')",
        attune_common::repositories::execution::SELECT_COLUMNS
    ))
    .bind(parent_execution_id)
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(
                "Failed to check remaining children for parent {}: {}",
                parent_execution_id,
                e
            );
            return;
        }
    };

    if still_running.is_empty() {
        // No children left in flight — finalize the parent execution now.
        let update = UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            result: Some(serde_json::json!({
                "error": "Workflow cancelled",
                "succeeded": false,
            })),
            ..Default::default()
        };
        if let Err(e) = ExecutionRepository::update(db, parent_execution_id, update).await {
            tracing::error!(
                "Failed to finalize parent execution {} as Cancelled: {}",
                parent_execution_id,
                e
            );
        } else {
            tracing::info!(
                "Finalized parent execution {} as Cancelled (no running children remain)",
                parent_execution_id
            );
        }
    }
}

/// Create execution routes
/// Stream execution updates via Server-Sent Events
///
/// This endpoint streams real-time updates for execution status changes.
/// Optionally filter by execution_id to watch a specific execution.
///
#[utoipa::path(
    get,
    path = "/api/v1/executions/stream",
    tag = "executions",
    params(
        ("execution_id" = Option<i64>, Query, description = "Optional execution ID to filter updates")
    ),
    responses(
        (status = 200, description = "SSE stream of execution updates", content_type = "text/event-stream"),
        (status = 401, description = "Unauthorized"),
    )
)]
pub async fn stream_execution_updates(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<StreamExecutionParams>,
    user: Result<RequireAuth, crate::auth::middleware::AuthError>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError> {
    let authenticated_user = authenticate_execution_stream_user(&state, &headers, user)?;
    validate_execution_updates_stream_user(&authenticated_user, params.execution_id)?;
    let rx = state.broadcast_tx.subscribe();
    let stream = BroadcastStream::new(rx);

    let filtered_stream = stream.filter_map(move |msg| {
        async move {
            match msg {
                Ok(notification) => {
                    // Parse the notification as JSON
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&notification) {
                        // Check if it's an execution update
                        if let Some(entity_type) = value.get("entity_type").and_then(|v| v.as_str())
                        {
                            if entity_type == "execution" {
                                // If filtering by execution_id, check if it matches
                                if let Some(filter_id) = params.execution_id {
                                    if let Some(entity_id) =
                                        value.get("entity_id").and_then(|v| v.as_i64())
                                    {
                                        if entity_id != filter_id {
                                            return None; // Skip this event
                                        }
                                    }
                                }

                                // Send the notification as an SSE event
                                return Some(Ok(Event::default().data(notification)));
                            }
                        }
                    }
                    None
                }
                Err(_) => None, // Skip broadcast errors
            }
        }
    });

    Ok(Sse::new(filtered_stream).keep_alive(KeepAlive::default()))
}

#[derive(serde::Deserialize)]
pub struct StreamExecutionLogParams {
    pub offset: Option<u64>,
}

#[derive(Clone, Copy)]
enum ExecutionLogStream {
    Stdout,
    Stderr,
}

impl ExecutionLogStream {
    fn parse(name: &str) -> Result<Self, ApiError> {
        match name {
            "stdout" => Ok(Self::Stdout),
            "stderr" => Ok(Self::Stderr),
            _ => Err(ApiError::BadRequest(format!(
                "Unsupported log stream '{}'. Expected 'stdout' or 'stderr'.",
                name
            ))),
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout.log",
            Self::Stderr => "stderr.log",
        }
    }

    fn artifact_ref(self, execution_id: i64) -> String {
        match self {
            Self::Stdout => format!("execution.{}.stdout", execution_id),
            Self::Stderr => format!("execution.{}.stderr", execution_id),
        }
    }
}

enum ExecutionLogTailState {
    WaitingForFile {
        full_path: std::path::PathBuf,
        execution_id: i64,
    },
    SendInitial {
        full_path: std::path::PathBuf,
        execution_id: i64,
        offset: u64,
        pending_utf8: Vec<u8>,
    },
    Tail {
        full_path: std::path::PathBuf,
        execution_id: i64,
        offset: u64,
        idle_polls: u32,
        pending_utf8: Vec<u8>,
    },
    Finished,
}

/// Stream stdout/stderr for an execution as SSE.
///
/// This tails the worker's live log files directly from the shared artifacts
/// volume. The file may not exist yet when the worker has not emitted any
/// output, so the stream waits briefly for it to appear.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{id}/logs/{stream}/stream",
    tag = "executions",
    params(
        ("id" = i64, Path, description = "Execution ID"),
        ("stream" = String, Path, description = "Log stream name: stdout or stderr"),
        ("offset" = Option<u64>, Query, description = "Resume streaming from this byte offset"),
    ),
    responses(
        (status = 200, description = "SSE stream of execution log content", content_type = "text/event-stream"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Execution not found"),
    ),
)]
pub async fn stream_execution_log(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((id, stream_name)): Path<(i64, String)>,
    Query(params): Query<StreamExecutionLogParams>,
    user: Result<RequireAuth, crate::auth::middleware::AuthError>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError> {
    let authenticated_user = authenticate_execution_stream_user(&state, &headers, user)?;
    validate_execution_log_stream_user(&authenticated_user, id)?;

    let execution = ExecutionRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Execution with ID {} not found", id)))?;
    authorize_execution_log_stream(&state, &authenticated_user, &execution).await?;

    let stream_name = ExecutionLogStream::parse(&stream_name)?;
    let full_path = resolve_execution_log_full_path(&state, id, stream_name).await?;
    let db = state.db.clone();

    let initial_state = ExecutionLogTailState::WaitingForFile {
        full_path,
        execution_id: id,
    };
    let start_offset = params.offset.unwrap_or(0);

    let stream = futures::stream::unfold(initial_state, move |state| {
        let db = db.clone();
        async move {
            match state {
                ExecutionLogTailState::Finished => None,
                ExecutionLogTailState::WaitingForFile {
                    full_path,
                    execution_id,
                } => {
                    if full_path.exists() {
                        Some((
                            Ok(Event::default().event("waiting").data("Log file found")),
                            ExecutionLogTailState::SendInitial {
                                full_path,
                                execution_id,
                                offset: start_offset,
                                pending_utf8: Vec::new(),
                            },
                        ))
                    } else if execution_log_execution_terminal(&db, execution_id).await {
                        Some((
                            Ok(Event::default().event("done").data("")),
                            ExecutionLogTailState::Finished,
                        ))
                    } else {
                        tokio::time::sleep(LOG_STREAM_POLL_INTERVAL).await;
                        Some((
                            Ok(Event::default()
                                .event("waiting")
                                .data("Waiting for log output")),
                            ExecutionLogTailState::WaitingForFile {
                                full_path,
                                execution_id,
                            },
                        ))
                    }
                }
                ExecutionLogTailState::SendInitial {
                    full_path,
                    execution_id,
                    offset,
                    pending_utf8,
                } => {
                    let pending_utf8_on_empty = pending_utf8.clone();
                    match read_log_chunk(
                        &full_path,
                        offset,
                        LOG_STREAM_READ_CHUNK_SIZE,
                        pending_utf8,
                    )
                    .await
                    {
                        Some((content, new_offset, pending_utf8)) => Some((
                            Ok(Event::default()
                                .id(new_offset.to_string())
                                .event("content")
                                .data(content)),
                            ExecutionLogTailState::SendInitial {
                                full_path,
                                execution_id,
                                offset: new_offset,
                                pending_utf8,
                            },
                        )),
                        None => Some((
                            Ok(Event::default().comment("initial-catchup-complete")),
                            ExecutionLogTailState::Tail {
                                full_path,
                                execution_id,
                                offset,
                                idle_polls: 0,
                                pending_utf8: pending_utf8_on_empty,
                            },
                        )),
                    }
                }
                ExecutionLogTailState::Tail {
                    full_path,
                    execution_id,
                    offset,
                    idle_polls,
                    pending_utf8,
                } => {
                    let pending_utf8_on_empty = pending_utf8.clone();
                    match read_log_chunk(
                        &full_path,
                        offset,
                        LOG_STREAM_READ_CHUNK_SIZE,
                        pending_utf8,
                    )
                    .await
                    {
                        Some((append, new_offset, pending_utf8)) => Some((
                            Ok(Event::default()
                                .id(new_offset.to_string())
                                .event("append")
                                .data(append)),
                            ExecutionLogTailState::Tail {
                                full_path,
                                execution_id,
                                offset: new_offset,
                                idle_polls: 0,
                                pending_utf8,
                            },
                        )),
                        None => {
                            let terminal =
                                execution_log_execution_terminal(&db, execution_id).await;
                            if terminal && idle_polls >= 2 {
                                Some((
                                    Ok(Event::default().event("done").data("Execution complete")),
                                    ExecutionLogTailState::Finished,
                                ))
                            } else {
                                tokio::time::sleep(LOG_STREAM_POLL_INTERVAL).await;
                                Some((
                                    Ok(Event::default()
                                        .event("waiting")
                                        .data("Waiting for log output")),
                                    ExecutionLogTailState::Tail {
                                        full_path,
                                        execution_id,
                                        offset,
                                        idle_polls: idle_polls + 1,
                                        pending_utf8: pending_utf8_on_empty,
                                    },
                                ))
                            }
                        }
                    }
                }
            }
        }
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn resolve_execution_log_full_path(
    state: &Arc<AppState>,
    execution_id: i64,
    stream_name: ExecutionLogStream,
) -> Result<std::path::PathBuf, ApiError> {
    let artifact_ref = stream_name.artifact_ref(execution_id);

    if let Some(artifact) = ArtifactRepository::find_by_ref(&state.db, &artifact_ref).await? {
        if let Some(version) =
            ArtifactVersionRepository::find_latest(&state.db, artifact.id).await?
        {
            if let Some(file_path) = version.file_path {
                return Ok(std::path::PathBuf::from(&state.config.artifacts_dir).join(file_path));
            }
        }
    }

    Ok(std::path::PathBuf::from(&state.config.artifacts_dir)
        .join(format!("execution_{}", execution_id))
        .join(stream_name.file_name()))
}

async fn read_log_chunk(
    path: &std::path::Path,
    offset: u64,
    max_bytes: usize,
    mut pending_utf8: Vec<u8>,
) -> Option<(String, u64, Vec<u8>)> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let mut file = tokio::fs::File::open(path).await.ok()?;
    let metadata = file.metadata().await.ok()?;
    if metadata.len() <= offset {
        return None;
    }

    file.seek(std::io::SeekFrom::Start(offset)).await.ok()?;
    let bytes_to_read = ((metadata.len() - offset) as usize).min(max_bytes);
    let mut buf = vec![0u8; bytes_to_read];
    let read = file.read(&mut buf).await.ok()?;
    buf.truncate(read);
    if buf.is_empty() {
        return None;
    }

    pending_utf8.extend_from_slice(&buf);
    let (content, pending_utf8) = decode_utf8_chunk(pending_utf8);

    Some((content, offset + read as u64, pending_utf8))
}

async fn execution_log_execution_terminal(db: &sqlx::PgPool, execution_id: i64) -> bool {
    match ExecutionRepository::find_by_id(db, execution_id).await {
        Ok(Some(execution)) => matches!(
            execution.status,
            ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Cancelled
                | ExecutionStatus::Timeout
                | ExecutionStatus::Abandoned
        ),
        _ => true,
    }
}

fn decode_utf8_chunk(mut bytes: Vec<u8>) -> (String, Vec<u8>) {
    match std::str::from_utf8(&bytes) {
        Ok(valid) => (valid.to_string(), Vec::new()),
        Err(err) if err.error_len().is_none() => {
            let pending = bytes.split_off(err.valid_up_to());
            (String::from_utf8_lossy(&bytes).into_owned(), pending)
        }
        Err(_) => (String::from_utf8_lossy(&bytes).into_owned(), Vec::new()),
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ExecutionVisibilityAnchorRow {
    id: i64,
    action_ref: String,
}

/// Memoizes the (potentially recursive) lookups used by
/// [`authorize_execution_access`] so that repeated Read/Decrypt checks
/// against the same execution within one request only hit the database
/// once for each of the tree anchor and the ancestor-executor chain.
#[derive(Debug, Clone, Default)]
struct ExecutionVisibilityCache {
    anchor: Option<ExecutionVisibilityAnchorRow>,
    ancestor_ids: Option<Vec<i64>>,
}

async fn execution_visibility_anchor(
    db: &sqlx::PgPool,
    execution: &attune_common::models::Execution,
) -> Result<ExecutionVisibilityAnchorRow, ApiError> {
    if execution.parent.is_none() {
        return Ok(ExecutionVisibilityAnchorRow {
            id: execution.id,
            action_ref: execution.action_ref.clone(),
        });
    }

    sqlx::query_as::<_, ExecutionVisibilityAnchorRow>(
        r#"
        WITH RECURSIVE lineage AS (
            SELECT id, parent, action_ref, 0 AS depth
            FROM execution
            WHERE id = $1

            UNION ALL

            SELECT p.id, p.parent, p.action_ref, lineage.depth + 1
            FROM execution p
            INNER JOIN lineage ON lineage.parent = p.id
        )
        SELECT id, action_ref
        FROM lineage
        ORDER BY depth DESC
        LIMIT 1
        "#,
    )
    .bind(execution.id)
    .fetch_one(db)
    .await
    .map_err(Into::into)
}

async fn authorize_execution_log_stream(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    execution: &attune_common::models::Execution,
) -> Result<(), ApiError> {
    if user.claims.token_type != TokenType::Access {
        return Ok(());
    }
    authorize_execution_access(
        state,
        user,
        execution,
        Action::Read,
        None,
        &mut ExecutionVisibilityCache::default(),
    )
    .await
}

/// Authorizes access to an execution for the given action.
///
/// `snapshot`, when provided, is a pre-loaded identity-attributes + effective-grants
/// snapshot (see `authz::AuthorizationSnapshot`) reused from the caller instead of being
/// reloaded from the database. `anchor_cache` memoizes the (potentially recursive)
/// visibility-anchor lookup so that repeated Read/Decrypt checks against the same
/// execution within one request only hit the database once.
async fn authorize_execution_access(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    execution: &attune_common::models::Execution,
    action: Action,
    snapshot: Option<&AuthorizationSnapshot>,
    visibility_cache: &mut ExecutionVisibilityCache,
) -> Result<(), ApiError> {
    if !matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        return Ok(());
    }
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let mut ctx = AuthorizationContext::new(identity_id);
    let anchor = if matches!(action, Action::Read | Action::Decrypt) {
        if let Some(anchor) = visibility_cache.anchor.as_ref() {
            anchor.clone()
        } else {
            let anchor = execution_visibility_anchor(&state.db, execution).await?;
            visibility_cache.anchor = Some(anchor.clone());
            anchor
        }
    } else {
        ExecutionVisibilityAnchorRow {
            id: execution.id,
            action_ref: execution.action_ref.clone(),
        }
    };

    ctx.target_id = Some(anchor.id);
    ctx.target_ref = Some(anchor.action_ref.clone());
    ctx.pack_ref = anchor
        .action_ref
        .split_once('.')
        .map(|(pack, _)| pack.to_string());

    // `owner`/`execution_scope` constraints ("self"/"descendants") are
    // evaluated against *this* execution's own executor and its full
    // ancestor chain, not the visibility anchor above. The anchor roots
    // pack_refs/refs/ids-based visibility at the top-level workflow action
    // (matching the bulk list endpoint), but "self"/"descendants" scoping
    // must stay keyed on who actually executed this execution (or one of
    // its ancestors), matching `history.rs`'s reference semantics. Using the
    // anchor's executor here would silently widen a "self"-scoped grant into
    // "descendants" for every workflow the identity happens to own.
    ctx.owner_identity_id = execution.executor;
    ctx.execution_owner_identity_id = execution.executor;
    ctx.execution_ancestor_identity_ids = if let Some(ids) = visibility_cache.ancestor_ids.as_ref()
    {
        ids.clone()
    } else {
        let ids = execution_ancestor_identity_ids(&state.db, execution.parent).await?;
        visibility_cache.ancestor_ids = Some(ids.clone());
        ids
    };

    let authz = state.authorization_service();
    let check = AuthorizationCheck {
        resource: Resource::Executions,
        action,
        context: ctx,
    };
    let authz_result = match snapshot {
        Some(snapshot) => authz.authorize_with_snapshot(user, Some(snapshot), check),
        None => authz.authorize(user, check).await,
    };

    if authz_result.is_ok() {
        return Ok(());
    }

    if matches!(action, Action::Read)
        && user.claims.token_type == TokenType::Access
        && execution_anchor_is_public_action(&state.db, &anchor.action_ref).await?
    {
        return Ok(());
    }

    authz_result
}

async fn execution_anchor_is_public_action(
    db: &sqlx::PgPool,
    action_ref: &str,
) -> Result<bool, ApiError> {
    let Some(action) = ActionRepository::find_by_ref(db, action_ref).await? else {
        return Ok(false);
    };
    Ok(action.reference_visibility == ActionReferenceVisibility::Public)
}

async fn execution_ancestor_identity_ids(
    db: &sqlx::PgPool,
    mut parent_id: Option<i64>,
) -> Result<Vec<i64>, ApiError> {
    let mut identities = Vec::new();
    let mut guard = 0;
    while let Some(id) = parent_id {
        guard += 1;
        if guard > 64 {
            break;
        }
        let Some(parent) = ExecutionRepository::find_by_id(db, id).await? else {
            break;
        };
        if let Some(executor) = parent.executor {
            identities.push(executor);
        }
        parent_id = parent.parent;
    }
    identities.sort_unstable();
    identities.dedup();
    Ok(identities)
}

async fn reveal_execution_secret_entity(
    state: &Arc<AppState>,
    redacted: Option<serde_json::Value>,
    entity_type: &str,
    entity_id: i64,
) -> Result<Option<serde_json::Value>, ApiError> {
    let Some(redacted) = redacted else {
        return Ok(None);
    };
    let secrets =
        ExecutionSecretValueRepository::find_stored_by_entity(&state.db, entity_type, entity_id)
            .await?;
    if secrets.is_empty() {
        return Ok(Some(redacted));
    }
    let encryption_key = state
        .config
        .security
        .encryption_key
        .as_ref()
        .ok_or_else(|| {
            ApiError::InternalServerError(
                "Cannot reveal secret execution values without security.encryption_key".to_string(),
            )
        })?;
    restore_secret_values(redacted, &secrets, encryption_key)
        .map(Some)
        .map_err(|e| ApiError::InternalServerError(format!("Failed to decrypt secret values: {e}")))
}

fn emit_execution_secret_disclosure_audit(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    execution: &attune_common::models::Execution,
    paths: Vec<String>,
) {
    use attune_common::audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome};
    let mut builder = AuditEventBuilder::new(
        AuditCategory::Secret,
        event_type::secret::EXECUTION_VALUES_DECRYPTED,
        AuditOutcome::Success,
    )
    .resource("executions")
    .resource_id(execution.id)
    .resource_ref(execution.action_ref.clone())
    .actor_login(user.login().to_string())
    .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase())
    .with_details(serde_json::json!({
        "execution_id": execution.id,
        "action_ref": execution.action_ref,
        "paths": paths,
    }));
    if let Ok(id) = user.identity_id() {
        builder = builder.actor_identity(id);
    }
    state.audit_emitter.emit(builder.build());
}

fn authenticate_execution_stream_user(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    user: Result<RequireAuth, crate::auth::middleware::AuthError>,
) -> Result<AuthenticatedUser, ApiError> {
    match user {
        Ok(RequireAuth(user)) => Ok(user),
        Err(_) => {
            if let Some(user) = crate::auth::oidc::cookie_authenticated_user(headers, state)? {
                return Ok(user);
            }

            Err(ApiError::Unauthorized(
                "Missing authentication token".to_string(),
            ))
        }
    }
}

fn validate_execution_log_stream_user(
    user: &AuthenticatedUser,
    execution_id: i64,
) -> Result<(), ApiError> {
    let claims = &user.claims;

    match claims.token_type {
        TokenType::Access => Ok(()),
        TokenType::Execution => validate_execution_token_scope(claims, execution_id),
        TokenType::Sensor | TokenType::Refresh | TokenType::Worker => Err(ApiError::Unauthorized(
            "Invalid authentication token".to_string(),
        )),
    }
}

fn validate_execution_token_scope(claims: &Claims, execution_id: i64) -> Result<(), ApiError> {
    if claims.scope.as_deref() != Some("execution") {
        return Err(ApiError::Unauthorized(
            "Invalid authentication token".to_string(),
        ));
    }

    let token_execution_id = claims
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("execution_id"))
        .and_then(|value| value.as_i64())
        .ok_or_else(|| ApiError::Unauthorized("Invalid authentication token".to_string()))?;

    if token_execution_id != execution_id {
        return Err(ApiError::Forbidden(format!(
            "Execution token is not valid for execution {}",
            execution_id
        )));
    }

    Ok(())
}

fn validate_execution_updates_stream_user(
    user: &AuthenticatedUser,
    execution_id: Option<i64>,
) -> Result<(), ApiError> {
    let claims = &user.claims;

    match claims.token_type {
        TokenType::Access => Ok(()),
        TokenType::Execution => {
            let execution_id = execution_id.ok_or_else(|| {
                ApiError::Forbidden(
                    "Execution tokens require an execution_id filter for update streams"
                        .to_string(),
                )
            })?;
            validate_execution_token_scope(claims, execution_id)
        }
        TokenType::Sensor | TokenType::Refresh | TokenType::Worker => Err(ApiError::Unauthorized(
            "Invalid authentication token".to_string(),
        )),
    }
}

#[derive(serde::Deserialize)]
pub struct StreamExecutionParams {
    pub execution_id: Option<i64>,
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/executions", get(list_executions))
        .route("/executions/execute", axum::routing::post(create_execution))
        .route("/executions/stats", get(get_execution_stats))
        .route("/executions/stream", get(stream_execution_updates))
        .route(
            "/executions/{id}/logs/{stream}/stream",
            get(stream_execution_log),
        )
        .route("/executions/{id}", get(get_execution))
        .route(
            "/executions/{id}/workflow-cache-iterations",
            get(list_workflow_cache_iterations),
        )
        .route(
            "/executions/{id}/cancel",
            axum::routing::post(cancel_execution),
        )
        .route(
            "/executions/{id}/reschedule",
            axum::routing::post(reschedule_execution),
        )
        .route(
            "/executions/status/{status}",
            get(list_executions_by_status),
        )
        .route(
            "/enforcements/{enforcement_id}/executions",
            get(list_executions_by_enforcement),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::jwt::{validate_token, JwtConfig};
    use attune_common::auth::jwt::generate_execution_token;
    use attune_common::rbac::{
        Action as RbacAction, ExecutionScopeConstraint, GrantConstraints, Resource as RbacResource,
    };
    use std::collections::HashMap;

    #[test]
    fn test_execution_routes_structure() {
        // Just verify the router can be constructed
        let _router = routes();
    }

    #[test]
    fn grants_include_execution_action_accepts_scoped_grants() {
        let grants = vec![Grant {
            resource: RbacResource::Executions,
            actions: vec![RbacAction::Read],
            constraints: Some(GrantConstraints {
                pack_refs: Some(vec!["python_example".to_string()]),
                ..Default::default()
            }),
        }];

        assert!(grants_include_execution_action(&grants, RbacAction::Read));
        assert!(!grants_include_execution_action(
            &grants,
            RbacAction::Cancel
        ));
    }

    #[test]
    fn collect_execution_visibility_grants_filters_unsupported_constraints() {
        let grants = vec![
            Grant {
                resource: RbacResource::Executions,
                actions: vec![RbacAction::Read],
                constraints: Some(GrantConstraints {
                    pack_refs: Some(vec!["python_example".to_string()]),
                    ..Default::default()
                }),
            },
            Grant {
                resource: RbacResource::Executions,
                actions: vec![RbacAction::Read],
                constraints: Some(GrantConstraints {
                    owner_refs: Some(vec!["forbidden".to_string()]),
                    ..Default::default()
                }),
            },
        ];
        let attrs = HashMap::new();
        let filtered = collect_execution_visibility_grants(&grants, RbacAction::Read, &attrs);
        assert_eq!(filtered.len(), 1);
        assert_eq!(
            filtered[0].pack_refs.as_deref(),
            Some(&["python_example".to_string()][..])
        );
    }

    #[test]
    fn collect_execution_visibility_grants_applies_attribute_constraints() {
        let grants = vec![Grant {
            resource: RbacResource::Executions,
            actions: vec![RbacAction::Read],
            constraints: Some(GrantConstraints {
                execution_scope: Some(ExecutionScopeConstraint::Descendants),
                attributes: Some(HashMap::from([(
                    "team".to_string(),
                    serde_json::json!("platform"),
                )])),
                ..Default::default()
            }),
        }];
        let mut attrs = HashMap::new();
        attrs.insert("team".to_string(), serde_json::json!("platform"));
        assert_eq!(
            collect_execution_visibility_grants(&grants, RbacAction::Read, &attrs).len(),
            1
        );

        attrs.insert("team".to_string(), serde_json::json!("other"));
        assert!(collect_execution_visibility_grants(&grants, RbacAction::Read, &attrs).is_empty());
    }

    #[test]
    fn execution_token_scope_must_match_requested_execution() {
        let jwt_config = JwtConfig {
            secret: "test_secret_key_for_testing".to_string(),
            access_token_expiration: 3600,
            refresh_token_expiration: 604800,
        };

        let token = generate_execution_token(42, 123, "core.echo", &jwt_config, None).unwrap();
        let claims = validate_token(&token, &jwt_config).unwrap();
        let user = AuthenticatedUser { claims };
        let err = validate_execution_log_stream_user(&user, 456).unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)));
    }
}
