//! Webhook management and receiver API routes

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use std::sync::Arc;
use std::time::Instant;

use attune_common::{
    mq::{EventCreatedPayload, MessageEnvelope, MessageType},
    repositories::{
        event::{CreateEventInput, EventRepository},
        execution_secret_value::ExecutionSecretValueRepository,
        trigger::{TriggerRepository, WebhookEventLogInput},
        Create, FindById, FindByRef,
    },
    secret_values::{prepare_secret_values, ENTITY_EVENT_CONFIG, ENTITY_EVENT_PAYLOAD},
};

use attune_common::rbac::{Action, AuthorizationContext, Resource};

use crate::{
    auth::middleware::RequireAuth,
    authz::AuthorizationCheck,
    dto::{
        trigger::TriggerResponse,
        webhook::{WebhookReceiverRequest, WebhookReceiverResponse},
        ApiResponse,
    },
    middleware::{ApiError, ApiResult},
    routes::events::redact_event_parts_for_trigger,
    routes::triggers::can_access_trigger_api,
    state::AppState,
    webhook_security,
};

// ============================================================================
// WEBHOOK CONFIG HELPERS
// ============================================================================

/// Helper to extract boolean value from webhook_config JSON using path notation
fn get_webhook_config_bool(
    trigger: &attune_common::models::trigger::Trigger,
    path: &str,
    default: bool,
) -> bool {
    let config = match &trigger.webhook_config {
        Some(c) => c,
        None => return default,
    };

    let parts: Vec<&str> = path.split('/').collect();
    let mut current = config;

    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Last part - extract value
            return current
                .get(part)
                .and_then(|v| v.as_bool())
                .unwrap_or(default);
        } else {
            // Intermediate part - navigate deeper
            current = match current.get(part) {
                Some(v) => v,
                None => return default,
            };
        }
    }

    default
}

/// Helper to extract string value from webhook_config JSON using path notation
fn get_webhook_config_str(
    trigger: &attune_common::models::trigger::Trigger,
    path: &str,
) -> Option<String> {
    let config = trigger.webhook_config.as_ref()?;

    let parts: Vec<&str> = path.split('/').collect();
    let mut current = config;

    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Last part - extract value
            return current
                .get(part)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        } else {
            // Intermediate part - navigate deeper
            current = current.get(part)?;
        }
    }

    None
}

/// Helper to extract i64 value from webhook_config JSON using path notation
fn get_webhook_config_i64(
    trigger: &attune_common::models::trigger::Trigger,
    path: &str,
) -> Option<i64> {
    let config = trigger.webhook_config.as_ref()?;

    let parts: Vec<&str> = path.split('/').collect();
    let mut current = config;

    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Last part - extract value
            return current.get(part).and_then(|v| v.as_i64());
        } else {
            // Intermediate part - navigate deeper
            current = current.get(part)?;
        }
    }

    None
}

/// Helper to extract array of strings from webhook_config JSON using path notation
fn get_webhook_config_array(
    trigger: &attune_common::models::trigger::Trigger,
    path: &str,
) -> Option<Vec<String>> {
    let config = trigger.webhook_config.as_ref()?;

    let parts: Vec<&str> = path.split('/').collect();
    let mut current = config;

    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Last part - extract array
            return current.get(part).and_then(|v| {
                v.as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect()
                })
            });
        } else {
            // Intermediate part - navigate deeper
            current = current.get(part)?;
        }
    }

    None
}

// ============================================================================
// WEBHOOK MANAGEMENT ENDPOINTS
// ============================================================================

