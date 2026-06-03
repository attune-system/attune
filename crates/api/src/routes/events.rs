//! Event and Enforcement query API routes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use utoipa::ToSchema;
use validator::Validate;

use attune_common::{
    mq::{EventCreatedPayload, MessageEnvelope, MessageType},
    rbac::{Action as RbacAction, AuthorizationContext, Resource},
    repositories::{
        event::{
            CreateEventInput, EnforcementRepository, EnforcementSearchFilters, EventRepository,
            EventSearchFilters,
        },
        execution_secret_value::ExecutionSecretValueRepository,
        trigger::TriggerRepository,
        Create, FindById, FindByRef,
    },
    secret_values::{redacted_paths, restore_secret_values, ENTITY_ENFORCEMENT_CONFIG},
};

use crate::auth::{middleware::AuthenticatedUser, RequireAuth};
use crate::{
    authz::{AuthorizationCheck, AuthorizationService},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        event::{
            EnforcementDetailQueryParams, EnforcementQueryParams, EnforcementResponse,
            EnforcementSummary, EventQueryParams, EventResponse, EventSummary,
        },
        ApiResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

/// Request body for creating an event
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct CreateEventRequest {
    /// Trigger reference (e.g., "core.timer", "core.webhook")
    /// Also accepts "trigger_type" for compatibility with the sensor interface spec.
    #[validate(length(min = 1))]
    #[serde(alias = "trigger_type")]
    #[schema(example = "core.timer")]
    pub trigger_ref: String,

    /// Event payload data
    #[schema(value_type = Object, example = json!({"timestamp": "2024-01-13T10:30:00Z"}))]
    pub payload: Option<JsonValue>,

    /// Event configuration
    #[schema(value_type = Object)]
    pub config: Option<JsonValue>,

    /// Trigger instance ID (for correlation, often rule_id)
    #[schema(example = "rule_123")]
    pub trigger_instance_id: Option<String>,
}

/// Create a new event
#[utoipa::path(
    post,
    path = "/api/v1/events",
    tag = "events",
    request_body = CreateEventRequest,
    security(("bearer_auth" = [])),
    responses(
        (status = 201, description = "Event created successfully", body = ApiResponse<EventResponse>),
        (status = 400, description = "Validation error"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn create_event(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateEventRequest>,
) -> ApiResult<impl IntoResponse> {
    // Only sensor and execution tokens may create events directly.
    // User sessions must go through the webhook receiver instead.
    use crate::auth::jwt::TokenType;
    if user.0.claims.token_type == TokenType::Access {
        return Err(ApiError::Forbidden(
            "Events may only be created by sensor services. To fire an event as a user, \
             enable webhooks on the trigger and POST to its webhook URL."
                .to_string(),
        ));
    }

    // Validate request
    payload
        .validate()
        .map_err(|e| ApiError::ValidationError(format!("Invalid event request: {}", e)))?;

    // Lookup trigger by reference to get trigger ID
    let trigger = TriggerRepository::find_by_ref(&state.db, &payload.trigger_ref)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Trigger '{}' not found", payload.trigger_ref))
        })?;
    if !trigger.enabled {
        return Err(ApiError::BadRequest(format!(
            "Trigger '{}' is disabled",
            payload.trigger_ref
        )));
    }

    // Parse trigger_instance_id to extract rule ID (format: "rule_{id}")
    let (rule_id, rule_ref) = if let Some(instance_id) = &payload.trigger_instance_id {
        if let Some(id_str) = instance_id.strip_prefix("rule_") {
            if let Ok(rid) = id_str.parse::<i64>() {
                // Fetch rule reference from database
                let fetched_rule_ref: Option<String> =
                    sqlx::query_scalar("SELECT ref FROM rule WHERE id = $1")
                        .bind(rid)
                        .fetch_optional(&state.db)
                        .await?;

                if let Some(rref) = fetched_rule_ref {
                    tracing::debug!("Event associated with rule {} (id: {})", rref, rid);
                    (Some(rid), Some(rref))
                } else {
                    tracing::warn!("trigger_instance_id {} provided but rule not found", rid);
                    (None, None)
                }
            } else {
                tracing::warn!("Invalid rule ID in trigger_instance_id: {}", instance_id);
                (None, None)
            }
        } else {
            tracing::debug!(
                "trigger_instance_id doesn't match rule format: {}",
                instance_id
            );
            (None, None)
        }
    } else {
        (None, None)
    };

    // Determine source (sensor) from authenticated user if it's a sensor token
    let (source_id, source_ref) = match user.0.claims.token_type {
        TokenType::Sensor => {
            // Extract sensor reference from login
            let sensor_ref = user.0.claims.login.clone();

            // Look up sensor by reference
            let sensor_id: Option<i64> = sqlx::query_scalar("SELECT id FROM sensor WHERE ref = $1")
                .bind(&sensor_ref)
                .fetch_optional(&state.db)
                .await?;

            match sensor_id {
                Some(id) => {
                    tracing::debug!("Event created by sensor {} (id: {})", sensor_ref, id);
                    (Some(id), Some(sensor_ref))
                }
                None => {
                    tracing::warn!("Sensor token for ref '{}' but sensor not found", sensor_ref);
                    (None, Some(sensor_ref))
                }
            }
        }
        _ => (None, None),
    };

    // Create event input
    let input = CreateEventInput {
        trigger: Some(trigger.id),
        trigger_ref: payload.trigger_ref.clone(),
        config: payload.config,
        payload: payload.payload,
        source: source_id,
        source_ref,
        rule: rule_id,
        rule_ref,
    };

    // Create the event
    let event = EventRepository::create(&state.db, input).await?;

    // Publish EventCreated message to message queue if publisher is available
    if let Some(publisher) = state.get_publisher().await {
        let message_payload = EventCreatedPayload {
            event_id: event.id,
            trigger_id: event.trigger,
            trigger_ref: event.trigger_ref.clone(),
            sensor_id: event.source,
            sensor_ref: event.source_ref.clone(),
            payload: event.payload.clone().unwrap_or(serde_json::json!({})),
            config: event.config.clone(),
        };

        let envelope = MessageEnvelope::new(MessageType::EventCreated, message_payload)
            .with_source("api-service");

        if let Err(e) = publisher.publish_envelope(&envelope).await {
            tracing::warn!(
                "Failed to publish EventCreated message for event {}: {}",
                event.id,
                e
            );
            // Continue even if message publishing fails - event is already recorded
        } else {
            tracing::debug!(
                "Published EventCreated message for event {} (trigger: {})",
                event.id,
                event.trigger_ref
            );
        }
    }

    let response = ApiResponse::new(EventResponse::from(event));

    Ok((StatusCode::CREATED, Json(response)))
}

/// List all events with pagination and optional filters
#[utoipa::path(
    get,
    path = "/api/v1/events",
    tag = "events",
    params(EventQueryParams),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "List of events", body = PaginatedResponse<EventSummary>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_events(
    _user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<EventQueryParams>,
) -> ApiResult<impl IntoResponse> {
    // All filtering and pagination happen in a single SQL query.
    let filters = EventSearchFilters {
        trigger: query.trigger,
        trigger_ref: query.trigger_ref.clone(),
        source: query.source,
        rule_ref: query.rule_ref.clone(),
        include_total: query.include_total == Some(true),
        limit: query.limit(),
        offset: query.offset(),
    };

    let result = EventRepository::search(&state.db, &filters).await?;

    let paginated_events: Vec<EventSummary> =
        result.rows.into_iter().map(EventSummary::from).collect();

    let pagination_params = PaginationParams {
        page: query.page,
        page_size: query.per_page,
    };

    let response = if let Some(total) = result.total {
        PaginatedResponse::new(paginated_events, &pagination_params, total)
    } else {
        PaginatedResponse::without_totals(paginated_events, &pagination_params, result.has_next)
    };

    Ok((StatusCode::OK, Json(response)))
}

/// Get a single event by ID
#[utoipa::path(
    get,
    path = "/api/v1/events/{id}",
    tag = "events",
    params(
        ("id" = i64, Path, description = "Event ID")
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Event details", body = ApiResponse<EventResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Event not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_event(
    _user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<impl IntoResponse> {
    let event = EventRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Event with ID {} not found", id)))?;

    let response = ApiResponse::new(EventResponse::from(event));

    Ok((StatusCode::OK, Json(response)))
}

/// List all enforcements with pagination and optional filters
#[utoipa::path(
    get,
    path = "/api/v1/enforcements",
    tag = "enforcements",
    params(EnforcementQueryParams),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "List of enforcements", body = PaginatedResponse<EnforcementSummary>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_enforcements(
    _user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<EnforcementQueryParams>,
) -> ApiResult<impl IntoResponse> {
    // All filtering and pagination happen in a single SQL query.
    // Filters are combinable (AND), not mutually exclusive.
    let filters = EnforcementSearchFilters {
        status: query.status,
        rule: query.rule,
        event: query.event,
        trigger_ref: query.trigger_ref.clone(),
        rule_ref: query.rule_ref.clone(),
        include_total: query.include_total == Some(true),
        limit: query.limit(),
        offset: query.offset(),
    };

    let result = EnforcementRepository::search(&state.db, &filters).await?;

    let paginated_enforcements: Vec<EnforcementSummary> = result
        .rows
        .into_iter()
        .map(EnforcementSummary::from)
        .collect();

    let pagination_params = PaginationParams {
        page: query.page,
        page_size: query.per_page,
    };

    let response = if let Some(total) = result.total {
        PaginatedResponse::new(paginated_enforcements, &pagination_params, total)
    } else {
        PaginatedResponse::without_totals(
            paginated_enforcements,
            &pagination_params,
            result.has_next,
        )
    };

    Ok((StatusCode::OK, Json(response)))
}

/// Get a single enforcement by ID
#[utoipa::path(
    get,
    path = "/api/v1/enforcements/{id}",
    tag = "enforcements",
    params(
        ("id" = i64, Path, description = "Enforcement ID")
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Enforcement details", body = ApiResponse<EnforcementResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Enforcement not found"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_enforcement(
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(query): Query<EnforcementDetailQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let enforcement = EnforcementRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Enforcement with ID {} not found", id)))?;

    authorize_enforcement_access(&state, &user, &enforcement, RbacAction::Read).await?;

    let reveal_paths = if query.include_secret_values {
        authorize_enforcement_access(&state, &user, &enforcement, RbacAction::Decrypt).await?;
        redacted_paths(
            &enforcement
                .config
                .clone()
                .unwrap_or(serde_json::Value::Null),
        )
    } else {
        Vec::new()
    };

    let mut response = EnforcementResponse::from(enforcement.clone());
    if query.include_secret_values {
        response.config =
            reveal_enforcement_secret_config(&state, response.config, enforcement.id).await?;
        emit_enforcement_secret_disclosure_audit(&state, &user, &enforcement, reveal_paths);
    }

    let response = ApiResponse::new(response);

    Ok((StatusCode::OK, Json(response)))
}

async fn authorize_enforcement_access(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    enforcement: &attune_common::models::event::Enforcement,
    action: RbacAction,
) -> Result<(), ApiError> {
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_id = Some(enforcement.id);
    ctx.target_ref = Some(enforcement.rule_ref.clone());
    ctx.pack_ref = enforcement
        .rule_ref
        .split_once('.')
        .map(|(pack, _)| pack.to_string());

    AuthorizationService::new(state.db.clone())
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Enforcements,
                action,
                context: ctx,
            },
        )
        .await
}

async fn reveal_enforcement_secret_config(
    state: &Arc<AppState>,
    redacted: Option<serde_json::Value>,
    enforcement_id: i64,
) -> Result<Option<serde_json::Value>, ApiError> {
    let Some(redacted) = redacted else {
        return Ok(None);
    };
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &state.db,
        ENTITY_ENFORCEMENT_CONFIG,
        enforcement_id,
    )
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
                "Cannot reveal secret enforcement values without security.encryption_key"
                    .to_string(),
            )
        })?;
    restore_secret_values(redacted, &secrets, encryption_key)
        .map(Some)
        .map_err(|e| ApiError::InternalServerError(format!("Failed to decrypt secret values: {e}")))
}

fn emit_enforcement_secret_disclosure_audit(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    enforcement: &attune_common::models::event::Enforcement,
    paths: Vec<String>,
) {
    use attune_common::audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome};
    let mut builder = AuditEventBuilder::new(
        AuditCategory::Secret,
        event_type::secret::ENFORCEMENT_VALUES_DECRYPTED,
        AuditOutcome::Success,
    )
    .resource("enforcements")
    .resource_id(enforcement.id)
    .resource_ref(enforcement.rule_ref.clone())
    .actor_login(user.login().to_string())
    .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase())
    .with_details(serde_json::json!({
        "enforcement_id": enforcement.id,
        "rule_ref": enforcement.rule_ref,
        "paths": paths,
    }));
    if let Ok(id) = user.identity_id() {
        builder = builder.actor_identity(id);
    }
    state.audit_emitter.emit(builder.build());
}

/// Register event and enforcement routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/events", get(list_events).post(create_event))
        .route("/events/{id}", get(get_event))
        .route("/enforcements", get(list_enforcements))
        .route("/enforcements/{id}", get(get_enforcement))
}
