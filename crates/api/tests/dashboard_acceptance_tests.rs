use attune_api::authz::AuthorizationService;
use attune_api::dashboard_data::contracts::FreshnessMode;
use attune_api::dashboard_data::watermark::{
    merge_bucket_rows_deterministic, BucketCountRow, TimeRange, WatermarkCutoverPlan,
};
use attune_common::models::{DashboardScopeType, DashboardVisibility};
use attune_common::repositories::dashboard::{
    CreateDashboardInput, DashboardRepository, UpdateDashboardInput,
};
use attune_common::repositories::identity::{
    CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
    PermissionAssignmentRepository, PermissionSetRepository,
};
use attune_common::repositories::Create;
use axum::http::StatusCode;
use chrono::TimeZone;
use chrono::{Duration, Utc};
use helpers::{Result, TestContext};
use serde_json::{json, Value};
use sqlx::types::Json;

mod dashboard_acceptance_fixtures;
mod helpers;

fn assert_time_series_sorted(data: &Value, field: &str) {
    let rows = data[field]
        .as_array()
        .expect("time-series field must be an array");

    let mut previous: Option<(String, Option<String>)> = None;
    for row in rows {
        let current = (
            row["bucket"].as_str().unwrap_or_default().to_string(),
            row["label"].as_str().map(str::to_string),
        );
        if let Some(prev) = &previous {
            assert!(
                prev <= &current,
                "{field} rows must be sorted by bucket then label"
            );
        }
        previous = Some(current);
    }
}

async fn register_user_with_grants(
    ctx: &TestContext,
    login_prefix: &str,
    grants: Value,
) -> Result<String> {
    let login = format!("{}_{}", login_prefix, uuid::Uuid::new_v4().simple());
    let response = ctx
        .post(
            "/auth/register",
            json!({
                "login": login,
                "password": "TestPassword123!",
                "display_name": format!("Dashboard User {}", login),
            }),
            None,
        )
        .await?;
    assert!(
        response.status() == StatusCode::OK || response.status() == StatusCode::CREATED,
        "expected 200/201 from /auth/register, got {}",
        response.status()
    );
    let body: Value = response.json().await?;
    let token = body["data"]["access_token"]
        .as_str()
        .expect("missing access token")
        .to_string();

    let identity = IdentityRepository::find_by_login(&ctx.pool, &login)
        .await?
        .expect("registered identity should exist");
    let permset = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("test.dashboard_{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: Some("Dashboard test grants".to_string()),
            description: None,
            grants,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permset.id,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    AuthorizationService::invalidate_permission_set_caches().await;

    Ok(token)
}

async fn create_dashboard(
    ctx: &TestContext,
    dashboard_ref: &str,
    label: &str,
    spec: Value,
) -> Result<attune_common::models::dashboard::Dashboard> {
    create_dashboard_with_scope(
        ctx,
        dashboard_ref,
        label,
        spec,
        DashboardScopeType::Global,
        "global",
        None,
    )
    .await
}

async fn create_dashboard_with_scope(
    ctx: &TestContext,
    dashboard_ref: &str,
    label: &str,
    spec: Value,
    scope_type: DashboardScopeType,
    scope_ref: &str,
    owner_identity: Option<i64>,
) -> Result<attune_common::models::dashboard::Dashboard> {
    assert!(spec.is_object(), "dashboard spec must be object");
    Ok(DashboardRepository::create(
        &ctx.pool,
        CreateDashboardInput {
            r#ref: dashboard_ref.to_string(),
            scope_type,
            scope_ref: scope_ref.to_string(),
            pack: None,
            owner_identity,
            visibility: DashboardVisibility::Public,
            is_adhoc: true,
            label: label.to_string(),
            description: Some("Dashboard acceptance test".to_string()),
            enabled: true,
            is_default_home: false,
            spec_version: 1,
            spec,
            tags: vec!["acceptance".to_string()],
            created_by: None,
        },
    )
    .await?)
}

async fn seed_execution_status(
    ctx: &TestContext,
    entity_id: i64,
    action_ref: &str,
    status: &str,
    at: chrono::DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, old_values, new_values)
        VALUES ($1, 'UPDATE', $2, $3, $4, $5, $6)
        "#,
    )
    .bind(at)
    .bind(entity_id)
    .bind(action_ref)
    .bind(vec!["status"])
    .bind(Json(json!({"status": "running"})))
    .bind(Json(json!({ "status": status })))
    .execute(&ctx.pool)
    .await?;
    Ok(())
}

