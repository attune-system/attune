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

use attune_common::secret_values::{
    prepare_secret_values, redact_secret_path_sources, secret_paths_from_schema, SecretPathSource,
    SecretSource, ENTITY_EVENT_CONFIG, ENTITY_EVENT_PAYLOAD,
};
use attune_common::{
    mq::{EventCreatedPayload, MessageEnvelope, MessageType},
    rbac::{Action as RbacAction, AuthorizationContext, Resource},
    repositories::{
        event::{
            CreateEventInput, EnforcementRepository, EnforcementSearchFilters, EventRepository,
            EventSearchFilters,
        },
        execution::ExecutionRepository,
        execution_secret_value::ExecutionSecretValueRepository,
        trigger::TriggerRepository,
        Create, FindById, FindByRef,
    },
    secret_values::{redacted_paths, restore_secret_values, ENTITY_ENFORCEMENT_CONFIG},
    trace_tag::normalize_trace_tag,
};

use crate::auth::{middleware::AuthenticatedUser, RequireAuth};
use crate::{
    authz::{AuthorizationCheck, AuthorizationService},
    dto::{
        common::{PaginatedResponse, PaginationParams},
        event::{
            EnforcementDetailQueryParams, EnforcementQueryParams, EnforcementResponse,
            EnforcementSummary, EventDetailQueryParams, EventQueryParams, EventResponse,
            EventSummary,
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

    /// Optional source trace tag for this event.
    /// When omitted for execution-token callers, inherits from the parent execution.
    #[schema(example = "core.timer.1234", nullable = true)]
    pub trace_tag: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RedactedEventParts {
    pub payload: Option<JsonValue>,
    pub config: Option<JsonValue>,
    pub payload_secrets: Vec<attune_common::secret_values::SecretValueInput>,
    pub config_secrets: Vec<attune_common::secret_values::SecretValueInput>,
}

/// Redact event payload/config fields designated as secret by the trigger schema.
///
/// By convention, a trigger `param_schema` describes the event payload. For
/// schemas that explicitly contain top-level `payload` and/or `config` object
/// definitions, those nested definitions are applied to the corresponding event
/// section.
pub(crate) fn redact_event_parts_for_trigger(
    trigger_ref: &str,
    schema: Option<&JsonValue>,
    payload: Option<JsonValue>,
    config: Option<JsonValue>,
) -> RedactedEventParts {
    let (payload_schema, config_schema) = event_section_schemas(schema);
    let (payload, payload_secrets) =
        redact_event_section(trigger_ref, "payload", payload, payload_schema.or(schema));
    let (config, config_secrets) =
        redact_event_section(trigger_ref, "config", config, config_schema);

    RedactedEventParts {
        payload,
        config,
        payload_secrets,
        config_secrets,
    }
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

    let explicit_trace_tag = payload
        .trace_tag
        .as_ref()
        .map(|value| normalize_trace_tag(value))
        .transpose()
        .map_err(|e| ApiError::BadRequest(format!("Invalid trace_tag: {e}")))?;
    let inherited_trace_tag = if explicit_trace_tag.is_none() {
        match user.0.execution_id() {
            Some(execution_id) => ExecutionRepository::find_by_id(&state.db, execution_id)
                .await?
                .and_then(|execution| execution.trace_tag),
            None => None,
        }
    } else {
        None
    };
    let source_trace_tag = explicit_trace_tag.or(inherited_trace_tag);

    let redacted = redact_event_parts_for_trigger(
        &trigger.r#ref,
        trigger.param_schema.as_ref(),
        payload.payload,
        payload.config,
    );
    let prepared_payload_secrets =
        prepare_event_secret_values(&state, redacted.payload_secrets).await?;
    let prepared_config_secrets =
        prepare_event_secret_values(&state, redacted.config_secrets).await?;

    // Create event input
    let input = CreateEventInput {
        trigger: Some(trigger.id),
        trigger_ref: payload.trigger_ref.clone(),
        config: redacted.config,
        payload: redacted.payload,
        trace_tag: source_trace_tag,
        source: source_id,
        source_ref,
        rule: rule_id,
        rule_ref,
    };

    // Create the event
    let mut tx = state.db.begin().await?;
    let event = EventRepository::create(&mut *tx, input).await?;
    if !prepared_payload_secrets.is_empty() {
        ExecutionSecretValueRepository::upsert_many_with_conn(
            &mut tx,
            ENTITY_EVENT_PAYLOAD,
            event.id,
            &prepared_payload_secrets,
        )
        .await?;
    }
    if !prepared_config_secrets.is_empty() {
        ExecutionSecretValueRepository::upsert_many_with_conn(
            &mut tx,
            ENTITY_EVENT_CONFIG,
            event.id,
            &prepared_config_secrets,
        )
        .await?;
    }
    tx.commit().await?;

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
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<EventQueryParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_collection_access(&state, &user, Resource::Events, RbacAction::Read).await?;

    // All filtering and pagination happen in a single SQL query.
    let filters = EventSearchFilters {
        trigger: query.trigger,
        trigger_ref: query.trigger_ref.clone(),
        source: query.source,
        rule_ref: query.rule_ref.clone(),
        trace_tag: query.trace_tag.clone(),
        include_total: query.include_total == Some(true),
        limit: query.limit(),
        offset: query.offset(),
    };

    let result = EventRepository::search(&state.db, &filters).await?;

    let event_ids: Vec<i64> = result.rows.iter().map(|event| event.id).collect();
    let event_trace_tags = EventRepository::trace_tags_by_event_ids(&state.db, &event_ids).await?;

    let mut paginated_events: Vec<EventSummary> =
        result.rows.into_iter().map(EventSummary::from).collect();
    for event in &mut paginated_events {
        event.trace_tag = event_trace_tags
            .get(&event.id)
            .cloned()
            .or_else(|| event.trace_tag.clone());
    }

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
        ("id" = i64, Path, description = "Event ID"),
        EventDetailQueryParams
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
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(query): Query<EventDetailQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let event = EventRepository::find_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Event with ID {} not found", id)))?;

    authorize_event_access(&state, &user, &event, RbacAction::Read).await?;

    let reveal_paths = if query.include_secret_values {
        authorize_event_access(&state, &user, &event, RbacAction::Decrypt).await?;
        let mut paths = redacted_paths(&event.payload.clone().unwrap_or(serde_json::Value::Null))
            .into_iter()
            .map(|path| format!("payload{path}"))
            .collect::<Vec<_>>();
        paths.extend(
            redacted_paths(&event.config.clone().unwrap_or(serde_json::Value::Null))
                .into_iter()
                .map(|path| format!("config{path}")),
        );
        paths
    } else {
        Vec::new()
    };

    let mut response = EventResponse::from(event.clone());
    if query.include_secret_values {
        response.payload =
            reveal_event_secret_entity(&state, response.payload, ENTITY_EVENT_PAYLOAD, event.id)
                .await?;
        response.config =
            reveal_event_secret_entity(&state, response.config, ENTITY_EVENT_CONFIG, event.id)
                .await?;
        emit_event_secret_disclosure_audit(&state, &user, &event, reveal_paths);
    }

    let response = ApiResponse::new(response);

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
    RequireAuth(user): RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<EnforcementQueryParams>,
) -> ApiResult<impl IntoResponse> {
    authorize_collection_access(&state, &user, Resource::Enforcements, RbacAction::Read).await?;

    // All filtering and pagination happen in a single SQL query.
    // Filters are combinable (AND), not mutually exclusive.
    let filters = EnforcementSearchFilters {
        status: query.status,
        rule: query.rule,
        event: query.event,
        trigger_ref: query.trigger_ref.clone(),
        rule_ref: query.rule_ref.clone(),
        trace_tag: query.trace_tag.clone(),
        include_total: query.include_total == Some(true),
        limit: query.limit(),
        offset: query.offset(),
    };

    let result = EnforcementRepository::search(&state.db, &filters).await?;

    let enforcement_ids: Vec<i64> = result
        .rows
        .iter()
        .map(|enforcement| enforcement.id)
        .collect();
    let enforcement_trace_tags =
        EnforcementRepository::trace_tags_by_enforcement_ids(&state.db, &enforcement_ids).await?;

    let mut paginated_enforcements: Vec<EnforcementSummary> = result
        .rows
        .into_iter()
        .map(EnforcementSummary::from)
        .collect();
    for enforcement in &mut paginated_enforcements {
        enforcement.trace_tag = enforcement_trace_tags.get(&enforcement.id).cloned();
    }

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

fn event_section_schemas(schema: Option<&JsonValue>) -> (Option<&JsonValue>, Option<&JsonValue>) {
    let Some(schema) = schema else {
        return (None, None);
    };
    let Some(map) = schema.as_object() else {
        return (None, None);
    };

    if map.get("type").and_then(JsonValue::as_str) == Some("object") {
        let properties = map.get("properties").and_then(JsonValue::as_object);
        return (
            properties.and_then(|props| props.get("payload")),
            properties.and_then(|props| props.get("config")),
        );
    }

    (
        map.get("payload")
            .filter(|schema| looks_like_section_schema(schema)),
        map.get("config")
            .filter(|schema| looks_like_section_schema(schema)),
    )
}

fn looks_like_section_schema(schema: &JsonValue) -> bool {
    schema.as_object().is_some_and(|map| {
        map.contains_key("properties")
            || map.get("type").and_then(JsonValue::as_str) == Some("object")
    })
}

fn redact_event_section(
    trigger_ref: &str,
    section: &'static str,
    value: Option<JsonValue>,
    schema: Option<&JsonValue>,
) -> (
    Option<JsonValue>,
    Vec<attune_common::secret_values::SecretValueInput>,
) {
    let Some(value) = value else {
        return (None, Vec::new());
    };

    let path_sources = secret_paths_from_schema(schema)
        .into_iter()
        .map(|path| SecretPathSource {
            source: SecretSource::TriggerSchema {
                trigger_ref: Some(trigger_ref.to_string()),
                section,
                path: path.clone(),
            },
            path,
        })
        .collect::<Vec<_>>();
    let (redacted, secrets) = redact_secret_path_sources(value, &path_sources);
    (Some(redacted), secrets)
}

async fn prepare_event_secret_values(
    state: &Arc<AppState>,
    secrets: Vec<attune_common::secret_values::SecretValueInput>,
) -> Result<Vec<attune_common::secret_values::PreparedSecretValue>, ApiError> {
    if secrets.is_empty() {
        return Ok(Vec::new());
    }
    let encryption_key = state
        .config
        .security
        .encryption_key
        .as_ref()
        .ok_or_else(|| {
            ApiError::InternalServerError(
                "Cannot store secret event values without security.encryption_key".to_string(),
            )
        })?;
    prepare_secret_values(secrets, encryption_key)
        .map_err(|e| ApiError::InternalServerError(format!("Failed to encrypt secret values: {e}")))
}

async fn authorize_collection_access(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    resource: Resource,
    action: RbacAction,
) -> Result<(), ApiError> {
    if !matches!(
        user.claims.token_type,
        crate::auth::jwt::TokenType::Access | crate::auth::jwt::TokenType::Execution
    ) {
        return Ok(());
    }
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    AuthorizationService::new(state.db.clone())
        .authorize(
            user,
            AuthorizationCheck {
                resource,
                action,
                context: AuthorizationContext::new(identity_id),
            },
        )
        .await
}

async fn authorize_event_access(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    event: &attune_common::models::event::Event,
    action: RbacAction,
) -> Result<(), ApiError> {
    if !matches!(
        user.claims.token_type,
        crate::auth::jwt::TokenType::Access | crate::auth::jwt::TokenType::Execution
    ) {
        return Ok(());
    }
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_id = Some(event.id);
    ctx.target_ref = Some(event.trigger_ref.clone());
    ctx.pack_ref = event
        .trigger_ref
        .split_once('.')
        .map(|(pack, _)| pack.to_string());

    AuthorizationService::new(state.db.clone())
        .authorize(
            user,
            AuthorizationCheck {
                resource: Resource::Events,
                action,
                context: ctx,
            },
        )
        .await
}

async fn reveal_event_secret_entity(
    state: &Arc<AppState>,
    redacted: Option<serde_json::Value>,
    entity_type: &str,
    event_id: i64,
) -> Result<Option<serde_json::Value>, ApiError> {
    let Some(redacted) = redacted else {
        return Ok(None);
    };
    let secrets =
        ExecutionSecretValueRepository::find_stored_by_entity(&state.db, entity_type, event_id)
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
                "Cannot reveal secret event values without security.encryption_key".to_string(),
            )
        })?;
    restore_secret_values(redacted, &secrets, encryption_key)
        .map(Some)
        .map_err(|e| ApiError::InternalServerError(format!("Failed to decrypt secret values: {e}")))
}

fn emit_event_secret_disclosure_audit(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    event: &attune_common::models::event::Event,
    paths: Vec<String>,
) {
    use attune_common::audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome};
    let mut builder = AuditEventBuilder::new(
        AuditCategory::Secret,
        event_type::secret::EVENT_VALUES_DECRYPTED,
        AuditOutcome::Success,
    )
    .resource("events")
    .resource_id(event.id)
    .resource_ref(event.trigger_ref.clone())
    .actor_login(user.login().to_string())
    .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase())
    .with_details(serde_json::json!({
        "event_id": event.id,
        "trigger_ref": event.trigger_ref,
        "paths": paths,
    }));
    if let Ok(id) = user.identity_id() {
        builder = builder.actor_identity(id);
    }
    state.audit_emitter.emit(builder.build());
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
    if !matches!(
        user.claims.token_type,
        crate::auth::jwt::TokenType::Access | crate::auth::jwt::TokenType::Execution
    ) {
        return Ok(());
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::secret_values::is_redaction_marker;
    use serde_json::json;

    #[test]
    fn redacts_event_payload_from_flat_trigger_schema() {
        let schema = json!({
            "username": {"type": "string"},
            "password": {"type": "string", "secret": true}
        });

        let redacted = redact_event_parts_for_trigger(
            "demo.login",
            Some(&schema),
            Some(json!({"username": "alice", "password": "s3cr3t"})),
            None,
        );

        let payload = redacted.payload.unwrap();
        assert_eq!(payload["username"], "alice");
        assert!(is_redaction_marker(&payload["password"]));
        assert_eq!(redacted.payload_secrets.len(), 1);
        assert_eq!(redacted.payload_secrets[0].json_path, "/password");
        assert_eq!(redacted.payload_secrets[0].source_kind, "trigger_schema");
    }

    #[test]
    fn redacts_event_payload_and_config_from_sectioned_schema() {
        let schema = json!({
            "payload": {
                "properties": {
                    "api_key": {"type": "string", "secret": true}
                }
            },
            "config": {
                "properties": {
                    "headers": {
                        "properties": {
                            "authorization": {"type": "string", "secret": true}
                        }
                    }
                }
            }
        });

        let redacted = redact_event_parts_for_trigger(
            "demo.webhook",
            Some(&schema),
            Some(json!({"api_key": "payload-secret", "message": "ok"})),
            Some(json!({"headers": {"authorization": "Bearer secret"}})),
        );

        let payload = redacted.payload.unwrap();
        let config = redacted.config.unwrap();
        assert!(is_redaction_marker(&payload["api_key"]));
        assert_eq!(payload["message"], "ok");
        assert!(is_redaction_marker(&config["headers"]["authorization"]));
        assert_eq!(redacted.payload_secrets[0].json_path, "/api_key");
        assert_eq!(
            redacted.config_secrets[0].json_path,
            "/headers/authorization"
        );
    }
}
