//! Protected native-maintenance contracts against run-owned PostgreSQL clones.

mod helpers;

use attune_api::authz::AuthorizationService;
use attune_common::{
    config::{NativeMaintenanceConfig, RetentionConfig},
    repositories::{
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        native_maintenance::{
            partitions::PartitionRepository, schedule::ScheduleRepository,
            summaries::SummaryRepository, SummaryKind,
        },
        retention::RetentionRepository,
        Create,
    },
};
use axum::http::StatusCode;
use chrono::{Duration, Timelike, Utc};
use helpers::{Result, TestContext};
use serde_json::{json, Value};

async fn grant(ctx: &TestContext, actions: &[&str]) -> Result<()> {
    let identity = ctx.user.as_ref().expect("authenticated fixture").id;
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("test.native_{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{ "resource": "retention", "actions": actions }]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(identity).await;
    AuthorizationService::invalidate_permission_set_caches().await;
    Ok(())
}

#[tokio::test]
async fn native_status_requires_authentication_and_retention_read() -> Result<()> {
    let mut ctx = TestContext::new().await?;
    let response = ctx
        .get("/api/v1/retention-config/native-status", None)
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    ctx = ctx.with_auth().await?;
    let response = ctx
        .get("/api/v1/retention-config/native-status", ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    grant(&ctx, &["update"]).await?;
    let response = ctx
        .get("/api/v1/retention-config/native-status", ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    grant(&ctx, &["read"]).await?;
    let response = ctx
        .get("/api/v1/retention-config/native-status", ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response = ctx
        .put(
            "/api/v1/retention-config",
            RetentionConfig::default(),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    ctx.cleanup().await
}

async fn assert_native_status_busy_is_retryable(
    operation_timeout_milliseconds: u64,
    lock_timeout_milliseconds: u64,
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let result: Result<_> = async {
        grant(&ctx, &["read", "update"]).await?;
        let config = RetentionConfig {
            native_maintenance: NativeMaintenanceConfig {
                operation_timeout_milliseconds,
                lock_timeout_milliseconds,
                ..Default::default()
            },
            ..Default::default()
        };
        let update = ctx
            .put("/api/v1/retention-config", &config, ctx.token())
            .await?;
        if update.status() != StatusCode::OK {
            return Err("could not persist the owned status deadline fixture".into());
        }
        // Prime type discovery before acquiring the blocker. Cancellation must
        // come from the actual status read, not cold connection setup.
        PartitionRepository::status(&ctx.pool, &NativeMaintenanceConfig::default(), Utc::now())
            .await?;
        let mut blocker = ctx.pool.begin().await?;
        sqlx::query("LOCK TABLE ONLY event IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *blocker)
            .await?;
        let blocked = ctx
            .get("/api/v1/retention-config/native-status", ctx.token())
            .await;
        let released = blocker.rollback().await;
        let blocked = blocked?;
        released?;
        let blocked_status = blocked.status();
        let blocked_body: Value = blocked.json().await?;
        let recovered = ctx
            .get("/api/v1/retention-config/native-status", ctx.token())
            .await?;
        let recovered_status = recovered.status();
        let recovered_body: Value = recovered.json().await?;
        let persisted = RetentionRepository::load_config(&ctx.pool).await?;
        Ok((
            blocked_status,
            blocked_body,
            recovered_status,
            recovered_body,
            persisted,
            config,
        ))
    }
    .await;
    // The real audit writer, pool and owned database stop before assertions,
    // including the deliberately red pre-fix run.
    ctx.cleanup().await?;
    let (blocked_status, blocked_body, recovered_status, recovered_body, persisted, config) =
        result?;
    assert_eq!(
        blocked_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{blocked_body}"
    );
    assert_eq!(
        blocked_body,
        json!({
            "error": "Database operation temporarily unavailable; please retry",
            "code": "RETRYABLE_DATABASE_ERROR"
        })
    );
    assert_eq!(recovered_status, StatusCode::OK, "{recovered_body}");
    assert_eq!(
        recovered_body["data"]["partitions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(persisted, config, "status reads must not change deadlines");
    Ok(())
}

#[tokio::test]
async fn native_status_statement_timeout_is_retryable_and_recovers() -> Result<()> {
    // The statement deadline expires before the lock deadline, deterministically
    // producing PostgreSQL query_canceled (57014) without sleeps or CPU pressure.
    assert_native_status_busy_is_retryable(500, 5000).await
}

#[tokio::test]
async fn native_status_lock_timeout_is_retryable_and_recovers() -> Result<()> {
    assert_native_status_busy_is_retryable(5000, 100).await
}

#[tokio::test]
async fn native_update_rejects_zero_negative_and_malformed_limits_without_persistence() -> Result<()>
{
    let ctx = TestContext::new().await?.with_auth().await?;
    grant(&ctx, &["read"]).await?;
    let response = ctx
        .put(
            "/api/v1/retention-config",
            RetentionConfig::default(),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    grant(&ctx, &["update"]).await?;
    RetentionRepository::ensure_config(&ctx.pool).await?;
    let before = RetentionRepository::load_config(&ctx.pool).await?;
    let native = serde_json::to_value(&before.native_maintenance)?;
    for field in native
        .as_object()
        .expect("config object")
        .keys()
        .filter(|key| *key != "enabled")
    {
        let mut request = serde_json::to_value(&before)?;
        request["native_maintenance"][field] = json!(0);
        let response = ctx
            .put("/api/v1/retention-config", request, ctx.token())
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{field}");
        let body: Value = response.json().await?;
        assert!(body["error"]
            .as_str()
            .expect("validation error")
            .contains(field));
    }
    for value in [json!(-1), json!(1.5), json!("invalid"), Value::Null] {
        let mut request = serde_json::to_value(&before)?;
        request["native_maintenance"]["partition_lookahead_days"] = value;
        let response = ctx
            .put("/api/v1/retention-config", request, ctx.token())
            .await?;
        assert!(matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ));
    }
    let mut request = serde_json::to_value(&before)?;
    request["native_maintenance"]["operation_timeout_milliseconds"] = json!(2147483648_u64);
    assert_eq!(
        ctx.put("/api/v1/retention-config", request, ctx.token())
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(RetentionRepository::load_config(&ctx.pool).await?, before);
    ctx.cleanup().await
}

#[tokio::test]
async fn native_config_roundtrips_and_status_reports_real_backlog_coverage_and_cadences(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    grant(&ctx, &["read", "update"]).await?;
    let native = NativeMaintenanceConfig {
        enabled: false,
        partition_interval_seconds: 1234,
        summary_interval_seconds: 123,
        partition_lookahead_days: 10,
        max_partition_operations_per_cycle: 9,
        default_repair_row_limit: 1,
        lock_timeout_milliseconds: 321,
        operation_timeout_milliseconds: 1500,
        max_partition_cycle_milliseconds: 7890,
        max_summary_buckets_per_cycle: 17,
        max_summary_invalidations_per_bucket: 45,
        summary_bootstrap_hours: 12,
        max_summary_cycle_milliseconds: 6789,
    };
    let config = RetentionConfig {
        native_maintenance: native.clone(),
        ..Default::default()
    };
    let response = ctx
        .put("/api/v1/retention-config", &config, ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(
        serde_json::from_value::<RetentionConfig>(body["data"].clone())?,
        config
    );
    let body: Value = ctx
        .get("/api/v1/retention-config", ctx.token())
        .await?
        .json()
        .await?;
    assert_eq!(
        serde_json::from_value::<RetentionConfig>(body["data"].clone())?,
        config
    );

    let now = Utc::now();
    let hour = now
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap()
        - Duration::hours(2);
    SummaryRepository::refresh_bucket(&ctx.pool, SummaryKind::EventVolume, hour, &native).await?;
    // Fixture writes through the parent, exercising statement invalidation and DEFAULT routing.
    let old_day = now - Duration::days(20);
    sqlx::query(
        "INSERT INTO event (created, trigger_ref) VALUES ($1, 'test.native'), ($1, 'test.native')",
    )
    .bind(old_day)
    .execute(&ctx.pool)
    .await?;
    ScheduleRepository::ensure(&ctx.pool, now).await?;
    let expected_partitions =
        serde_json::to_value(PartitionRepository::status(&ctx.pool, &native, now).await?)?;
    let expected_summaries = serde_json::to_value(SummaryRepository::status(&ctx.pool).await?)?;
    let expected_schedule = serde_json::to_value(ScheduleRepository::status(&ctx.pool).await?)?;
    let response = ctx
        .get("/api/v1/retention-config/native-status", ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["enabled"], false);
    assert!(body["data"]["observed_at"].as_str().is_some());
    assert_eq!(body["data"]["partitions"], expected_partitions);
    assert_eq!(body["data"]["summaries"], expected_summaries);
    assert_eq!(body["data"]["schedule"], expected_schedule);
    let event = body["data"]["partitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["parent"] == "event")
        .unwrap();
    assert_eq!(event["default_rows_at_least"], 2);
    assert_eq!(event["default_count_exact"], false);
    assert!(event["missing_future_partitions"].as_i64().unwrap() > 0);
    let summary = body["data"]["summaries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "event_volume")
        .unwrap();
    assert_eq!(summary["coverage_hours"], 1);
    assert_eq!(summary["dirty_hours"], 1);
    assert_eq!(summary["dirty_notifications"], 1);
    ctx.cleanup().await
}
