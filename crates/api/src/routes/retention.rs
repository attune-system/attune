//! Runtime retention configuration API routes.

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use std::sync::Arc;

use attune_common::{
    audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome},
    config::RetentionConfig,
    rbac::{Action, AuthorizationContext, Resource},
    repositories::retention::RetentionRepository,
};

use crate::{
    auth::RequireAuth,
    authz::AuthorizationCheck,
    dto::ApiResponse,
    middleware::{ApiError, ApiResult},
    state::AppState,
};

fn retention_check(action: Action) -> AuthorizationCheck {
    AuthorizationCheck {
        resource: Resource::Retention,
        action,
        context: AuthorizationContext::new(0),
    }
}

/// Get runtime retention configuration.
#[utoipa::path(
    get,
    path = "/api/v1/retention-config",
    tag = "retention",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Runtime retention configuration", body = ApiResponse<RetentionConfig>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_retention_config(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
) -> ApiResult<impl IntoResponse> {
    let authz = state.authorization_service();
    authz
        .authorize(&user.0, retention_check(Action::Read))
        .await?;

    let config = RetentionRepository::load_config(&state.db).await?;
    Ok((StatusCode::OK, Json(ApiResponse::new(config))))
}

/// Update runtime retention configuration.
#[utoipa::path(
    put,
    path = "/api/v1/retention-config",
    tag = "retention",
    request_body = RetentionConfig,
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Runtime retention configuration updated", body = ApiResponse<RetentionConfig>),
        (status = 400, description = "Invalid retention configuration"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn update_retention_config(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(request): Json<RetentionConfig>,
) -> ApiResult<impl IntoResponse> {
    let authz = state.authorization_service();
    authz
        .authorize(&user.0, retention_check(Action::Update))
        .await?;

    validate_retention_config(&request)?;

    let previous = RetentionRepository::load_config(&state.db).await?;
    let updated = RetentionRepository::update_config(&state.db, &request).await?;

    emit_retention_config_audit(&state, &user.0, &previous, &updated);

    Ok((StatusCode::OK, Json(ApiResponse::new(updated))))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/retention-config",
        get(get_retention_config).put(update_retention_config),
    )
}

fn validate_retention_config(config: &RetentionConfig) -> ApiResult<()> {
    if config.check_interval_seconds == 0 {
        return Err(ApiError::BadRequest(
            "retention.check_interval_seconds must be greater than zero".to_string(),
        ));
    }
    if config.batch_size <= 0 {
        return Err(ApiError::BadRequest(
            "retention.batch_size must be greater than zero".to_string(),
        ));
    }
    let cache = &config.cache_retention;
    for (field, value) in [
        ("batch_size", cache.batch_size),
        (
            "max_batches_per_generation",
            cache.max_batches_per_generation,
        ),
        ("max_generations_per_cycle", cache.max_generations_per_cycle),
        ("max_namespaces_per_cycle", cache.max_namespaces_per_cycle),
    ] {
        if value <= 0 {
            return Err(ApiError::BadRequest(format!(
                "retention.cache_retention.{field} must be greater than zero"
            )));
        }
    }
    if cache.staging_failure_alert_threshold == 0 {
        return Err(ApiError::BadRequest(
            "retention.cache_retention.staging_failure_alert_threshold must be greater than zero"
                .to_string(),
        ));
    }
    if cache.alert_limit_per_cycle < 0 {
        return Err(ApiError::BadRequest(
            "retention.cache_retention.alert_limit_per_cycle must be nonnegative".to_string(),
        ));
    }

    for (target, target_config) in [
        ("events", &config.targets.events),
        ("enforcements", &config.targets.enforcements),
        ("executions", &config.targets.executions),
        ("execution_history", &config.targets.execution_history),
        ("worker_history", &config.targets.worker_history),
        (
            "sensor_process_history",
            &config.targets.sensor_process_history,
        ),
        ("audit_events", &config.targets.audit_events),
        (
            "continuous_aggregates",
            &config.targets.continuous_aggregates,
        ),
        ("notifications", &config.targets.notifications),
        ("webhook_event_logs", &config.targets.webhook_event_logs),
        ("inquiries", &config.targets.inquiries),
        ("work_queue_items", &config.targets.work_queue_items),
        (
            "work_queue_dispatches",
            &config.targets.work_queue_dispatches,
        ),
        ("pack_test_executions", &config.targets.pack_test_executions),
        ("execution_admission", &config.targets.execution_admission),
        ("workers", &config.targets.workers),
        ("sensor_processes", &config.targets.sensor_processes),
    ] {
        if target_config.max_age_seconds == Some(0) {
            return Err(ApiError::BadRequest(format!(
                "retention.targets.{target}.max_age_seconds must be greater than zero or null"
            )));
        }
    }

    Ok(())
}

fn emit_retention_config_audit(
    state: &Arc<AppState>,
    user: &crate::auth::middleware::AuthenticatedUser,
    previous: &RetentionConfig,
    updated: &RetentionConfig,
) {
    let mut builder = AuditEventBuilder::new(
        AuditCategory::Admin,
        event_type::maintenance::RETENTION_CONFIG_UPDATED,
        AuditOutcome::Success,
    )
    .resource("runtime_retention")
    .resource_ref("config")
    .with_details(serde_json::json!({
        "previous": previous,
        "updated": updated,
    }));

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

    #[test]
    fn cache_retention_validation_accepts_defaults() {
        assert!(validate_retention_config(&RetentionConfig::default()).is_ok());
    }

    #[test]
    fn cache_retention_validation_rejects_unbounded_batches() {
        let mut config = RetentionConfig::default();
        config.cache_retention.max_batches_per_generation = 0;
        assert!(matches!(
            validate_retention_config(&config),
            Err(ApiError::BadRequest(message))
                if message.contains("max_batches_per_generation")
        ));
    }
}