/// Enable webhooks for a trigger
#[utoipa::path(
    post,
    path = "/api/v1/triggers/{ref}/webhooks/enable",
    tag = "webhooks",
    params(
        ("ref" = String, Path, description = "Trigger reference (pack.name)")
    ),
    responses(
        (status = 200, description = "Webhooks enabled", body = TriggerResponse),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("jwt" = [])
    )
)]
pub async fn enable_webhook(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(trigger_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // First, find the trigger by ref to get its ID
    let trigger = TriggerRepository::find_by_ref(&state.db, &trigger_ref)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;
    if !can_access_trigger_api(&state, &user, &trigger, None).await? {
        return Err(ApiError::NotFound(format!(
            "Trigger '{}' not found",
            trigger_ref
        )));
    }

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = state.authorization_service();
        let mut ctx = AuthorizationContext::new(identity_id);
        ctx.target_ref = Some(trigger.r#ref.clone());
        ctx.pack_ref = trigger.pack_ref.clone();
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Triggers,
                    action: Action::Update,
                    context: ctx,
                },
            )
            .await?;
    }

    // Enable webhooks for this trigger
    let _webhook_info = TriggerRepository::enable_webhook(&state.db, trigger.id)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

    // Fetch the updated trigger to return
    let updated_trigger = TriggerRepository::find_by_id(&state.db, trigger.id)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("Trigger not found after update".to_string()))?;

    let response = TriggerResponse::from(updated_trigger);
    Ok(Json(ApiResponse::new(response)))
}

/// Disable webhooks for a trigger
#[utoipa::path(
    post,
    path = "/api/v1/triggers/{ref}/webhooks/disable",
    tag = "webhooks",
    params(
        ("ref" = String, Path, description = "Trigger reference (pack.name)")
    ),
    responses(
        (status = 200, description = "Webhooks disabled", body = TriggerResponse),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("jwt" = [])
    )
)]
pub async fn disable_webhook(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(trigger_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // First, find the trigger by ref to get its ID
    let trigger = TriggerRepository::find_by_ref(&state.db, &trigger_ref)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;
    if !can_access_trigger_api(&state, &user, &trigger, None).await? {
        return Err(ApiError::NotFound(format!(
            "Trigger '{}' not found",
            trigger_ref
        )));
    }

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = state.authorization_service();
        let mut ctx = AuthorizationContext::new(identity_id);
        ctx.target_ref = Some(trigger.r#ref.clone());
        ctx.pack_ref = trigger.pack_ref.clone();
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Triggers,
                    action: Action::Update,
                    context: ctx,
                },
            )
            .await?;
    }

    // Disable webhooks for this trigger
    TriggerRepository::disable_webhook(&state.db, trigger.id)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

    // Fetch the updated trigger to return
    let updated_trigger = TriggerRepository::find_by_id(&state.db, trigger.id)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("Trigger not found after update".to_string()))?;

    let response = TriggerResponse::from(updated_trigger);
    Ok(Json(ApiResponse::new(response)))
}

/// Regenerate webhook key for a trigger
#[utoipa::path(
    post,
    path = "/api/v1/triggers/{ref}/webhooks/regenerate",
    tag = "webhooks",
    params(
        ("ref" = String, Path, description = "Trigger reference (pack.name)")
    ),
    responses(
        (status = 200, description = "Webhook key regenerated", body = TriggerResponse),
        (status = 400, description = "Webhooks not enabled for this trigger"),
        (status = 404, description = "Trigger not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("jwt" = [])
    )
)]
pub async fn regenerate_webhook_key(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(trigger_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    // First, find the trigger by ref to get its ID
    let trigger = TriggerRepository::find_by_ref(&state.db, &trigger_ref)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("Trigger '{}' not found", trigger_ref)))?;
    if !can_access_trigger_api(&state, &user, &trigger, None).await? {
        return Err(ApiError::NotFound(format!(
            "Trigger '{}' not found",
            trigger_ref
        )));
    }

    if user.claims.token_type == crate::auth::jwt::TokenType::Access {
        let identity_id = user
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;
        let authz = state.authorization_service();
        let mut ctx = AuthorizationContext::new(identity_id);
        ctx.target_ref = Some(trigger.r#ref.clone());
        ctx.pack_ref = trigger.pack_ref.clone();
        authz
            .authorize(
                &user,
                AuthorizationCheck {
                    resource: Resource::Triggers,
                    action: Action::Update,
                    context: ctx,
                },
            )
            .await?;
    }

    // Check if webhooks are enabled
    if !trigger.webhook_enabled {
        return Err(ApiError::BadRequest(
            "Webhooks are not enabled for this trigger. Enable webhooks first.".to_string(),
        ));
    }

    // Regenerate the webhook key
    let _regenerate_result = TriggerRepository::regenerate_webhook_key(&state.db, trigger.id)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

    // Fetch the updated trigger to return
    let updated_trigger = TriggerRepository::find_by_id(&state.db, trigger.id)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("Trigger not found after update".to_string()))?;

    let response = TriggerResponse::from(updated_trigger);
    Ok(Json(ApiResponse::new(response)))
}

