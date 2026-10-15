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
    prepare_secret_values, ENTITY_EVENT_CONFIG, ENTITY_EVENT_PAYLOAD,
};
use attune_common::{
    mq::{EventCreatedPayload, MessageEnvelope, MessageType},
    rbac::{Action as RbacAction, Grant, Resource},
    repositories::{
        event::{
            CreateEventInput, EnforcementRepository, EnforcementSearchFilters,
            EnforcementVisibilityFilter, EventRepository, EventSearchFilters,
            EventVisibilityFilter, VisibilityReadScope,
        },
        event_schema::EventSchemaRepository,
        execution::ExecutionRepository,
        execution_secret_value::ExecutionSecretValueRepository,
        rule::RuleRepository,
        sensor_admission::SensorAdmissionRepository,
        sensor_workload::SensorWorkloadRepository,
        trigger::{SensorRepository, TriggerRepository},
        Create, FindById, FindByRef,
    },
    secret_values::{redacted_paths, restore_secret_values},
    trace_tag::normalize_trace_tag,
};

use crate::auth::{jwt::TokenType, middleware::AuthenticatedUser, RequireAuth};
use crate::routes::visibility::{
    action_name, build_visibility_read_scope, has_unconstrained_resource_action,
    is_scoped_identity_token, resource_action_grant_exists, scope_allows_resource_ref,
};
use crate::{
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
#[serde(deny_unknown_fields)]
pub struct CreateEventRequest {
    /// Trigger reference (e.g., "core.timer", "core.webhook")
    /// Also accepts "trigger_type" for compatibility with the sensor interface spec.
    #[validate(length(min = 1))]
    #[serde(alias = "trigger_type")]
    #[schema(example = "core.timer")]
    pub trigger_ref: String,

    /// Event payload data
    #[schema(
        value_type = Object,
        required = false,
        example = json!({"timestamp": "2024-01-13T10:30:00Z"})
    )]
    pub payload: Option<JsonValue>,

    /// Event configuration
    #[schema(value_type = Object, required = false)]
    pub config: Option<JsonValue>,

    /// Trigger instance ID (for correlation, often rule_id)
    #[schema(example = "rule_123")]
    pub trigger_instance_id: Option<String>,

    /// Optional source trace tag for this event.
    /// When omitted for execution-token callers, inherits from the parent execution.
    #[schema(example = "core.timer.1234", nullable = true)]
    pub trace_tag: Option<String>,
}

fn event_token_type_allowed(token_type: &TokenType) -> bool {
    matches!(token_type, TokenType::Sensor | TokenType::Execution)
}

fn parse_rule_instance_id(instance_id: Option<&str>) -> Result<Option<i64>, &'static str> {
    let Some(instance_id) = instance_id else {
        return Ok(None);
    };
    let id = instance_id
        .strip_prefix("rule_")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .ok_or("trigger_instance_id must use the format rule_<positive numeric id>")?;
    Ok(Some(id))
}