#[test]
fn fixtures_include_expected_request_shape() {
    let request = dashboard_acceptance_fixtures::sample_dashboard_data_request();
    assert_eq!(request["filters"], json!({}));
    assert_eq!(request["include_meta"], true);
    assert_eq!(request["request_id"], "acceptance-test");
}

#[test]
fn fixtures_enforce_source_meta_and_order_contract_shape() {
    let response = serde_json::json!({
        "data": {
            "sources": [
                {
                    "source_id": "a",
                    "meta": {
                        "authorization_mode": "operator_global",
                        "freshness_mode": "raw_only",
                        "aggregate_watermark": null,
                        "cache_hit": false,
                        "bucket_size": null,
                        "truncated": false,
                        "unit_hints": {},
                        "ordering": [],
                        "authorized_refs": null
                    }
                },
                {
                    "source_id": "b",
                    "meta": {
                        "authorization_mode": "identity_filtered",
                        "freshness_mode": "raw_only_fallback",
                        "aggregate_watermark": null,
                        "cache_hit": true,
                        "bucket_size": "1h",
                        "truncated": false,
                        "unit_hints": {},
                        "ordering": ["bucket_start"],
                        "authorized_refs": {"action_refs": ["core.echo"]}
                    }
                }
            ]
        }
    });

    dashboard_acceptance_fixtures::assert_source_order(&response, &["a", "b"]);
    let sources = response["data"]["sources"]
        .as_array()
        .expect("sources must be an array");
    for source in sources {
        dashboard_acceptance_fixtures::assert_required_source_meta_fields(source);
    }
}

#[tokio::test]
async fn analytics_dashboard_is_deterministic_for_identical_explicit_ranges() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;

    let until = Utc::now() - Duration::hours(1);
    let since = until - Duration::hours(6);
    let path = format!(
        "/api/v1/analytics/dashboard?since={}&until={}",
        since.to_rfc3339(),
        until.to_rfc3339()
    );

    let first = ctx.get(&path, ctx.token()).await?;
    let second = ctx.get(&path, ctx.token()).await?;

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);

    let first_body: Value = first.json().await?;
    let second_body: Value = second.json().await?;

    assert_eq!(
        first_body["data"], second_body["data"],
        "identical requests must return identical payloads"
    );

    let data = &first_body["data"];
    assert_time_series_sorted(data, "execution_throughput");
    assert_time_series_sorted(data, "execution_status");
    assert_time_series_sorted(data, "event_volume");
    assert_time_series_sorted(data, "enforcement_volume");
    assert_time_series_sorted(data, "worker_status");

    Ok(())
}