// ============================================================================
// WEBHOOK RECEIVER ENDPOINT
// ============================================================================

/// Webhook receiver endpoint - receives webhook events and creates events
#[utoipa::path(
    post,
    path = "/api/v1/webhooks/{webhook_key}",
    tag = "webhooks",
    params(
        ("webhook_key" = String, Path, description = "Webhook key")
    ),
    request_body = WebhookReceiverRequest,
    responses(
        (status = 200, description = "Webhook received and event created", body = WebhookReceiverResponse),
        (status = 404, description = "Invalid webhook key"),
        (status = 429, description = "Rate limit exceeded"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn receive_webhook(
    State(state): State<Arc<AppState>>,
    Path(webhook_key): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<impl IntoResponse> {
    let start_time = Instant::now();

    // Extract metadata from headers
    let source_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
        .map(|s| s.to_string());

    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let signature = headers
        .get("x-webhook-signature")
        .or_else(|| headers.get("x-hub-signature-256"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Parse JSON payload
    let payload: WebhookReceiverRequest = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("Invalid JSON payload: {}", e)))?;

    let payload_size_bytes = body.len() as i32;

    // Look up trigger by webhook key
    let trigger = match TriggerRepository::find_by_webhook_key(&state.db, &webhook_key).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            // Log failed attempt
            let _ = log_webhook_failure(
                &state,
                webhook_key.clone(),
                source_ip.clone(),
                user_agent.clone(),
                payload_size_bytes,
                404,
                "Invalid webhook key".to_string(),
                start_time,
            )
            .await;
            return Err(ApiError::NotFound("Invalid webhook key".to_string()));
        }
        Err(e) => {
            let _ = log_webhook_failure(
                &state,
                webhook_key.clone(),
                source_ip.clone(),
                user_agent.clone(),
                payload_size_bytes,
                500,
                e.to_string(),
                start_time,
            )
            .await;
            return Err(ApiError::InternalServerError(e.to_string()));
        }
    };

    // Verify the trigger itself is enabled. A disabled trigger rejects all
    // event ingress, including webhooks.
    if !trigger.enabled {
        let _ = log_webhook_event(
            &state,
            &trigger,
            &webhook_key,
            None,
            source_ip.clone(),
            user_agent.clone(),
            payload_size_bytes,
            400,
            Some("Trigger is disabled".to_string()),
            start_time,
            None,
            false,
            None,
        )
        .await;
        return Err(ApiError::BadRequest(format!(
            "Trigger '{}' is disabled",
            trigger.r#ref
        )));
    }

    // Verify webhooks are enabled
    if !trigger.webhook_enabled {
        let _ = log_webhook_event(
            &state,
            &trigger,
            &webhook_key,
            None,
            source_ip.clone(),
            user_agent.clone(),
            payload_size_bytes,
            400,
            Some("Webhooks not enabled for this trigger".to_string()),
            start_time,
            None,
            false,
            None,
        )
        .await;
        return Err(ApiError::BadRequest(
            "Webhooks are not enabled for this trigger".to_string(),
        ));
    }

    // Phase 3: Check payload size limit
    if let Some(limit_kb) = get_webhook_config_i64(&trigger, "payload_size_limit_kb") {
        let limit_bytes = limit_kb * 1024;
        if i64::from(payload_size_bytes) > limit_bytes {
            let _ = log_webhook_event(
                &state,
                &trigger,
                &webhook_key,
                None,
                source_ip.clone(),
                user_agent.clone(),
                payload_size_bytes,
                413,
                Some(format!(
                    "Payload too large: {} bytes (limit: {} bytes)",
                    payload_size_bytes, limit_bytes
                )),
                start_time,
                None,
                false,
                None,
            )
            .await;
            return Err(ApiError::BadRequest(format!(
                "Payload too large. Maximum size: {} KB",
                limit_kb
            )));
        }
    }

    // Phase 3: Check IP whitelist
    let ip_whitelist_enabled = get_webhook_config_bool(&trigger, "ip_whitelist/enabled", false);
    let ip_allowed = if ip_whitelist_enabled {
        if let Some(ref ip) = source_ip {
            if let Some(whitelist) = get_webhook_config_array(&trigger, "ip_whitelist/ips") {
                match webhook_security::check_ip_in_whitelist(ip, &whitelist) {
                    Ok(allowed) => {
                        if !allowed {
                            let _ = log_webhook_event(
                                &state,
                                &trigger,
                                &webhook_key,
                                None,
                                source_ip.clone(),
                                user_agent.clone(),
                                payload_size_bytes,
                                403,
                                Some("IP address not in whitelist".to_string()),
                                start_time,
                                None,
                                false,
                                Some(false),
                            )
                            .await;
                            return Err(ApiError::Forbidden("IP address not allowed".to_string()));
                        }
                        Some(true)
                    }
                    Err(e) => {
                        tracing::warn!("IP whitelist check error: {}", e);
                        Some(false)
                    }
                }
            } else {
                Some(false)
            }
        } else {
            Some(false)
        }
    } else {
        None
    };

    // Phase 3: Check rate limit
    let rate_limit_enabled = get_webhook_config_bool(&trigger, "rate_limit/enabled", false);
    if rate_limit_enabled {
        if let (Some(max_requests), Some(window_seconds)) = (
            get_webhook_config_i64(&trigger, "rate_limit/requests"),
            get_webhook_config_i64(&trigger, "rate_limit/window_seconds"),
        ) {
            if max_requests <= 0 || window_seconds <= 0 {
                tracing::warn!(
                    trigger_ref = %trigger.r#ref,
                    max_requests,
                    window_seconds,
                    "Ignoring invalid webhook rate limit configuration"
                );
            } else {
                let recent_requests = TriggerRepository::count_recent_webhook_requests(
                    &state.db,
                    trigger.id,
                    &webhook_key,
                    source_ip.as_deref(),
                    window_seconds as i32,
                )
                .await?;

                if recent_requests >= max_requests {
                    let _ = log_webhook_event(
                        &state,
                        &trigger,
                        &webhook_key,
                        None,
                        source_ip.clone(),
                        user_agent.clone(),
                        payload_size_bytes,
                        429,
                        Some("Rate limit exceeded".to_string()),
                        start_time,
                        None,
                        true,
                        ip_allowed,
                    )
                    .await;
                    return Err(ApiError::TooManyRequests(format!(
                        "Rate limit exceeded. Maximum {} requests per {} seconds",
                        max_requests, window_seconds
                    )));
                }
            }
        }
    }

    // Phase 3: Verify HMAC signature
    let hmac_enabled = get_webhook_config_bool(&trigger, "hmac/enabled", false);
    let hmac_verified = if hmac_enabled {
        if let (Some(secret), Some(algorithm)) = (
            get_webhook_config_str(&trigger, "hmac/secret"),
            get_webhook_config_str(&trigger, "hmac/algorithm"),
        ) {
            if let Some(sig) = signature {
                match webhook_security::verify_hmac_signature(&body, &sig, &secret, &algorithm) {
                    Ok(valid) => {
                        if !valid {
                            let _ = log_webhook_event(
                                &state,
                                &trigger,
                                &webhook_key,
                                None,
                                source_ip.clone(),
                                user_agent.clone(),
                                payload_size_bytes,
                                401,
                                Some("Invalid HMAC signature".to_string()),
                                start_time,
                                Some(false),
                                false,
                                ip_allowed,
                            )
                            .await;
                            return Err(ApiError::Unauthorized(
                                "Invalid webhook signature".to_string(),
                            ));
                        }
                        Some(true)
                    }
                    Err(e) => {
                        let _ = log_webhook_event(
                            &state,
                            &trigger,
                            &webhook_key,
                            None,
                            source_ip.clone(),
                            user_agent.clone(),
                            payload_size_bytes,
                            401,
                            Some(format!("HMAC verification error: {}", e)),
                            start_time,
                            Some(false),
                            false,
                            ip_allowed,
                        )
                        .await;
                        return Err(ApiError::Unauthorized(format!(
                            "Signature verification failed: {}",
                            e
                        )));
                    }
                }
            } else {
                let _ = log_webhook_event(
                    &state,
                    &trigger,
                    &webhook_key,
                    None,
                    source_ip.clone(),
                    user_agent.clone(),
                    payload_size_bytes,
                    401,
                    Some("HMAC signature required but not provided".to_string()),
                    start_time,
                    Some(false),
                    false,
                    ip_allowed,
                )
                .await;
                return Err(ApiError::Unauthorized("Signature required".to_string()));
            }
        } else {
            None
        }
    } else {
        None
    };

    // Build config with webhook context metadata
    let mut config = serde_json::json!({
        "source": "webhook",
        "webhook_key": webhook_key,
        "received_at": chrono::Utc::now().to_rfc3339(),
    });

    // Add optional metadata
    if let Some(headers) = payload.headers {
        config["headers"] = headers;
    }
    if let Some(ref ip) = source_ip {
        config["source_ip"] = serde_json::Value::String(ip.clone());
    }
    if let Some(ref ua) = user_agent {
        config["user_agent"] = serde_json::Value::String(ua.clone());
    }
    let hmac_enabled = get_webhook_config_bool(&trigger, "hmac/enabled", false);
    if hmac_enabled {
        config["hmac_verified"] = serde_json::Value::Bool(hmac_verified.unwrap_or(false));
    }

    let redacted = redact_event_parts_for_trigger(
        &trigger.r#ref,
        trigger.param_schema.as_ref(),
        Some(payload.payload),
        Some(config),
    );
    let prepared_payload_secrets =
        prepare_webhook_secret_values(&state, redacted.payload_secrets).await?;
    let prepared_config_secrets =
        prepare_webhook_secret_values(&state, redacted.config_secrets).await?;

    // Create event
    let event_input = CreateEventInput {
        trigger: Some(trigger.id),
        trigger_ref: trigger.r#ref.clone(),
        config: redacted.config,
        payload: redacted.payload,
        trace_tag: None,
        source: None,
        source_ref: Some("webhook".to_string()),
        rule: None,
        rule_ref: None,
    };

    let mut tx = state.db.begin().await?;
    let event = EventRepository::create(&mut *tx, event_input)
        .await
        .map_err(|e| {
            let _ = futures::executor::block_on(log_webhook_event(
                &state,
                &trigger,
                &webhook_key,
                None,
                source_ip.clone(),
                user_agent.clone(),
                payload_size_bytes,
                500,
                Some(format!("Failed to create event: {}", e)),
                start_time,
                hmac_verified,
                false,
                ip_allowed,
            ));
            ApiError::InternalServerError(e.to_string())
        })?;
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
    tracing::info!(
        "Webhook event {} created, attempting to publish EventCreated message",
        event.id
    );
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
            .with_source("api-webhook-receiver");

        if let Err(e) = publisher.publish_envelope(&envelope).await {
            tracing::warn!(
                "Failed to publish EventCreated message for event {}: {}",
                event.id,
                e
            );
            // Continue even if message publishing fails - event is already recorded
        } else {
            tracing::info!(
                "Published EventCreated message for event {} (trigger: {})",
                event.id,
                event.trigger_ref
            );
        }
    } else {
        tracing::warn!(
            "Publisher not available, cannot publish EventCreated message for event {}",
            event.id
        );
    }

    // Log successful webhook
    let _ = log_webhook_event(
        &state,
        &trigger,
        &webhook_key,
        Some(event.id),
        source_ip.clone(),
        user_agent.clone(),
        payload_size_bytes,
        200,
        None,
        start_time,
        hmac_verified,
        false,
        ip_allowed,
    )
    .await;

    let response = WebhookReceiverResponse {
        event_id: event.id,
        trigger_ref: trigger.r#ref.clone(),
        received_at: event.created,
        message: "Webhook received successfully".to_string(),
    };

    Ok(Json(ApiResponse::new(response)))
}