fn sensor_trigger_scope_allows(trigger_types: &[String], trigger_ref: &str) -> bool {
    trigger_types.iter().any(|allowed| allowed == trigger_ref)
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
        (status = 403, description = "Token is not authorized to create this event"),
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
    if !event_token_type_allowed(&user.0.claims.token_type) {
        return Err(ApiError::Forbidden(
            "Events may only be created by sensor or execution tokens. To fire an event as a user, \
             enable webhooks on the trigger and POST to its webhook URL."
                .to_string(),
        ));
    }

    // Validate request
    payload
        .validate()
        .map_err(|e| ApiError::ValidationError(format!("Invalid event request: {}", e)))?;

    if user.0.claims.token_type == TokenType::Sensor {
        let allowed_trigger_refs = user.0.sensor_trigger_types();
        if !sensor_trigger_scope_allows(&allowed_trigger_refs, &payload.trigger_ref) {
            return Err(ApiError::Forbidden(format!(
                "Sensor token is not authorized for trigger '{}'",
                payload.trigger_ref
            )));
        }
    }

    // Lookup trigger by reference to get trigger ID
    let trigger = TriggerRepository::find_by_ref(&state.db, &payload.trigger_ref)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Trigger '{}' not found", payload.trigger_ref))
        })?;
    if !trigger.enabled || trigger.retired_at.is_some() {
        return Err(ApiError::BadRequest(format!(
            "Trigger '{}' is disabled",
            payload.trigger_ref
        )));
    }

    let mut tx = state.db.begin().await?;
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(&mut *tx)
        .await?;
    let recorded_authority = user.0.recorded_runtime_authority(&mut tx).await?;
    if let Some(authority) = &recorded_authority {
        let target = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
            &mut tx,
            attune_common::repositories::runtime_target::RuntimeTarget::EventTrigger(trigger.id),
            authority.authority().owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(|| ApiError::Forbidden("Event target is unavailable".into()))?;
        if !authority.allows_new_effect(&target, RbacAction::Create) {
            return Err(ApiError::Forbidden(
                "Selected runtime authority does not permit Events/Create on this trigger".into(),
            ));
        }
    }

    // Resolve and authorize the source before accepting a rule target or event payload.
    let (source_id, source_ref, sensor_fence) = match user.0.claims.token_type {
        TokenType::Sensor => {
            let sensor_ref = user.0.claims.login.clone();
            let sensor = SensorRepository::find_by_ref(&state.db, &sensor_ref)
                .await?
                .ok_or_else(|| {
                    ApiError::Forbidden(format!(
                        "Sensor token references unknown sensor '{}'",
                        sensor_ref
                    ))
                })?;
            if trigger.sensor != Some(sensor.id) {
                return Err(ApiError::Forbidden(format!(
                    "Sensor '{}' does not own trigger '{}'",
                    sensor_ref, trigger.r#ref
                )));
            }

            tracing::debug!(
                "Event created by sensor {} (id: {})",
                sensor.r#ref,
                sensor.id
            );
            let fence = user.0.sensor_workload_fence().map_err(|_| {
                ApiError::Forbidden(
                    "Sensor token is missing its workload assignment fence".to_string(),
                )
            })?;
            (Some(sensor.id), Some(sensor.r#ref), Some(fence))
        }
        TokenType::Execution => (None, None, None),
        _ => unreachable!("event token type checked above"),
    };

    // Parse trigger_instance_id to extract rule ID (format: "rule_{id}").
    // This linkage is required both functionally (the executor uses
    // `event.rule` to scope enforcement matching to a single rule instance
    // when multiple rules share the same trigger, e.g. concurrent timers)
    // and for read-time visibility (see `apply_event_summary_visibility`,
    // which already redacts `rule`/`rule_ref` for readers whose grants don't
    // cover the associated rule). To avoid trusting caller-supplied input, the
    // rule is looked up from the database and only accepted if it actually
    // targets this event's trigger - a sensor cannot claim association with
    // an unrelated rule.
    let parsed_rule_id = parse_rule_instance_id(payload.trigger_instance_id.as_deref())
        .map_err(|message| ApiError::BadRequest(message.to_string()))?;
    let (rule_id, rule_ref) = match parsed_rule_id {
        Some(rule_id) if sensor_fence.is_some() => (Some(rule_id), None),
        Some(rule_id) => {
            let rule = RuleRepository::find_by_id(&state.db, rule_id)
                .await?
                .ok_or_else(|| {
                    ApiError::BadRequest(format!(
                        "trigger_instance_id references unknown rule {}",
                        rule_id
                    ))
                })?;
            if rule.trigger != Some(trigger.id) {
                return Err(ApiError::BadRequest(format!(
                    "Rule '{}' does not use trigger '{}'",
                    rule.r#ref, trigger.r#ref
                )));
            }
            tracing::debug!(
                "Event associated with rule {} (id: {})",
                rule.r#ref,
                rule_id
            );
            (Some(rule_id), Some(rule.r#ref))
        }
        None => (None, None),
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

    if sensor_fence.is_some() {
        SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
    }
    let mut event_input_origins = Vec::new();
    let mut event_origin_bytes = 0usize;
    if let Some(authority) = &recorded_authority {
        let execution_id = authority.credential_binding().execution_id;
        for secret in ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *tx,
            attune_common::secret_values::ENTITY_EXECUTION_CONFIG,
            execution_id,
        )
        .await?
        {
            push_bounded_event_origin(
                &mut event_input_origins,
                &mut event_origin_bytes,
                attune_common::secret_provenance::SecretOrigin::Entity {
                    entity_type: attune_common::secret_values::ENTITY_EXECUTION_CONFIG.into(),
                    entity_id: execution_id,
                    path: secret.json_path,
                },
            )?;
        }
        for origin in
            attune_common::repositories::execution_key_read::ExecutionKeyReadRepository::origins(
                &mut tx,
                authority.credential_binding().admission_id,
            )
            .await?
        {
            push_bounded_event_origin(
                &mut event_input_origins,
                &mut event_origin_bytes,
                attune_common::secret_provenance::SecretOrigin::Key(origin),
            )?;
        }
    }
    let (trigger, binding) = if let Some(fence) = sensor_fence {
        let sensor_id = source_id.expect("sensor fence requires a resolved sensor");
        if !SensorWorkloadRepository::lock_current_fence(&mut tx, sensor_id, fence).await? {
            return Err(ApiError::Forbidden(
                "Sensor workload assignment is stale or expired".into(),
            ));
        }
        let (generation_id, generation, _) =
            attune_common::repositories::sensor_schema::SensorSchemaRepository::find(
                &mut *tx,
                fence.workload_id,
                fence.generation,
            )
            .await?
            .ok_or_else(|| {
                ApiError::Forbidden("Sensor generation binding is unavailable".into())
            })?;
        if generation.executable.sensor.id != sensor_id {
            return Err(ApiError::Forbidden(
                "Sensor generation owner mismatch".into(),
            ));
        }
        for entity_type in [
            attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG,
            attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG,
        ] {
            for secret in ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *tx,
                entity_type,
                generation_id,
            )
            .await?
            {
                let origin = attune_common::secret_provenance::SecretOrigin::Entity {
                    entity_type: entity_type.to_string(),
                    entity_id: generation_id,
                    path: secret.json_path,
                };
                push_bounded_event_origin(
                    &mut event_input_origins,
                    &mut event_origin_bytes,
                    origin,
                )?;
            }
        }
        for (delivery, path) in attune_common::repositories::sensor_schema::SensorSchemaRepository::delivery_origin_paths(&mut *tx, generation_id).await? {
            push_bounded_event_origin(&mut event_input_origins, &mut event_origin_bytes,
                attune_common::secret_provenance::SecretOrigin::Entity {
                    entity_type: attune_common::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG.to_string(),
                    entity_id: delivery, path,
                })?;
        }
        for origin in
            attune_common::repositories::sensor_schema::SensorSchemaRepository::key_read_origins(
                &mut *tx,
                generation_id,
            )
            .await?
        {
            push_bounded_event_origin(
                &mut event_input_origins,
                &mut event_origin_bytes,
                attune_common::secret_provenance::SecretOrigin::Key(origin),
            )?;
        }
        let binding = generation
            .triggers
            .get(&trigger.r#ref)
            .filter(|binding| binding.event.trigger_id == Some(trigger.id))
            .ok_or_else(|| {
                ApiError::Forbidden("Trigger is not admitted for this sensor generation".into())
            })?
            .event
            .clone();
        (trigger, binding)
    } else {
        EventSchemaRepository::materialize_current(&mut tx, trigger.id, false).await?
    };
    let mut redacted = binding.prepare(payload.payload, payload.config)?;
    if !event_input_origins.is_empty() {
        let count = redacted.payload_secrets.len() + redacted.config_secrets.len();
        if event_origin_bytes
            .checked_mul(count)
            .is_none_or(|bytes| bytes > 4 * 1024 * 1024)
        {
            return Err(ApiError::ValidationError(
                "Event input provenance exceeds its limit".into(),
            ));
        }
        for secret in redacted
            .payload_secrets
            .iter_mut()
            .chain(&mut redacted.config_secrets)
        {
            let mut provenance = attune_common::secret_provenance::SecretProvenance {
                origins: event_input_origins.clone(),
                templates: Vec::new(),
            };
            if secret.source_kind == "provenance" {
                let previous = secret
                    .source_ref
                    .as_deref()
                    .and_then(|reference| {
                        serde_json::from_str::<attune_common::secret_provenance::SecretProvenance>(
                            reference,
                        )
                        .ok()
                    })
                    .ok_or_else(|| {
                        ApiError::InternalServerError("Invalid event secret provenance".into())
                    })?;
                provenance.merge(previous);
            } else {
                provenance
                    .origins
                    .push(attune_common::secret_provenance::SecretOrigin::Local {
                        source_kind: secret.source_kind.clone(),
                        source_ref: secret.source_ref.clone(),
                    });
            }
            secret.source_kind = "provenance".into();
            secret.source_ref = Some(serde_json::to_string(&provenance).map_err(|_| {
                ApiError::InternalServerError("Event provenance could not be encoded".into())
            })?);
        }
    }
    let prepared_payload_secrets =
        prepare_event_secret_values(&state, redacted.payload_secrets).await?;
    let prepared_config_secrets =
        prepare_event_secret_values(&state, redacted.config_secrets).await?;

    // Create event input
    let mut input = CreateEventInput {
        trigger: Some(trigger.id),
        trigger_ref: trigger.r#ref.clone(),
        config: redacted.config,
        payload: redacted.payload,
        trace_tag: source_trace_tag,
        source: source_id,
        source_ref,
        rule: rule_id,
        rule_ref,
    };

    // Create the event
    if let Some(fence) = sensor_fence {
        SensorAdmissionRepository::lock_workload_checks(&mut tx).await?;
        let sensor_id = source_id.expect("sensor fence requires a resolved sensor");
        if !SensorWorkloadRepository::lock_current_fence(&mut tx, sensor_id, fence).await? {
            return Err(ApiError::Forbidden(
                "Sensor workload assignment is stale or expired".to_string(),
            ));
        }
        if !SensorWorkloadRepository::trigger_is_member(
            &mut tx,
            sensor_id,
            fence.workload_id,
            trigger.id,
        )
        .await?
        {
            return Err(ApiError::Forbidden(
                "Trigger is not a member of the sensor workload".to_string(),
            ));
        }
        input.trigger_ref = TriggerRepository::find_by_id(&mut *tx, trigger.id)
            .await?
            .ok_or_else(|| ApiError::Forbidden("Trigger no longer exists".to_string()))?
            .r#ref;
        if let Some(rule_id) = rule_id {
            if !SensorWorkloadRepository::rule_is_member(
                &mut tx,
                sensor_id,
                fence.workload_id,
                rule_id,
                trigger.id,
            )
            .await?
            {
                return Err(ApiError::Forbidden(
                    "Targeted rule is not a member of the sensor workload".to_string(),
                ));
            }
            input.rule_ref = Some(
                RuleRepository::find_by_id(&mut *tx, rule_id)
                    .await?
                    .ok_or_else(|| {
                        ApiError::Forbidden("Targeted rule no longer exists".to_string())
                    })?
                    .r#ref,
            );
        }
    }
    let event = EventRepository::create(&mut *tx, input).await?;
    if let Some(authority) = &recorded_authority {
        attune_common::repositories::execution_event_emitter::ExecutionEventEmitterRepository::attach(
            &mut tx, &event, authority,
        ).await?;
    }
    EventSchemaRepository::attach(&mut tx, event.id, &binding).await?;
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
    let grants = load_collection_grants(&state, &user, Resource::Events, RbacAction::Read).await?;
    let include_public_trigger_scope =
        allows_public_trigger_event_read_without_resource_grant(&user, RbacAction::Read);
    let (
        event_visibility,
        rule_read_scope,
        trigger_read_scope,
        global_event_read,
        include_execution_context,
    ) = if let Some(ref grants) = grants {
        let global_event_read =
            has_unconstrained_resource_action(grants, Resource::Events, RbacAction::Read);
        let visibility = event_visibility_filter_from_grants(grants, include_public_trigger_scope);
        let include_execution_context = global_event_read
            && has_unconstrained_resource_action(grants, Resource::Executions, RbacAction::Read);
        (
            (!global_event_read).then_some(visibility.clone()),
            visibility.rule_scope,
            visibility.trigger_scope,
            global_event_read,
            include_execution_context,
        )
    } else {
        (
            None,
            VisibilityReadScope {
                unconstrained: true,
                include_public: false,
                grants: Vec::new(),
            },
            VisibilityReadScope {
                unconstrained: true,
                include_public: false,
                grants: Vec::new(),
            },
            true,
            true,
        )
    };

    // All filtering and pagination happen in a single SQL query.
    let filters = EventSearchFilters {
        id: None,
        trigger: query.trigger,
        trigger_ref: query.trigger_ref.clone(),
        source: query.source,
        rule_ref: query.rule_ref.clone(),
        trace_tag: query.trace_tag.clone(),
        visibility: event_visibility,
        include_total: query.include_total == Some(true),
        limit: query.limit(),
        offset: query.offset(),
    };

    let result = EventRepository::search(&state.db, &filters).await?;

    let event_trace_tags = if include_execution_context {
        let event_ids: Vec<i64> = result.rows.iter().map(|event| event.id).collect();
        EventRepository::trace_tags_by_event_ids(&state.db, &event_ids).await?
    } else {
        std::collections::HashMap::new()
    };

    let mut paginated_events: Vec<EventSummary> =
        result.rows.into_iter().map(EventSummary::from).collect();
    for event in &mut paginated_events {
        if include_execution_context {
            event.trace_tag = event_trace_tags
                .get(&event.id)
                .cloned()
                .or_else(|| event.trace_tag.clone());
        }
        apply_event_summary_visibility(
            event,
            &rule_read_scope,
            &trigger_read_scope,
            global_event_read,
        );
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

    // Load effective grants once and reuse them for both the Read/Decrypt
    // access checks and the rule/trigger redaction scopes below, instead of
    // fetching them separately for each.
    let grants = if is_scoped_identity_token(&user) {
        Some(
            state
                .authorization_service()
                .effective_grants(&user)
                .await?,
        )
    } else {
        None
    };
    let mut event_visibility_cache: Option<bool> = None;
    let include_public_trigger_scope =
        allows_public_trigger_event_read_without_resource_grant(&user, RbacAction::Read);

    authorize_event_access(
        &state,
        &user,
        &event,
        RbacAction::Read,
        grants.as_deref(),
        &mut event_visibility_cache,
    )
    .await?;
    let (rule_read_scope, trigger_read_scope, global_event_read) = if let Some(ref grants) = grants
    {
        (
            build_visibility_read_scope(grants, Resource::Rules, RbacAction::Read, false),
            build_visibility_read_scope(
                grants,
                Resource::Triggers,
                RbacAction::Read,
                include_public_trigger_scope,
            ),
            has_unconstrained_resource_action(grants, Resource::Events, RbacAction::Read),
        )
    } else {
        (
            VisibilityReadScope {
                unconstrained: true,
                include_public: false,
                grants: Vec::new(),
            },
            VisibilityReadScope {
                unconstrained: true,
                include_public: false,
                grants: Vec::new(),
            },
            true,
        )
    };

    let reveal_paths = if query.include_secret_values {
        authorize_event_access(
            &state,
            &user,
            &event,
            RbacAction::Decrypt,
            grants.as_deref(),
            &mut event_visibility_cache,
        )
        .await?;
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
    let binding = EventSchemaRepository::find(&state.db, event.id).await?;
    response.schema_binding = binding
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| ApiError::InternalServerError("Invalid stored event schema".into()))?;
    if query.include_secret_values {
        if let Some(binding) = &binding {
            if !binding.matches_owner(&event) {
                return Err(ApiError::Forbidden(
                    "Event schema binding does not match its owner".into(),
                ));
            }
            // Validate complete internal plaintext, never a partially disclosed
            // projection whose denied fields still contain redaction markers.
            let payload = restore_bound_event_secret_entity(
                &state,
                response.payload.clone(),
                ENTITY_EVENT_PAYLOAD,
                event.id,
            )
            .await?;
            let config = restore_bound_event_secret_entity(
                &state,
                response.config.clone(),
                ENTITY_EVENT_CONFIG,
                event.id,
            )
            .await?;
            binding.validate_restored(payload.as_ref(), config.as_ref())?;
        } else {
            for entity_type in [ENTITY_EVENT_PAYLOAD, ENTITY_EVENT_CONFIG] {
                if !ExecutionSecretValueRepository::find_stored_by_entity(
                    &state.db,
                    entity_type,
                    event.id,
                )
                .await?
                .is_empty()
                {
                    return Err(ApiError::Forbidden(
                        "Event secret disclosure requires its original schema binding".into(),
                    ));
                }
            }
        }
        response.payload = crate::secret_disclosure::reveal_authorized(
            &state,
            &user,
            response.payload,
            ENTITY_EVENT_PAYLOAD,
            event.id,
        )
        .await?;
        response.config = crate::secret_disclosure::reveal_authorized(
            &state,
            &user,
            response.config,
            ENTITY_EVENT_CONFIG,
            event.id,
        )
        .await?;
        let mut still_redacted =
            redacted_paths(&response.payload.clone().unwrap_or(serde_json::Value::Null))
                .into_iter()
                .map(|path| format!("payload{path}"))
                .collect::<std::collections::BTreeSet<_>>();
        still_redacted.extend(
            redacted_paths(&response.config.clone().unwrap_or(serde_json::Value::Null))
                .into_iter()
                .map(|path| format!("config{path}")),
        );
        emit_event_secret_disclosure_audit(
            &state,
            &user,
            &event,
            reveal_paths
                .into_iter()
                .filter(|path| !still_redacted.contains(path))
                .collect(),
        );
    }
    apply_event_response_visibility(
        &mut response,
        &rule_read_scope,
        &trigger_read_scope,
        global_event_read,
    );
    if response.trigger_ref == REDACTED_REF {
        response.schema_binding = None;
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
    let grants =
        load_collection_grants(&state, &user, Resource::Enforcements, RbacAction::Read).await?;
    let (
        enforcement_visibility,
        rule_read_scope,
        trigger_read_scope,
        global_enforcement_read,
        include_execution_context,
    ) = if let Some(ref grants) = grants {
        let global_enforcement_read =
            has_unconstrained_resource_action(grants, Resource::Enforcements, RbacAction::Read);
        let visibility = enforcement_visibility_filter_from_grants(grants);
        let trigger_scope =
            build_visibility_read_scope(grants, Resource::Triggers, RbacAction::Read, false);
        let include_execution_context = global_enforcement_read
            && has_unconstrained_resource_action(grants, Resource::Executions, RbacAction::Read);
        (
            (!global_enforcement_read).then_some(visibility.clone()),
            visibility.rule_scope,
            trigger_scope,
            global_enforcement_read,
            include_execution_context,
        )
    } else {
        (
            None,
            VisibilityReadScope {
                unconstrained: true,
                include_public: false,
                grants: Vec::new(),
            },
            VisibilityReadScope {
                unconstrained: true,
                include_public: false,
                grants: Vec::new(),
            },
            true,
            true,
        )
    };

    // All filtering and pagination happen in a single SQL query.
    // Filters are combinable (AND), not mutually exclusive.
    let filters = EnforcementSearchFilters {
        id: None,
        status: query.status,
        rule: query.rule,
        event: query.event,
        trigger_ref: query.trigger_ref.clone(),
        rule_ref: query.rule_ref.clone(),
        trace_tag: query.trace_tag.clone(),
        visibility: enforcement_visibility,
        include_total: query.include_total == Some(true),
        limit: query.limit(),
        offset: query.offset(),
    };

    let result = EnforcementRepository::search(&state.db, &filters).await?;

    let enforcement_trace_tags = if include_execution_context {
        let enforcement_ids: Vec<i64> = result
            .rows
            .iter()
            .map(|enforcement| enforcement.id)
            .collect();
        EnforcementRepository::trace_tags_by_enforcement_ids(&state.db, &enforcement_ids).await?
    } else {
        std::collections::HashMap::new()
    };

    let mut paginated_enforcements: Vec<EnforcementSummary> = result
        .rows
        .into_iter()
        .map(EnforcementSummary::from)
        .collect();
    for enforcement in &mut paginated_enforcements {
        if include_execution_context {
            enforcement.trace_tag = enforcement_trace_tags.get(&enforcement.id).cloned();
        }
        apply_enforcement_summary_visibility(
            enforcement,
            &rule_read_scope,
            &trigger_read_scope,
            global_enforcement_read,
        );
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

fn push_bounded_event_origin(
    origins: &mut Vec<attune_common::secret_provenance::SecretOrigin>,
    bytes: &mut usize,
    origin: attune_common::secret_provenance::SecretOrigin,
) -> Result<(), ApiError> {
    let size = serde_json::to_vec(&origin)
        .map_err(|_| ApiError::InternalServerError("Event provenance could not be encoded".into()))?
        .len();
    *bytes = bytes
        .checked_add(size)
        .ok_or_else(|| ApiError::ValidationError("Event provenance exceeds its limit".into()))?;
    if origins.len() >= 1024 || *bytes > 1024 * 1024 {
        return Err(ApiError::ValidationError(
            "Event provenance exceeds its limit".into(),
        ));
    }
    origins.push(origin);
    Ok(())
}

const REDACTED_REF: &str = "[redacted]";

fn allows_scoped_collection_read_without_resource_grant(
    user: &AuthenticatedUser,
    resource: Resource,
    action: RbacAction,
) -> bool {
    user.claims.token_type == TokenType::Access
        && action == RbacAction::Read
        && matches!(resource, Resource::Events | Resource::Enforcements)
}

fn allows_public_trigger_event_read_without_resource_grant(
    user: &AuthenticatedUser,
    action: RbacAction,
) -> bool {
    user.claims.token_type == TokenType::Access && action == RbacAction::Read
}

pub(crate) fn event_visibility_filter_from_grants(
    grants: &[Grant],
    include_public_trigger_scope: bool,
) -> EventVisibilityFilter {
    EventVisibilityFilter {
        rule_scope: build_visibility_read_scope(grants, Resource::Rules, RbacAction::Read, false),
        trigger_scope: build_visibility_read_scope(
            grants,
            Resource::Triggers,
            RbacAction::Read,
            include_public_trigger_scope,
        ),
    }
}

fn enforcement_visibility_filter_from_grants(grants: &[Grant]) -> EnforcementVisibilityFilter {
    EnforcementVisibilityFilter {
        rule_scope: build_visibility_read_scope(grants, Resource::Rules, RbacAction::Read, false),
    }
}

async fn load_collection_grants(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    resource: Resource,
    action: RbacAction,
) -> Result<Option<Vec<Grant>>, ApiError> {
    if !is_scoped_identity_token(user) {
        return Ok(None);
    }

    let grants = state.authorization_service().effective_grants(user).await?;
    if resource_action_grant_exists(&grants, resource, action)
        || allows_scoped_collection_read_without_resource_grant(user, resource, action)
    {
        Ok(Some(grants))
    } else {
        Err(ApiError::Forbidden(format!(
            "Insufficient permissions: {}:{}",
            match resource {
                Resource::Events => "events",
                Resource::Enforcements => "enforcements",
                _ => "resource",
            },
            action_name(action)
        )))
    }
}

fn apply_event_summary_visibility(
    event: &mut EventSummary,
    rule_scope: &VisibilityReadScope,
    trigger_scope: &VisibilityReadScope,
    global_event_read: bool,
) {
    if global_event_read {
        return;
    }

    if !scope_allows_resource_ref(rule_scope, event.rule, event.rule_ref.as_deref()) {
        event.rule = None;
        event.rule_ref = None;
    }

    let trigger_visible = scope_allows_resource_ref(
        trigger_scope,
        event.trigger,
        Some(event.trigger_ref.as_str()),
    );
    if !(trigger_visible || (event.rule.is_none() && trigger_scope.include_public)) {
        event.trigger = None;
        event.trigger_ref = REDACTED_REF.to_string();
    }
}

fn apply_event_response_visibility(
    event: &mut EventResponse,
    rule_scope: &VisibilityReadScope,
    trigger_scope: &VisibilityReadScope,
    global_event_read: bool,
) {
    if global_event_read {
        return;
    }

    if !scope_allows_resource_ref(rule_scope, event.rule, event.rule_ref.as_deref()) {
        event.rule = None;
        event.rule_ref = None;
    }

    let trigger_visible = scope_allows_resource_ref(
        trigger_scope,
        event.trigger,
        Some(event.trigger_ref.as_str()),
    );
    if !(trigger_visible || (event.rule.is_none() && trigger_scope.include_public)) {
        event.trigger = None;
        event.trigger_ref = REDACTED_REF.to_string();
    }
}

fn apply_enforcement_summary_visibility(
    enforcement: &mut EnforcementSummary,
    rule_scope: &VisibilityReadScope,
    trigger_scope: &VisibilityReadScope,
    global_enforcement_read: bool,
) {
    if global_enforcement_read {
        return;
    }

    if !scope_allows_resource_ref(
        rule_scope,
        enforcement.rule,
        Some(enforcement.rule_ref.as_str()),
    ) {
        enforcement.rule = None;
        enforcement.rule_ref = REDACTED_REF.to_string();
    }

    if !scope_allows_resource_ref(trigger_scope, None, Some(enforcement.trigger_ref.as_str())) {
        enforcement.trigger_ref = REDACTED_REF.to_string();
    }
}

fn apply_enforcement_response_visibility(
    enforcement: &mut EnforcementResponse,
    rule_scope: &VisibilityReadScope,
    trigger_scope: &VisibilityReadScope,
    global_enforcement_read: bool,
) {
    if global_enforcement_read {
        return;
    }

    if !scope_allows_resource_ref(
        rule_scope,
        enforcement.rule,
        Some(enforcement.rule_ref.as_str()),
    ) {
        enforcement.rule = None;
        enforcement.rule_ref = REDACTED_REF.to_string();
    }

    if !scope_allows_resource_ref(trigger_scope, None, Some(enforcement.trigger_ref.as_str())) {
        enforcement.trigger_ref = REDACTED_REF.to_string();
    }
}

pub(crate) async fn authorize_event_access(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    event: &attune_common::models::event::Event,
    action: RbacAction,
    grants: Option<&[Grant]>,
    visibility_cache: &mut Option<bool>,
) -> Result<(), ApiError> {
    let Some(grants) = grants else {
        return Ok(());
    };

    let Some(filter) = event_action_visibility_filter(user, grants, action)? else {
        return Ok(());
    };
    let visible = match *visibility_cache {
        Some(visible) => visible,
        None => {
            let result = EventRepository::search(
                &state.db,
                &EventSearchFilters {
                    id: Some(event.id),
                    include_total: false,
                    limit: 1,
                    offset: 0,
                    visibility: Some(filter),
                    ..Default::default()
                },
            )
            .await?;
            let visible = !result.rows.is_empty();
            *visibility_cache = Some(visible);
            visible
        }
    };
    if !visible {
        return Err(ApiError::Forbidden(
            "Insufficient permissions: events visibility".into(),
        ));
    }
    Ok(())
}

pub(crate) fn event_action_visibility_filter(
    user: &AuthenticatedUser,
    grants: &[Grant],
    action: RbacAction,
) -> Result<Option<EventVisibilityFilter>, ApiError> {
    let has_resource_grant = resource_action_grant_exists(grants, Resource::Events, action);
    let allow_public_trigger_read =
        allows_public_trigger_event_read_without_resource_grant(user, action);
    if !has_resource_grant && !allow_public_trigger_read {
        return Err(ApiError::Forbidden(format!(
            "Insufficient permissions: events:{}",
            action_name(action)
        )));
    }
    if has_resource_grant && has_unconstrained_resource_action(grants, Resource::Events, action) {
        return Ok(None);
    }

    Ok(Some(event_visibility_filter_from_grants(
        grants,
        allow_public_trigger_read,
    )))
}

async fn restore_bound_event_secret_entity(
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
        ("id" = i64, Path, description = "Enforcement ID"),
        EnforcementDetailQueryParams
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

    // Load effective grants once and reuse them for both the Read/Decrypt
    // access checks and the rule/trigger redaction scopes below, instead of
    // fetching them separately for each.
    let grants = if is_scoped_identity_token(&user) {
        Some(
            state
                .authorization_service()
                .effective_grants(&user)
                .await?,
        )
    } else {
        None
    };
    let mut enforcement_visibility_cache: Option<bool> = None;

    authorize_enforcement_access(
        &state,
        &user,
        &enforcement,
        RbacAction::Read,
        grants.as_deref(),
        &mut enforcement_visibility_cache,
    )
    .await?;
    let (rule_read_scope, trigger_read_scope, global_enforcement_read, include_execution_context) =
        if let Some(ref grants) = grants {
            (
                build_visibility_read_scope(grants, Resource::Rules, RbacAction::Read, false),
                build_visibility_read_scope(grants, Resource::Triggers, RbacAction::Read, false),
                has_unconstrained_resource_action(grants, Resource::Enforcements, RbacAction::Read),
                has_unconstrained_resource_action(grants, Resource::Enforcements, RbacAction::Read)
                    && has_unconstrained_resource_action(
                        grants,
                        Resource::Executions,
                        RbacAction::Read,
                    ),
            )
        } else {
            (
                VisibilityReadScope {
                    unconstrained: true,
                    include_public: false,
                    grants: Vec::new(),
                },
                VisibilityReadScope {
                    unconstrained: true,
                    include_public: false,
                    grants: Vec::new(),
                },
                true,
                true,
            )
        };

    let reveal_paths = if query.include_secret_values {
        authorize_enforcement_access(
            &state,
            &user,
            &enforcement,
            RbacAction::Decrypt,
            grants.as_deref(),
            &mut enforcement_visibility_cache,
        )
        .await?;
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
    let binding =
        attune_common::repositories::schema_contract::SchemaContractRepository::find_with_executor(
            &state.db,
            attune_common::repositories::schema_contract::BindingOwner::Enforcement,
            enforcement.id,
        )
        .await?;
    response.input_contract = binding
        .as_ref()
        .map(|binding| serde_json::to_value(&binding.input))
        .transpose()
        .map_err(|_| {
            ApiError::InternalServerError("Invalid stored enforcement input schema".into())
        })?;
    if let Some(event) = enforcement.event {
        let binding = EventSchemaRepository::find(&state.db, event).await?;
        response.payload_contract = binding
            .and_then(|binding| binding.payload)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| {
                ApiError::InternalServerError("Invalid original enforcement payload schema".into())
            })?;
    }
    if include_execution_context {
        let mut enforcement_trace_tags =
            EnforcementRepository::trace_tags_by_enforcement_ids(&state.db, &[id]).await?;
        response.trace_tag = enforcement_trace_tags.remove(&id);
    }
    if query.include_secret_values {
        response.config = crate::secret_disclosure::reveal_authorized(
            &state,
            &user,
            response.config,
            attune_common::secret_values::ENTITY_ENFORCEMENT_CONFIG,
            enforcement.id,
        )
        .await?;
        emit_enforcement_secret_disclosure_audit(&state, &user, &enforcement, reveal_paths);
    }
    apply_enforcement_response_visibility(
        &mut response,
        &rule_read_scope,
        &trigger_read_scope,
        global_enforcement_read,
    );

    let response = ApiResponse::new(response);

    Ok((StatusCode::OK, Json(response)))
}

async fn authorize_enforcement_access(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    enforcement: &attune_common::models::event::Enforcement,
    action: RbacAction,
    grants: Option<&[Grant]>,
    visibility_cache: &mut Option<bool>,
) -> Result<(), ApiError> {
    let Some(grants) = grants else {
        return Ok(());
    };

    let has_resource_grant = resource_action_grant_exists(grants, Resource::Enforcements, action);
    if !has_resource_grant
        && !allows_scoped_collection_read_without_resource_grant(
            user,
            Resource::Enforcements,
            action,
        )
    {
        return Err(ApiError::Forbidden(format!(
            "Insufficient permissions: enforcements:{}",
            action_name(action)
        )));
    }
    if has_resource_grant
        && has_unconstrained_resource_action(grants, Resource::Enforcements, action)
    {
        return Ok(());
    }

    // The rule visibility predicate below does not depend on `action`, so it
    // is computed at most once per enforcement per request and reused across
    // the Read/Decrypt checks instead of re-running the same search query.
    let visible = match *visibility_cache {
        Some(visible) => visible,
        None => {
            let filters = EnforcementSearchFilters {
                id: Some(enforcement.id),
                include_total: false,
                limit: 1,
                offset: 0,
                visibility: Some(enforcement_visibility_filter_from_grants(grants)),
                ..Default::default()
            };
            let result = EnforcementRepository::search(&state.db, &filters).await?;
            let visible = !result.rows.is_empty();
            *visibility_cache = Some(visible);
            visible
        }
    };
    if !visible {
        return Err(ApiError::Forbidden(
            "Insufficient permissions: enforcements visibility".to_string(),
        ));
    }

    Ok(())
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
    use axum::handler::Handler;
    Router::new()
        .route(
            "/events",
            get(list_events).post(create_event.layer(axum::Extension(
                crate::auth::middleware::RecordedRuntimeConsumer::EventIngress,
            ))),
        )
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
    fn direct_event_token_types_are_explicitly_limited() {
        assert!(event_token_type_allowed(&TokenType::Sensor));
        assert!(event_token_type_allowed(&TokenType::Execution));
        assert!(!event_token_type_allowed(&TokenType::Access));
        assert!(!event_token_type_allowed(&TokenType::Worker));
        assert!(!event_token_type_allowed(&TokenType::Refresh));
    }

    #[test]
    fn targeted_rule_instance_requires_exact_positive_numeric_id() {
        assert_eq!(parse_rule_instance_id(None).unwrap(), None);
        assert_eq!(parse_rule_instance_id(Some("rule_42")).unwrap(), Some(42));

        for invalid in ["rule_0", "rule_-1", "rule_nope", "other_42", "rule_42x"] {
            assert!(parse_rule_instance_id(Some(invalid)).is_err(), "{invalid}");
        }
    }

    #[test]
    fn sensor_trigger_scope_requires_an_exact_ref_match() {
        let scope = vec![
            "core.intervaltimer".to_string(),
            "core.crontimer".to_string(),
        ];

        assert!(sensor_trigger_scope_allows(&scope, "core.intervaltimer"));
        assert!(!sensor_trigger_scope_allows(&scope, "core.timer"));
        assert!(!sensor_trigger_scope_allows(&[], "core.intervaltimer"));
    }

    #[test]
    fn redacts_event_payload_from_flat_trigger_schema() {
        let schema = json!({
            "username": {"type": "string"},
            "password": {"type": "string", "secret": true}
        });

        let binding = event_test_binding("demo.login", Some(&schema), None);
        let redacted = binding
            .prepare(
                Some(json!({"username": "alice", "password": "s3cr3t"})),
                None,
            )
            .unwrap();

        let payload = redacted.payload.unwrap();
        assert_eq!(payload["username"], "alice");
        assert!(is_redaction_marker(&payload["password"]));
        assert_eq!(redacted.payload_secrets.len(), 1);
        assert_eq!(redacted.payload_secrets[0].json_path, "/password");
        let secret = &redacted.payload_secrets[0];
        assert_eq!(secret.source_kind, "provenance");
        let provenance: attune_common::secret_provenance::SecretProvenance =
            serde_json::from_str(secret.source_ref.as_deref().unwrap()).unwrap();
        assert_eq!(
            provenance.origins,
            vec![attune_common::secret_provenance::SecretOrigin::Local {
                source_kind: "trigger_schema".into(),
                source_ref: Some("demo.login:payload:/password".into()),
            }]
        );
    }

    #[test]
    fn redacts_event_payload_and_config_from_distinct_flat_schemas() {
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

        let binding = event_test_binding(
            "demo.webhook",
            Some(&schema["payload"]["properties"]),
            Some(&schema["config"]["properties"]),
        );
        let redacted = binding
            .prepare(
                Some(json!({"api_key": "payload-secret", "message": "ok"})),
                Some(json!({"headers": {"authorization": "Bearer secret"}})),
            )
            .unwrap();

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

    fn event_test_binding(
        reference: &str,
        payload: Option<&JsonValue>,
        config: Option<&JsonValue>,
    ) -> attune_common::event_schema::EventSchemaBinding {
        use attune_common::value_schema::{
            compile, Limits, RegistryView, SchemaContext, SourceIdentity,
        };
        let compile_section = |schema: Option<&JsonValue>| {
            schema.map(|schema| {
                compile(
                    SourceIdentity::Assigned {
                        component_ref: reference.to_owned(),
                        assignment_id: uuid::Uuid::new_v4(),
                    },
                    SchemaContext::FlatDocument,
                    schema,
                    &RegistryView::new(),
                    Limits::default(),
                )
                .unwrap()
            })
        };
        attune_common::event_schema::EventSchemaBinding {
            trigger_id: Some(1),
            trigger_ref: reference.to_owned(),
            payload: compile_section(payload),
            config: compile_section(config),
            webhook: false,
        }
    }
}
