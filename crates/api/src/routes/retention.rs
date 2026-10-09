//! Runtime retention configuration API routes.

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use std::sync::Arc;

use attune_common::{
    audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome},
    config::RetentionConfig,
    rbac::{Action, AuthorizationContext, Resource},
    repositories::{
        native_maintenance::{
            partitions::PartitionRepository, schedule::ScheduleRepository,
            summaries::SummaryRepository,
        },
        retention::RetentionRepository,
    },
};

use crate::{
    auth::RequireAuth,
    authz::AuthorizationCheck,
    dto::{native_maintenance::NativeMaintenanceStatus, ApiResponse},
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

fn native_status_error(error: attune_common::error::Error) -> ApiError {
    // Metadata reads inherit bounded maintenance deadlines. Their cancellation
    // is temporary unavailability, not a public PostgreSQL error message.
    if let attune_common::error::Error::Database(ref database_error) = error {
        if database_error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref()
            == Some("57014")
        {
            return ApiError::RetryableDatabaseError;
        }
    }
    error.into()
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

/// Inspect partition coverage, DEFAULT backlog, summary invalidations and job cadences.
#[utoipa::path(
    get,
    path = "/api/v1/retention-config/native-status",
    tag = "retention",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Native maintenance observations", body = ApiResponse<NativeMaintenanceStatus>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 503, description = "Native maintenance observations temporarily unavailable"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_native_maintenance_status(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
) -> ApiResult<impl IntoResponse> {
    state
        .authorization_service()
        .authorize(&user.0, retention_check(Action::Read))
        .await?;
    let config = RetentionRepository::load_config(&state.db)
        .await
        .map_err(native_status_error)?;
    let observed_at = chrono::Utc::now();
    let partitions =
        PartitionRepository::status(&state.db, &config.native_maintenance, observed_at)
            .await
            .map_err(native_status_error)?;
    let summaries = SummaryRepository::status(&state.db)
        .await
        .map_err(native_status_error)?;
    let schedule = ScheduleRepository::status(&state.db)
        .await
        .map_err(native_status_error)?;
    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(NativeMaintenanceStatus {
            observed_at,
            enabled: config.native_maintenance.enabled,
            partitions,
            summaries,
            schedule,
        })),
    ))
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
        (status = 422, description = "Malformed retention configuration"),
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
    Router::new()
        .route(
            "/retention-config",
            get(get_retention_config).put(update_retention_config),
        )
        .route(
            "/retention-config/native-status",
            get(get_native_maintenance_status),
        )
}

fn validate_retention_config(config: &RetentionConfig) -> ApiResult<()> {
    config
        .native_maintenance
        .validate()
        .map_err(ApiError::BadRequest)?;
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
    if config.max_batches_per_target <= 0 {
        return Err(ApiError::BadRequest(
            "retention.max_batches_per_target must be greater than zero".to_string(),
        ));
    }
    let cache = &config.cache_retention;
    for (field, value) in [
        ("max_generations_per_cycle", cache.max_generations_per_cycle),
        ("max_namespaces_per_cycle", cache.max_namespaces_per_cycle),
    ] {
        if value <= 0 {
            return Err(ApiError::BadRequest(format!(
                "retention.cache_retention.{field} must be greater than zero"
            )));
        }
    }
    cache
        .validate_storage_maintenance()
        .map_err(|message| ApiError::BadRequest(format!("retention.{message}")))?;
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
    fn retention_validation_rejects_nonpositive_batch_budgets() {
        for budget in [0, -1] {
            let config = RetentionConfig {
                max_batches_per_target: budget,
                ..RetentionConfig::default()
            };
            assert!(matches!(
                validate_retention_config(&config),
                Err(ApiError::BadRequest(message))
                    if message.contains("max_batches_per_target")
            ));
        }
    }

    #[test]
    fn cache_retention_validation_rejects_unbounded_ddl() {
        let mut config = RetentionConfig::default();
        config.cache_retention.ddl_statement_timeout_milliseconds = 0;
        assert!(matches!(
            validate_retention_config(&config),
            Err(ApiError::BadRequest(message))
                if message.contains("ddl_statement_timeout_milliseconds")
        ));
    }

    #[test]
    fn cache_statistics_deadline_and_cadence_have_independent_bounds() {
        let mut config = RetentionConfig::default();
        for interval in [0, 86_401] {
            config.cache_retention.statistics_interval_seconds = interval;
            assert!(
                matches!(validate_retention_config(&config), Err(ApiError::BadRequest(message)) if message.contains("statistics_interval_seconds"))
            );
        }
        config.cache_retention.statistics_interval_seconds = 86_400;
        for deadline in [0, 3_600_001] {
            config
                .cache_retention
                .statistics_statement_timeout_milliseconds = deadline;
            assert!(
                matches!(validate_retention_config(&config), Err(ApiError::BadRequest(message)) if message.contains("statistics_statement_timeout_milliseconds"))
            );
        }
        config
            .cache_retention
            .statistics_statement_timeout_milliseconds = 3_600_000;
        assert!(validate_retention_config(&config).is_ok());
        assert_eq!(
            config.cache_retention.ddl_statement_timeout_milliseconds,
            1_000
        );
        assert_eq!(
            config
                .cache_retention
                .ddl_creation_statement_timeout_milliseconds,
            5_000
        );
    }

    #[test]
    fn cache_creation_deadline_is_validated_independently_of_cleanup() {
        let mut config = RetentionConfig::default();
        config
            .cache_retention
            .ddl_creation_statement_timeout_milliseconds = 7_000;
        assert!(validate_retention_config(&config).is_ok());
        assert_eq!(
            config.cache_retention.ddl_statement_timeout_milliseconds,
            1_000
        );
        for deadline in [0, 3_600_001] {
            config
                .cache_retention
                .ddl_creation_statement_timeout_milliseconds = deadline;
            assert!(matches!(
                validate_retention_config(&config),
                Err(ApiError::BadRequest(message))
                    if message.contains("ddl_creation_statement_timeout_milliseconds")
            ));
        }
    }
}