async fn prepare_webhook_secret_values(
    state: &AppState,
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

// Helper function to log webhook events
#[allow(clippy::too_many_arguments)]
async fn log_webhook_event(
    state: &AppState,
    trigger: &attune_common::models::trigger::Trigger,
    webhook_key: &str,
    event_id: Option<i64>,
    source_ip: Option<String>,
    user_agent: Option<String>,
    payload_size_bytes: i32,
    status_code: i32,
    error_message: Option<String>,
    start_time: Instant,
    hmac_verified: Option<bool>,
    rate_limited: bool,
    ip_allowed: Option<bool>,
) -> Result<(), attune_common::error::Error> {
    let processing_time_ms = start_time.elapsed().as_millis() as i32;

    let log_input = WebhookEventLogInput {
        trigger_id: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        webhook_key: webhook_key.to_string(),
        event_id,
        source_ip,
        user_agent,
        payload_size_bytes: Some(payload_size_bytes),
        headers: None, // Could be added if needed
        status_code,
        error_message,
        processing_time_ms: Some(processing_time_ms),
        hmac_verified,
        rate_limited,
        ip_allowed,
    };

    TriggerRepository::log_webhook_event(&state.db, log_input).await?;
    Ok(())
}

// Helper function to log failures when trigger is not found
#[allow(clippy::too_many_arguments)]
async fn log_webhook_failure(
    _state: &AppState,
    webhook_key: String,
    source_ip: Option<String>,
    user_agent: Option<String>,
    payload_size_bytes: i32,
    status_code: i32,
    error_message: String,
    start_time: Instant,
) -> Result<(), attune_common::error::Error> {
    let processing_time_ms = start_time.elapsed().as_millis() as i32;

    // We can't log to webhook_event_log without a trigger_id, so just log to tracing
    tracing::warn!(
        webhook_key = %webhook_key,
        source_ip = ?source_ip,
        user_agent = ?user_agent,
        payload_size_bytes = payload_size_bytes,
        status_code = status_code,
        error_message = %error_message,
        processing_time_ms = processing_time_ms,
        "Webhook request failed"
    );
    Ok(())
}

// ============================================================================
// ROUTER
// ============================================================================

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Webhook management routes (protected)
        .route("/triggers/{ref}/webhooks/enable", post(enable_webhook))
        .route("/triggers/{ref}/webhooks/disable", post(disable_webhook))
        .route(
            "/triggers/{ref}/webhooks/regenerate",
            post(regenerate_webhook_key),
        )
        // TODO: Add Phase 3 management endpoints for HMAC, rate limiting, IP whitelist
        // Webhook receiver route (public - no auth required)
        .route("/webhooks/{webhook_key}", post(receive_webhook))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_routes_structure() {
        let _router = routes();
    }
}