#[tokio::test]
async fn analytics_dashboard_requires_authentication() -> Result<()> {
    let ctx = TestContext::new().await?;
    let response = ctx.get("/api/v1/analytics/dashboard", None).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn dashboard_ref_resolution_precedence_identity_pack_global() -> Result<()> {
    let ctx = TestContext::new().await?;
    let token = register_user_with_grants(
        &ctx,
        "dashboard_resolve",
        json!([{"resource": "dashboards", "actions": ["read"]}]),
    )
    .await?;
    let me = ctx.get("/auth/me", Some(&token)).await?;
    assert_eq!(me.status(), StatusCode::OK);
    let me_body: Value = me.json().await?;
    let identity_id = me_body["data"]["id"]
        .as_i64()
        .expect("auth/me data.id should be an integer");

    let pack_only_ref = format!("core.packscope_{}", uuid::Uuid::new_v4().simple());
    create_dashboard_with_scope(
        &ctx,
        &pack_only_ref,
        "Pack Scope",
        dashboard_acceptance_fixtures::dashboard_spec(
            &[("execution_source", "execution_count")],
            &[("card_execution", "execution_source")],
            None,
        ),
        DashboardScopeType::Pack,
        "core",
        None,
    )
    .await?;
    create_dashboard(
        &ctx,
        &pack_only_ref,
        "Global Scope",
        dashboard_acceptance_fixtures::dashboard_spec(
            &[("execution_source", "execution_count")],
            &[("card_execution", "execution_source")],
            None,
        ),
    )
    .await?;

    let pack_only_response = ctx
        .get(
            &format!("/api/v1/dashboards/{}", pack_only_ref),
            Some(&token),
        )
        .await?;
    assert_eq!(pack_only_response.status(), StatusCode::OK);
    let pack_only_body: Value = pack_only_response.json().await?;
    assert_eq!(pack_only_body["data"]["scope_type"], "pack");
    assert_eq!(pack_only_body["data"]["scope_ref"], "core");

    let full_precedence_ref = format!("core.precedence_{}", uuid::Uuid::new_v4().simple());
    create_dashboard_with_scope(
        &ctx,
        &full_precedence_ref,
        "Identity Scope",
        dashboard_acceptance_fixtures::dashboard_spec(
            &[("execution_source", "execution_count")],
            &[("card_execution", "execution_source")],
            None,
        ),
        DashboardScopeType::Identity,
        &identity_id.to_string(),
        Some(identity_id),
    )
    .await?;
    create_dashboard_with_scope(
        &ctx,
        &full_precedence_ref,
        "Pack Scope",
        dashboard_acceptance_fixtures::dashboard_spec(
            &[("execution_source", "execution_count")],
            &[("card_execution", "execution_source")],
            None,
        ),
        DashboardScopeType::Pack,
        "core",
        None,
    )
    .await?;
    create_dashboard(
        &ctx,
        &full_precedence_ref,
        "Global Scope",
        dashboard_acceptance_fixtures::dashboard_spec(
            &[("execution_source", "execution_count")],
            &[("card_execution", "execution_source")],
            None,
        ),
    )
    .await?;

    let full_precedence_response = ctx
        .get(
            &format!("/api/v1/dashboards/{}", full_precedence_ref),
            Some(&token),
        )
        .await?;
    assert_eq!(full_precedence_response.status(), StatusCode::OK);
    let full_precedence_body: Value = full_precedence_response.json().await?;
    assert_eq!(full_precedence_body["data"]["scope_type"], "identity");
    assert_eq!(
        full_precedence_body["data"]["scope_ref"],
        identity_id.to_string()
    );

    Ok(())
}

#[tokio::test]
async fn dashboard_partial_failure_contract_mixed_source_statuses() -> Result<()> {
    let ctx = TestContext::new().await?;
    let token = register_user_with_grants(
        &ctx,
        "dashboard_partial",
        json!([
            {"resource": "dashboards", "actions": ["read"]},
            {"resource": "executions", "actions": ["read"], "constraints": {"refs": ["core.allowed_action"]}}
        ]),
    )
    .await?;

    let dashboard_ref = format!("core.partial_{}", uuid::Uuid::new_v4().simple());
    create_dashboard(
        &ctx,
        &dashboard_ref,
        "Partial Contract",
        dashboard_acceptance_fixtures::dashboard_spec(
            &[
                ("a_forbidden", "queue_backlog"),
                ("c_ok", "execution_count"),
                ("b_invalid", "unknown_source"),
            ],
            &[
                ("card_forbidden", "a_forbidden"),
                ("card_ok", "c_ok"),
                ("card_invalid", "b_invalid"),
            ],
            None,
        ),
    )
    .await?;

    let now = Utc::now();
    seed_execution_status(
        &ctx,
        9001,
        "core.allowed_action",
        "completed",
        now - Duration::minutes(30),
    )
    .await?;

    let mut request = dashboard_acceptance_fixtures::sample_dashboard_data_request();
    request["source_ids"] = json!(["c_ok", "b_invalid", "a_forbidden"]);
    request["time_range"] = json!({
        "start": (now - Duration::hours(2)).to_rfc3339(),
        "end": now.to_rfc3339()
    });

    let response = ctx
        .post(
            &format!("/api/v1/dashboards/{}/data", dashboard_ref),
            request,
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;

    assert_eq!(body["partial"], true);
    dashboard_acceptance_fixtures::assert_source_order(
        &json!({ "data": { "sources": body["sources"] }}),
        &["a_forbidden", "b_invalid", "c_ok"],
    );

    let forbidden = dashboard_acceptance_fixtures::source_by_id(&body, "a_forbidden");
    let invalid = dashboard_acceptance_fixtures::source_by_id(&body, "b_invalid");
    let ok = dashboard_acceptance_fixtures::source_by_id(&body, "c_ok");
    assert_eq!(forbidden["status"], "forbidden");
    assert_eq!(invalid["status"], "invalid");
    assert_eq!(ok["status"], "ok");
    assert_eq!(ok["meta"]["authorization_mode"], "identity_filtered");
    assert_eq!(
        ok["meta"]["authorized_refs"]["action_refs"],
        json!(["core.allowed_action"])
    );
    for source in body["sources"].as_array().expect("sources array") {
        dashboard_acceptance_fixtures::assert_required_source_meta_fields(source);
    }
    Ok(())
}

#[tokio::test]
async fn dashboard_scope_rbac_isolation_and_cache_context_partitioning() -> Result<()> {
    let ctx = TestContext::new().await?;
    let token = register_user_with_grants(
        &ctx,
        "dashboard_scope",
        json!([
            {"resource": "dashboards", "actions": ["read"]},
            {"resource": "executions", "actions": ["read"], "constraints": {"pack_refs": ["core"], "refs": ["core.allowed_action"]}}
        ]),
    )
    .await?;

    let dashboard_ref = format!("core.scope_{}", uuid::Uuid::new_v4().simple());
    create_dashboard(
        &ctx,
        &dashboard_ref,
        "Scope/RBAC Contract",
        dashboard_acceptance_fixtures::dashboard_spec(
            &[("execution_source", "execution_count")],
            &[("card_execution", "execution_source")],
            Some(json!([{ "id": "action_ref", "type": "action_ref" }])),
        ),
    )
    .await?;

    let now = Utc::now();
    seed_execution_status(
        &ctx,
        9101,
        "core.allowed_action",
        "completed",
        now - Duration::minutes(40),
    )
    .await?;
    seed_execution_status(
        &ctx,
        9102,
        "core.blocked_action",
        "completed",
        now - Duration::minutes(35),
    )
    .await?;

    let mut base_request = dashboard_acceptance_fixtures::sample_dashboard_data_request();
    base_request["source_ids"] = json!(["execution_source"]);
    base_request["time_range"] = json!({
        "start": (now - Duration::hours(2)).to_rfc3339(),
        "end": now.to_rfc3339()
    });

    let allowed_response = ctx
        .post(
            &format!("/api/v1/dashboards/{}/data", dashboard_ref),
            base_request.clone(),
            Some(&token),
        )
        .await?;
    assert_eq!(allowed_response.status(), StatusCode::OK);
    let allowed_body: Value = allowed_response.json().await?;
    let source = dashboard_acceptance_fixtures::source_by_id(&allowed_body, "execution_source");
    assert_eq!(source["meta"]["authorization_mode"], "identity_filtered");
    assert_eq!(
        source["meta"]["authorized_refs"],
        json!({
            "pack_refs": ["core"],
            "action_refs": ["core.allowed_action"]
        })
    );
    let series = source["data"]
        .as_array()
        .expect("execution data array")
        .iter()
        .map(|row| row["series"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert!(
        series.iter().all(|entry| entry == "core.allowed_action"),
        "response must not leak blocked action refs: {series:?}"
    );

    let mut denied_filter_request = base_request.clone();
    denied_filter_request["filters"] = json!({"action_ref": "core.blocked_action"});
    let denied_response = ctx
        .post(
            &format!("/api/v1/dashboards/{}/data", dashboard_ref),
            denied_filter_request,
            Some(&token),
        )
        .await?;
    assert_eq!(denied_response.status(), StatusCode::OK);
    let denied_body: Value = denied_response.json().await?;
    let denied_source =
        dashboard_acceptance_fixtures::source_by_id(&denied_body, "execution_source");
    assert_eq!(denied_source["status"], "empty");
    assert_eq!(
        denied_source["meta"]["authorized_refs"],
        json!({
            "pack_refs": ["core"],
            "action_refs": []
        })
    );

    Ok(())
}

#[tokio::test]
async fn dashboard_source_params_enforce_effective_scope_intersection() -> Result<()> {
    let ctx = TestContext::new().await?;
    let token = register_user_with_grants(
        &ctx,
        "dashboard_source_params",
        json!([
            {"resource": "dashboards", "actions": ["read"]},
            {"resource": "executions", "actions": ["read"], "constraints": {"pack_refs": ["core"], "refs": ["core.allowed_action", "core.other_action"]}}
        ]),
    )
    .await?;

    let dashboard_ref = format!("core.source_params_{}", uuid::Uuid::new_v4().simple());
    create_dashboard(
        &ctx,
        &dashboard_ref,
        "Source Params Scope",
        json!({
            "layout": {
                "breakpoints": {
                    "lg": { "min_width": 1280, "columns": 12 },
                    "sm": { "min_width": 0, "columns": 4 }
                }
            },
            "filters": [
                { "id": "action_ref", "type": "action_ref" }
            ],
            "data_sources": {
                "execution_source": {
                    "type": "execution_count",
                    "params": {
                        "action_refs": ["core.allowed_action"]
                    }
                }
            },
            "cards": [
                {
                    "id": "card_execution",
                    "source": "execution_source",
                    "position": {
                        "lg": { "x": 0, "y": 0, "w": 6, "h": 4 },
                        "sm": { "x": 0, "y": 0, "w": 4, "h": 4 }
                    }
                }
            ]
        }),
    )
    .await?;

    let now = Utc::now();
    seed_execution_status(
        &ctx,
        9201,
        "core.allowed_action",
        "completed",
        now - Duration::minutes(20),
    )
    .await?;
    seed_execution_status(
        &ctx,
        9202,
        "core.other_action",
        "completed",
        now - Duration::minutes(15),
    )
    .await?;

    let mut base_request = dashboard_acceptance_fixtures::sample_dashboard_data_request();
    base_request["source_ids"] = json!(["execution_source"]);
    base_request["time_range"] = json!({
        "start": (now - Duration::hours(2)).to_rfc3339(),
        "end": now.to_rfc3339()
    });

    let scoped_response = ctx
        .post(
            &format!("/api/v1/dashboards/{}/data", dashboard_ref),
            base_request.clone(),
            Some(&token),
        )
        .await?;
    assert_eq!(scoped_response.status(), StatusCode::OK);
    let scoped_body: Value = scoped_response.json().await?;
    let scoped_source =
        dashboard_acceptance_fixtures::source_by_id(&scoped_body, "execution_source");
    assert_eq!(
        scoped_source["meta"]["authorized_refs"],
        json!({
            "pack_refs": ["core"],
            "action_refs": ["core.allowed_action"]
        })
    );
    let scoped_series = scoped_source["data"]
        .as_array()
        .expect("execution data array")
        .iter()
        .map(|row| row["series"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert!(
        scoped_series
            .iter()
            .all(|entry| entry == "core.allowed_action"),
        "source params must constrain response to declared refs: {scoped_series:?}"
    );

    let mut filtered_request = base_request.clone();
    filtered_request["filters"] = json!({"action_ref": "core.other_action"});
    let filtered_response = ctx
        .post(
            &format!("/api/v1/dashboards/{}/data", dashboard_ref),
            filtered_request,
            Some(&token),
        )
        .await?;
    assert_eq!(filtered_response.status(), StatusCode::OK);
    let filtered_body: Value = filtered_response.json().await?;
    let filtered_source =
        dashboard_acceptance_fixtures::source_by_id(&filtered_body, "execution_source");
    assert_eq!(filtered_source["status"], "empty");
    assert_eq!(
        filtered_source["meta"]["authorized_refs"],
        json!({
            "pack_refs": ["core"],
            "action_refs": []
        })
    );

    Ok(())
}

#[tokio::test]
async fn dashboard_source_order_contract_is_deterministic() -> Result<()> {
    let ctx = TestContext::new().await?;
    let token = register_user_with_grants(
        &ctx,
        "dashboard_order",
        json!([
            {"resource": "dashboards", "actions": ["read"]},
            {"resource": "queue_items", "actions": ["read"]}
        ]),
    )
    .await?;

    let dashboard_ref = format!("core.order_{}", uuid::Uuid::new_v4().simple());
    create_dashboard(
        &ctx,
        &dashboard_ref,
        "Source Order Contract",
        dashboard_acceptance_fixtures::dashboard_spec(
            &[
                ("z_source", "queue_backlog"),
                ("a_source", "queue_backlog"),
                ("m_source", "queue_backlog"),
            ],
            &[
                ("card_z", "z_source"),
                ("card_a", "a_source"),
                ("card_m", "m_source"),
            ],
            None,
        ),
    )
    .await?;

    let now = Utc::now();
    let mut request = dashboard_acceptance_fixtures::sample_dashboard_data_request();
    request["source_ids"] = json!(["z_source", "a_source", "m_source"]);
    request["card_ids"] = json!(["card_m", "card_z", "card_a"]);
    request["time_range"] = json!({
        "start": (now - Duration::hours(1)).to_rfc3339(),
        "end": now.to_rfc3339()
    });

    let first = ctx
        .post(
            &format!("/api/v1/dashboards/{}/data", dashboard_ref),
            request.clone(),
            Some(&token),
        )
        .await?;
    let second = ctx
        .post(
            &format!("/api/v1/dashboards/{}/data", dashboard_ref),
            request,
            Some(&token),
        )
        .await?;

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let first_body: Value = first.json().await?;
    let second_body: Value = second.json().await?;

    dashboard_acceptance_fixtures::assert_source_order(
        &json!({ "data": { "sources": first_body["sources"] }}),
        &["a_source", "m_source", "z_source"],
    );
    assert_eq!(
        first_body["sources"], second_body["sources"],
        "source envelopes must be deterministic across identical requests"
    );
    Ok(())
}

#[test]
fn dashboard_watermark_cutover_and_boundary_correctness() {
    let start = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
    let watermark = Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 6, 2, 0, 0, 0).unwrap();
    let request_range = TimeRange::new(start, end).expect("valid range");
    let plan = WatermarkCutoverPlan::build(request_range, Some(watermark)).expect("valid cutover");

    assert_eq!(plan.freshness_mode, FreshnessMode::AggregatePlusTail);
    assert_eq!(
        plan.aggregate_range.expect("aggregate range"),
        TimeRange::new(start, watermark).expect("valid aggregate range")
    );
    assert_eq!(
        plan.raw_range.expect("raw range"),
        TimeRange::new(watermark, end).expect("valid raw range")
    );

    let merged = merge_bucket_rows_deterministic(
        &plan,
        &[
            BucketCountRow {
                bucket_start: Utc.with_ymd_and_hms(2026, 6, 1, 11, 0, 0).unwrap(),
                series: "all".to_string(),
                count: 11,
            },
            BucketCountRow {
                bucket_start: watermark,
                series: "all".to_string(),
                count: 99,
            },
        ],
        &[
            BucketCountRow {
                bucket_start: watermark,
                series: "all".to_string(),
                count: 12,
            },
            BucketCountRow {
                bucket_start: Utc.with_ymd_and_hms(2026, 6, 1, 13, 0, 0).unwrap(),
                series: "all".to_string(),
                count: 13,
            },
        ],
    );

    assert_eq!(merged.len(), 3);
    assert_eq!(merged[0].count, 11);
    assert_eq!(merged[1].bucket_start, watermark);
    assert_eq!(merged[1].count, 12);
    assert_eq!(merged[2].count, 13);
}

#[test]
fn dashboard_watermark_missing_falls_back_to_raw_only_fallback() {
    let start = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 6, 2, 0, 0, 0).unwrap();
    let request_range = TimeRange::new(start, end).expect("valid range");
    let plan = WatermarkCutoverPlan::build(request_range, None).expect("valid fallback plan");

    assert_eq!(plan.freshness_mode, FreshnessMode::RawOnlyFallback);
    assert!(plan.aggregate_range.is_none());
    assert_eq!(plan.raw_range, Some(request_range));
    assert!(plan.aggregate_watermark.is_none());
}

#[tokio::test]
#[ignore = "blocked: timezone-aware dashboard bucketing endpoint not implemented yet"]
async fn dashboard_timezone_bucketing_handles_dst_and_non_hour_offsets() -> Result<()> {
    let _ctx = TestContext::new().await?.with_auth().await?;
    // Target behavior:
    // - UTC, DST zones, and non-hour-offset zones yield correct local bucketing
    // - repeated DST local hours remain distinct by UTC bucket_start
    Ok(())
}

#[tokio::test]
async fn dashboard_optimistic_concurrency_rejects_stale_updates() -> Result<()> {
    let ctx = TestContext::new().await?;
    let dashboard_ref = format!("core.concurrent_{}", uuid::Uuid::new_v4().simple());
    let dashboard = create_dashboard(
        &ctx,
        &dashboard_ref,
        "Optimistic Concurrency",
        dashboard_acceptance_fixtures::dashboard_spec(
            &[("queue_source", "queue_backlog")],
            &[("queue_card", "queue_source")],
            None,
        ),
    )
    .await?;

    let updated = DashboardRepository::update_with_version(
        &ctx.pool,
        dashboard.id,
        UpdateDashboardInput {
            label: Some("Updated once".to_string()),
            expected_revision: Some(dashboard.revision),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(updated.revision, dashboard.revision + 1);

    let stale_error = DashboardRepository::update_with_version(
        &ctx.pool,
        dashboard.id,
        UpdateDashboardInput {
            label: Some("Stale update".to_string()),
            expected_revision: Some(dashboard.revision),
            ..Default::default()
        },
    )
    .await
    .expect_err("stale revision must be rejected");
    assert!(
        stale_error.to_string().contains("revision mismatch"),
        "expected revision mismatch error, got: {stale_error}"
    );

    let latest = DashboardRepository::update_with_version(
        &ctx.pool,
        dashboard.id,
        UpdateDashboardInput {
            label: Some("Updated twice".to_string()),
            expected_revision: Some(updated.revision),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(latest.revision, updated.revision + 1);

    Ok(())
}
