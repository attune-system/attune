//! Integration tests for the owner-scoped cache API.
//!
//! These exercise the deployed router against a schema-per-test PostgreSQL
//! database: refresh lifecycle, generation-pinned reads, RBAC visibility,
//! fail-closed token handling, cursor integrity, conflict/precondition, and
//! quota behavior.

use axum::http::StatusCode;
use helpers::*;
use serde_json::{json, Value};

use attune_common::{
    audit::{AuditCategory, AuditEventFilters, AuditOutcome, AuditRepository},
    auth::jwt::{
        generate_sensor_token, generate_token, generate_worker_token_with_instance, JwtConfig,
        TokenType,
    },
    config::CacheAdmissionConfig,
    models::{enums::WorkerStatus, enums::WorkerType, ActionReferenceVisibility},
    repositories::{
        cache::{
            CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope,
            CreateCacheNamespaceInput, ManagedCacheNamespaceDefinition,
        },
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository, UpdateIdentityInput,
        },
        runtime::{CreateRuntimeInput, CreateWorkerInput, RuntimeRepository, WorkerRepository},
        sensor_workload::{
            AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadRepository,
        },
        trigger::{CreateSensorInput, CreateTriggerInput, SensorRepository, TriggerRepository},
        Create, Update,
    },
};

mod helpers;

fn test_jwt_config() -> JwtConfig {
    JwtConfig {
        secret: "test-secret-for-testing-only-not-secure".to_string(),
        access_token_expiration: 300,
        refresh_token_expiration: 3600,
    }
}

/// Registers a user and assigns a permission set carrying `grants`.
async fn register_user(ctx: &TestContext, login: &str, grants: Value) -> Result<(String, i64)> {
    let response = ctx
        .post(
            "/auth/register",
            json!({
                "login": login,
                "password": "TestPassword123!",
                "display_name": format!("Cache User {login}"),
            }),
            None,
        )
        .await?;
    assert!(
        response.status() == StatusCode::OK || response.status() == StatusCode::CREATED,
        "register failed: {}",
        response.status()
    );
    let body: Value = response.json().await?;
    let token = body["data"]["access_token"]
        .as_str()
        .expect("access token")
        .to_string();

    let identity = IdentityRepository::find_by_login(&ctx.pool, login)
        .await?
        .expect("identity exists");

    let permset = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("test.cache_{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: Some("Cache grants".to_string()),
            description: Some("Cache test grants".to_string()),
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
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;

    Ok((token, identity.id))
}

fn pack_writer_grants(pack_ref: &str) -> Value {
    json!([{
        "resource": "caches",
        "actions": ["read", "create", "update", "delete"],
        "constraints": { "owner_types": ["pack"], "owner_refs": [pack_ref] }
    }])
}

async fn set_identity_attributes(
    ctx: &TestContext,
    identity_id: i64,
    attributes: Value,
) -> Result<()> {
    IdentityRepository::update(
        &ctx.pool,
        identity_id,
        UpdateIdentityInput {
            attributes: Some(serde_json::from_value(attributes)?),
            ..Default::default()
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity_id).await;
    Ok(())
}

async fn begin_generation(
    ctx: &TestContext,
    token: &str,
    pack_ref: &str,
    namespace: &str,
    client_refresh_id: &str,
    expected_chunk_count: i64,
) -> Result<i64> {
    begin_generation_with_expected(
        ctx,
        token,
        pack_ref,
        namespace,
        client_refresh_id,
        expected_chunk_count,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn begin_generation_with_expected(
    ctx: &TestContext,
    token: &str,
    pack_ref: &str,
    namespace: &str,
    client_refresh_id: &str,
    expected_chunk_count: i64,
    expected_active_generation_id: Option<i64>,
) -> Result<i64> {
    let response = ctx
        .post(
            &format!("/api/v1/cache/namespaces/{namespace}/generations"),
            json!({
                "owner_type": "pack",
                "owner_ref": pack_ref,
                "client_refresh_id": client_refresh_id,
                "expected_active_generation_id": expected_active_generation_id,
                "expected_chunk_count": expected_chunk_count,
            }),
            Some(token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED, "begin generation");
    let body: Value = response.json().await?;
    Ok(body["data"]["generation_id"]
        .as_i64()
        .expect("generation id"))
}

async fn upload_chunk(
    ctx: &TestContext,
    token: &str,
    pack_ref: &str,
    namespace: &str,
    generation_id: i64,
    chunk_index: i64,
    entries: Value,
) -> Result<TestResponse> {
    ctx.put(
        &format!(
            "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/chunks/{chunk_index}"
        ),
        json!({ "owner_type": "pack", "owner_ref": pack_ref, "entries": entries }),
        Some(token),
    )
    .await
}

async fn create_namespace(
    ctx: &TestContext,
    token: &str,
    pack_ref: &str,
    namespace: &str,
    extra: Value,
) -> Result<TestResponse> {
    let mut body = json!({
        "owner_type": "pack",
        "owner_ref": pack_ref,
        "namespace": namespace,
    });
    if let (Value::Object(base), Value::Object(more)) = (&mut body, &extra) {
        for (key, value) in more {
            base.insert(key.clone(), value.clone());
        }
    }
    ctx.post("/api/v1/cache/namespaces", body, Some(token))
        .await
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_full_refresh_and_read_lifecycle() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_lifecycle").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_writer_lifecycle",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;

    // Create namespace.
    let response = create_namespace(&ctx, &token, &pack.r#ref, "users", json!({})).await?;
    assert_eq!(response.status(), StatusCode::CREATED);

    // Begin, upload two chunks, seal, promote.
    let generation_id =
        begin_generation(&ctx, &token, &pack.r#ref, "users", "refresh-1", 2).await?;

    let response = upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        generation_id,
        0,
        json!([
            { "external_id": "u1", "value": { "name": "Alice" } },
            { "external_id": "u2", "value": { "name": "Bob" } }
        ]),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK, "chunk 0");

    let response = upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        generation_id,
        1,
        json!([{ "external_id": "u3", "value": { "name": "Carol" } }]),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::OK, "chunk 1");

    let response = ctx
        .post(
            &format!("/api/v1/cache/namespaces/users/generations/{generation_id}/seal"),
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_chunk_count": 2 }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK, "seal");
    let sealed: Value = response.json().await?;
    assert_eq!(sealed["data"]["status"], "ready");
    assert_eq!(sealed["data"]["record_count"], 3);
    ctx.post(
        &format!("/api/v1/cache/namespaces/users/generations/{generation_id}/seal"),
        json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_chunk_count": 2 }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);

    let response = ctx
        .post(
            &format!("/api/v1/cache/namespaces/users/generations/{generation_id}/promote"),
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_active_generation_id": null }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK, "promote");
    let promoted: Value = response.json().await?;
    assert_eq!(promoted["data"]["status"], "active");

    // Exact lookup hit and authorized miss.
    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup",
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "external_id": "u2" }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let lookup: Value = response.json().await?;
    assert_eq!(lookup["data"]["generation_id"], generation_id);
    assert_eq!(lookup["data"]["item"]["value"]["name"], "Bob");

    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup",
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "external_id": "nope" }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let miss: Value = response.json().await?;
    assert!(
        miss["data"]["item"].is_null(),
        "missing id is an authorized null"
    );

    // Multi-ID lookup reports found and missing distinctly.
    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup-many",
            json!({
                "owner_type": "pack",
                "owner_ref": pack.r#ref,
                "external_ids": ["u1", "u3", "ghost"]
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let many: Value = response.json().await?;
    assert_eq!(many["data"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(many["data"]["missing_external_ids"], json!(["ghost"]));

    // Cursor scan returns every id once, in bytewise order, from one generation.
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pinned: Option<i64> = None;
    let mut traversal_expiration: Option<String> = None;
    loop {
        let mut path = format!(
            "/api/v1/cache/namespaces/users/entries?owner_type=pack&owner_ref={}&limit=2",
            pack.r#ref
        );
        if let (Some(gen), Some(cur)) = (pinned, &cursor) {
            path.push_str(&format!("&generation={gen}&cursor={cur}"));
        }
        let response = ctx.get(&path, Some(&token)).await?;
        assert_eq!(response.status(), StatusCode::OK, "scan page");
        let page: Value = response.json().await?;
        let generation = page["data"]["generation_id"].as_i64().unwrap();
        pinned.get_or_insert(generation);
        assert_eq!(
            generation, generation_id,
            "scan is pinned to active generation"
        );
        let page_expiration = page["data"]["cursor_expires_at"]
            .as_str()
            .expect("cursor expiration");
        match traversal_expiration.as_deref() {
            Some(expected) => assert_eq!(
                page_expiration, expected,
                "later pages must preserve the initial traversal deadline"
            ),
            None => traversal_expiration = Some(page_expiration.to_string()),
        }
        for item in page["data"]["items"].as_array().unwrap() {
            seen.push(item["external_id"].as_str().unwrap().to_string());
        }
        match page["data"]["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(seen, vec!["u1", "u2", "u3"], "bytewise order, each id once");
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_point_and_multi_lookup_honor_readable_generation_pins() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_pinned_lookup").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_writer_pinned_lookup",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);

    let first = begin_generation(&ctx, &token, &pack.r#ref, "users", "first", 1).await?;
    upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        first,
        0,
        json!([{ "external_id": "u1", "value": { "revision": "first" } }]),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.post(
        &format!("/api/v1/cache/namespaces/users/generations/{first}/seal"),
        json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_chunk_count": 1 }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.post(
        &format!("/api/v1/cache/namespaces/users/generations/{first}/promote"),
        json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_active_generation_id": null }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);

    let second = begin_generation_with_expected(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        "second",
        1,
        Some(first),
    )
    .await?;
    upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        second,
        0,
        json!([{ "external_id": "u1", "value": { "revision": "second" } }]),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.post(
        &format!("/api/v1/cache/namespaces/users/generations/{second}/seal"),
        json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_chunk_count": 1 }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.post(
        &format!("/api/v1/cache/namespaces/users/generations/{second}/promote"),
        json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_active_generation_id": first }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);

    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup",
            json!({
                "owner_type": "pack",
                "owner_ref": pack.r#ref,
                "external_id": "u1",
                "generation_id": first
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["generation_id"], first);
    assert_eq!(body["data"]["item"]["value"]["revision"], "first");

    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup-many",
            json!({
                "owner_type": "pack",
                "owner_ref": pack.r#ref,
                "external_ids": ["u1", "missing"],
                "generation_id": first
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["items"][0]["value"]["revision"], "first");

    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_reports_not_populated_before_promotion() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_empty").await?;
    let (token, _) =
        register_user(&ctx, "cache_writer_empty", pack_writer_grants(&pack.r#ref)).await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);

    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces/users/entries?owner_type=pack&owner_ref={}&limit=10",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "cache_not_populated");
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn tombstoned_namespace_rejects_refresh_writes_with_specific_code() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_tombstone_code").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_tombstone_writer",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    ctx.delete(
        &format!(
            "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref={}",
            pack.r#ref
        ),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);

    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/generations",
            json!({
                "owner_type": "pack",
                "owner_ref": pack.r#ref,
                "client_refresh_id": "must-fail",
                "expected_active_generation_id": null,
                "expected_chunk_count": 0
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "namespace_deleted");
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_policy_and_page_limits_are_rejected_at_the_api_boundary() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_api_boundaries").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_api_boundaries_writer",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;

    let response = create_namespace(
        &ctx,
        &token,
        &pack.r#ref,
        "invalid-policy",
        json!({"max_retained_generations": 1}),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    let response = ctx
        .put(
            "/api/v1/cache/namespaces/users",
            json!({
                "owner_type": "pack",
                "owner_ref": pack.r#ref,
                "max_retained_generations": 0
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    for path in [
        format!(
            "/api/v1/cache/namespaces?owner_type=pack&owner_ref={}&limit=501",
            pack.r#ref
        ),
        format!(
            "/api/v1/cache/namespaces/users/generations?owner_type=pack&owner_ref={}&limit=0",
            pack.r#ref
        ),
        format!(
            "/api/v1/cache/namespaces/users/entries?owner_type=pack&owner_ref={}&limit=1001",
            pack.r#ref
        ),
    ] {
        let response = ctx.get(&path, Some(&token)).await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "path: {path}");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn pack_managed_namespace_metadata_is_read_only_through_the_api() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_managed_api").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_managed_api_writer",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    let definition_ref = format!("{}.users", pack.r#ref);
    CacheNamespaceRepository::upsert_managed_definitions(
        &ctx.pool,
        pack.id,
        &pack.r#ref,
        &[ManagedCacheNamespaceDefinition {
            definition_ref: definition_ref.clone(),
            owner: CacheOwnerScope::pack(pack.id, Some(pack.r#ref.clone())),
            namespace: "users".to_string(),
            policy: CacheNamespacePolicy::default(),
        }],
        &CacheAdmissionConfig::default(),
    )
    .await?;

    let path = format!(
        "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref={}",
        pack.r#ref
    );
    let response = ctx.get(&path, Some(&token)).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["managed"], true);
    assert_eq!(body["data"]["definition_ref"], definition_ref);
    assert_eq!(body["data"]["managing_pack_ref"], pack.r#ref);

    let response = ctx
        .put(
            "/api/v1/cache/namespaces/users",
            json!({
                "owner_type": "pack",
                "owner_ref": pack.r#ref,
                "freshness_target_seconds": 60
            }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "pack_managed_namespace");

    let response = ctx.delete(&path, Some(&token)).await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "pack_managed_namespace");
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_namespaces_isolate_external_ids() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_isolation").await?;
    let (token, _) =
        register_user(&ctx, "cache_writer_iso", pack_writer_grants(&pack.r#ref)).await?;

    for (namespace, value) in [("users", "user-value"), ("locations", "location-value")] {
        create_namespace(&ctx, &token, &pack.r#ref, namespace, json!({}))
            .await?
            .assert_status(StatusCode::CREATED);
        let generation_id = begin_generation(&ctx, &token, &pack.r#ref, namespace, "r1", 1).await?;
        upload_chunk(
            &ctx,
            &token,
            &pack.r#ref,
            namespace,
            generation_id,
            0,
            json!([{ "external_id": "shared", "value": { "kind": value } }]),
        )
        .await?
        .assert_status(StatusCode::OK);
        ctx.post(
            &format!("/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/seal"),
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_chunk_count": 1 }),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::OK);
        ctx.post(
            &format!("/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/promote"),
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_active_generation_id": null }),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::OK);
    }

    for (namespace, expected) in [("users", "user-value"), ("locations", "location-value")] {
        let response = ctx
            .post(
                &format!("/api/v1/cache/namespaces/{namespace}/entries/lookup"),
                json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "external_id": "shared" }),
                Some(&token),
            )
            .await?;
        let body: Value = response.json().await?;
        assert_eq!(body["data"]["item"]["value"]["kind"], expected);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_rbac_list_and_read_share_visibility() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_rbac").await?;
    let (writer, _) =
        register_user(&ctx, "cache_admin_rbac", pack_writer_grants(&pack.r#ref)).await?;

    for namespace in ["users", "locations"] {
        create_namespace(&ctx, &writer, &pack.r#ref, namespace, json!({}))
            .await?
            .assert_status(StatusCode::CREATED);
    }

    // Reader only authorized for the `users` namespace.
    let reader_grants = json!([{
        "resource": "caches",
        "actions": ["read"],
        "constraints": { "owner_types": ["pack"], "owner_refs": [pack.r#ref], "refs": ["users"] }
    }]);
    let (reader, _) = register_user(&ctx, "cache_reader_rbac", reader_grants).await?;

    // List returns only the readable namespace (same predicate as read).
    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces?owner_type=pack&owner_ref={}",
                pack.r#ref
            ),
            Some(&reader),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    let names: Vec<&str> = body["data"]["namespaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["namespace"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["users"],
        "list is filtered to readable namespaces"
    );

    // Read of the permitted namespace succeeds; the other is forbidden.
    ctx.get(
        &format!(
            "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref={}",
            pack.r#ref
        ),
        Some(&reader),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.get(
        &format!(
            "/api/v1/cache/namespaces/locations?owner_type=pack&owner_ref={}",
            pack.r#ref
        ),
        Some(&reader),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_list_without_owner_returns_every_accessible_scope() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack_a = create_test_pack(&ctx.pool, "cache_browse_pack_a").await?;
    let pack_b = create_test_pack(&ctx.pool, "cache_browse_pack_b").await?;
    let (reader, reader_id) = register_user(
        &ctx,
        "cache_browse_reader",
        json!([{ "resource": "caches", "actions": ["read"] }]),
    )
    .await?;
    let (_, other_identity_id) = register_user(&ctx, "cache_browse_other", json!([])).await?;

    for (owner, namespace) in [
        (CacheOwnerScope::system(), "system_data"),
        (CacheOwnerScope::identity(reader_id), "my_data"),
        (
            CacheOwnerScope::identity(other_identity_id),
            "other_identity_data",
        ),
        (
            CacheOwnerScope::pack(pack_a.id, Some(pack_a.r#ref.clone())),
            "pack_a_data",
        ),
        (
            CacheOwnerScope::pack(pack_b.id, Some(pack_b.r#ref.clone())),
            "pack_b_data",
        ),
    ] {
        CacheNamespaceRepository::create(
            &ctx.pool,
            CreateCacheNamespaceInput {
                owner,
                namespace: namespace.to_string(),
                policy: CacheNamespacePolicy::default(),
            },
        )
        .await?;
    }

    let response = ctx
        .get("/api/v1/cache/namespaces", Some(&reader))
        .await?
        .assert_status(StatusCode::OK);
    let body: Value = response.json().await?;
    let namespaces = body["data"]["namespaces"].as_array().unwrap();
    let visible: std::collections::BTreeSet<_> = namespaces
        .iter()
        .map(|item| {
            (
                item["owner_type"].as_str().unwrap().to_string(),
                item["namespace"].as_str().unwrap().to_string(),
            )
        })
        .collect();

    for expected in [
        ("identity".to_string(), "my_data".to_string()),
        ("pack".to_string(), "pack_a_data".to_string()),
        ("pack".to_string(), "pack_b_data".to_string()),
        ("system".to_string(), "system_data".to_string()),
    ] {
        assert!(visible.contains(&expected), "missing {expected:?}");
    }
    assert!(namespaces
        .iter()
        .all(|item| item["namespace"] != "other_identity_data"));

    ctx.get(
        "/api/v1/cache/namespaces?owner_ref=cache_browse_pack_a",
        Some(&reader),
    )
    .await?
    .assert_status(StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_hidden_namespace_is_not_leaked() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack_a = create_test_pack(&ctx.pool, "pack_a_secret").await?;
    let pack_b = create_test_pack(&ctx.pool, "pack_b_secret").await?;
    let (owner_b, _) =
        register_user(&ctx, "cache_owner_b", pack_writer_grants(&pack_b.r#ref)).await?;
    create_namespace(&ctx, &owner_b, &pack_b.r#ref, "confidential", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);

    // User authorized only for pack A must not learn pack B's namespace exists.
    let (user_a, _) =
        register_user(&ctx, "cache_user_a", pack_writer_grants(&pack_a.r#ref)).await?;

    // Show is authorized before existence: forbidden regardless of existence.
    ctx.get(
        &format!(
            "/api/v1/cache/namespaces/confidential?owner_type=pack&owner_ref={}",
            pack_b.r#ref
        ),
        Some(&user_a),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);

    // Listing pack B yields nothing for user A, so counts don't leak either.
    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces?owner_type=pack&owner_ref={}",
                pack_b.r#ref
            ),
            Some(&user_a),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert!(body["data"]["namespaces"].as_array().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_rejects_worker_refresh_and_unsigned_sensor_tokens() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_tokens").await?;
    let (writer, identity_id) =
        register_user(&ctx, "cache_writer_tokens", pack_writer_grants(&pack.r#ref)).await?;
    create_namespace(&ctx, &writer, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);

    let config = test_jwt_config();
    let path = format!(
        "/api/v1/cache/namespaces/users/entries?owner_type=pack&owner_ref={}&limit=10",
        pack.r#ref
    );

    // Sensor token without a signed workload fence is invalid before cache authorization.
    let sensor = generate_sensor_token(
        identity_id,
        "sensor:core.timer",
        vec!["core.timer".to_string()],
        &config,
        Some(300),
    )
    .expect("sensor token");
    ctx.get(&path, Some(&sensor))
        .await?
        .assert_status(StatusCode::UNAUTHORIZED);

    // Worker token: rejected from cache data routes.
    let worker =
        generate_token(identity_id, "worker", &config, TokenType::Worker).expect("worker token");
    ctx.get(&path, Some(&worker))
        .await?
        .assert_status(StatusCode::FORBIDDEN);

    // Refresh token: rejected at authentication (never valid for API access).
    let refresh =
        generate_token(identity_id, "refresh", &config, TokenType::Refresh).expect("refresh token");
    ctx.get(&path, Some(&refresh))
        .await?
        .assert_status(StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn registered_sensor_tokens_use_exact_signed_read_only_cache_authority() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "sensor_cache_scope").await?;
    let other_pack = create_test_pack(&ctx.pool, "sensor_cache_other").await?;
    let (writer, _) = register_user(
        &ctx,
        "sensor_cache_writer",
        json!([{
            "resource": "caches",
            "actions": ["read", "create", "update", "delete"],
            "constraints": {
                "owner_types": ["pack"],
                "owner_refs": [pack.r#ref, other_pack.r#ref]
            }
        }]),
    )
    .await?;
    create_namespace(&ctx, &writer, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    create_namespace(&ctx, &writer, &other_pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);

    let runtime = RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: format!("{}.python", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            description: None,
            name: "Python".to_string(),
            aliases: vec!["python".to_string()],
            distributions: json!({}),
            installation: None,
            execution_config: json!({}),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await?;
    let sensor_ref = format!("{}.cache_reader", pack.r#ref);
    let sensor = SensorRepository::create(
        &ctx.pool,
        CreateSensorInput {
            r#ref: sensor_ref.clone(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: "Cache reader".to_string(),
            description: None,
            entrypoint: "cache_reader.py".to_string(),
            runtime: runtime.id,
            runtime_ref: runtime.r#ref,
            runtime_version_constraint: None,
            enabled: true,
            param_schema: None,
            config: Some(json!({"cache_permission_set_refs": ["standard"]})),
            worker_selector: json!({}),
            worker_tolerations: json!({}),
            worker_affinity: json!({}),
            log_retention_policy: None,
            log_retention_limit: None,
            artifact_retention_policy: None,
            artifact_retention_limit: None,
        },
    )
    .await?;
    let trigger_ref = format!("{}.cache_probe", pack.r#ref);
    TriggerRepository::create(
        &ctx.pool,
        CreateTriggerInput {
            r#ref: trigger_ref.clone(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: "Cache probe".to_string(),
            description: None,
            enabled: true,
            param_schema: None,
            out_schema: None,
            sensor: Some(sensor.id),
            sensor_ref: Some(sensor_ref.clone()),
            is_adhoc: false,
            reference_visibility: ActionReferenceVisibility::Private,
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await?;

    let worker = WorkerRepository::create(
        &ctx.pool,
        CreateWorkerInput {
            name: format!("{}-cache-test-worker", pack.r#ref),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: Some(json!({})),
            meta: None,
        },
    )
    .await?;
    let worker_instance = uuid::Uuid::new_v4();
    let workload = match SensorWorkloadRepository::acquire_or_renew(
        &ctx.pool,
        AcquireSensorWorkloadInput {
            sensor_id: sensor.id,
            worker_id: worker.id,
            worker_instance,
            lease_seconds: 300,
        },
    )
    .await?
    {
        AcquireSensorWorkloadOutcome::Acquired(workload) => workload,
        AcquireSensorWorkloadOutcome::HeldByOther(_) => panic!("test workload is already held"),
    };
    let worker_token = generate_worker_token_with_instance(
        1,
        &worker.id.to_string(),
        worker_instance,
        &test_jwt_config(),
        None,
    )?;
    let response = ctx
        .post(
            "/auth/internal/sensor-token",
            json!({
                "sensor_ref": sensor_ref,
                "pack_ref": pack.r#ref,
                "trigger_types": [trigger_ref],
                "permission_set_refs": ["standard"],
                "workload_id": workload.workload_id,
                "assignment_generation": workload.generation,
                "worker_instance": worker_instance,
                "ttl_seconds": 3600
            }),
            Some(&worker_token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["pack_ref"], pack.r#ref);
    assert_eq!(body["data"]["permission_set_refs"], json!(["standard"]));
    let sensor_token = body["data"]["token"].as_str().unwrap();

    ctx.get(
        &format!(
            "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref={}",
            pack.r#ref
        ),
        Some(sensor_token),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.get(
        &format!(
            "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref={}",
            other_pack.r#ref
        ),
        Some(sensor_token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    ctx.post(
        "/api/v1/cache/namespaces/users/generations",
        json!({
            "owner_type": "pack",
            "owner_ref": pack.r#ref,
            "client_refresh_id": "sensor-must-not-write",
            "expected_active_generation_id": null,
            "expected_chunk_count": 0
        }),
        Some(sensor_token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);

    ctx.post(
        "/auth/internal/sensor-token",
        json!({
            "sensor_ref": sensor_ref,
            "pack_ref": pack.r#ref,
            "trigger_types": [trigger_ref],
            "permission_set_refs": [],
            "workload_id": workload.workload_id,
            "assignment_generation": workload.generation,
            "worker_instance": worker_instance,
            "ttl_seconds": 3600
        }),
        Some(&worker_token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_chunk_replay_is_idempotent_and_conflicts_on_divergence() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_chunks").await?;
    let (token, _) =
        register_user(&ctx, "cache_writer_chunks", pack_writer_grants(&pack.r#ref)).await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    let generation_id = begin_generation(&ctx, &token, &pack.r#ref, "users", "r1", 1).await?;

    let chunk = json!([{ "external_id": "u1", "value": { "name": "Alice" } }]);
    upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        generation_id,
        0,
        chunk.clone(),
    )
    .await?
    .assert_status(StatusCode::OK);
    // Identical replay: success, no duplicate rows.
    upload_chunk(&ctx, &token, &pack.r#ref, "users", generation_id, 0, chunk)
        .await?
        .assert_status(StatusCode::OK);
    // Divergent payload for the same chunk index: conflict.
    let response = upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        generation_id,
        0,
        json!([{ "external_id": "u1", "value": { "name": "Changed" } }]),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    // Seal reports exactly one record despite the replay.
    let response = ctx
        .post(
            &format!("/api/v1/cache/namespaces/users/generations/{generation_id}/seal"),
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_chunk_count": 1 }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let sealed: Value = response.json().await?;
    assert_eq!(sealed["data"]["record_count"], 1);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_chunk_route_accepts_bounded_payloads_above_axum_default_limit() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_chunk_body_limit").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_chunk_body_writer",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    let generation_id =
        begin_generation(&ctx, &token, &pack.r#ref, "users", "large-body", 1).await?;

    let value = "x".repeat(750_000);
    let entries = (0..3)
        .map(|index| {
            json!({
                "external_id": format!("large-{index}"),
                "value": {"payload": value}
            })
        })
        .collect::<Vec<_>>();
    upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        generation_id,
        0,
        Value::Array(entries),
    )
    .await?
    .assert_status(StatusCode::OK);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_duplicate_external_id_across_chunks_is_rejected() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_dupe").await?;
    let (token, _) =
        register_user(&ctx, "cache_writer_dupe", pack_writer_grants(&pack.r#ref)).await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    let generation_id = begin_generation(&ctx, &token, &pack.r#ref, "users", "r1", 2).await?;

    upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        generation_id,
        0,
        json!([{ "external_id": "u1", "value": { "name": "Alice" } }]),
    )
    .await?
    .assert_status(StatusCode::OK);

    // A later chunk repeating an external id from a prior chunk is a typed,
    // ID-free ingestion conflict, surfaced with a distinct machine code.
    let response = upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        generation_id,
        1,
        json!([{ "external_id": "u1", "value": { "name": "Bob" } }]),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "cache_duplicate_external_id");
    // The error must not leak the offending external identifier.
    let error_text = body["error"].as_str().unwrap_or_default();
    assert!(
        !error_text.contains("u1"),
        "duplicate error must not leak external ids: {error_text}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_promotion_optimistic_conflict() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_promote").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_writer_promote",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);

    let mut ready = Vec::new();
    for refresh in ["r1", "r2"] {
        let generation_id =
            begin_generation(&ctx, &token, &pack.r#ref, "users", refresh, 1).await?;
        upload_chunk(
            &ctx,
            &token,
            &pack.r#ref,
            "users",
            generation_id,
            0,
            json!([{ "external_id": "u1", "value": { "r": refresh } }]),
        )
        .await?
        .assert_status(StatusCode::OK);
        ctx.post(
            &format!("/api/v1/cache/namespaces/users/generations/{generation_id}/seal"),
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_chunk_count": 1 }),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::OK);
        ready.push(generation_id);
    }

    // Omitting the nullable optimistic guard is not equivalent to explicitly
    // asserting an empty namespace.
    ctx.post(
        &format!(
            "/api/v1/cache/namespaces/users/generations/{}/promote",
            ready[0]
        ),
        json!({ "owner_type": "pack", "owner_ref": pack.r#ref }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::BAD_REQUEST);

    // First publication (expected active = null) succeeds.
    ctx.post(
        &format!("/api/v1/cache/namespaces/users/generations/{}/promote", ready[0]),
        json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_active_generation_id": null }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);

    // A transport-level retry of the winning request is idempotent.
    let replay = ctx
        .post(
            &format!(
                "/api/v1/cache/namespaces/users/generations/{}/promote",
                ready[0]
            ),
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_active_generation_id": null }),
            Some(&token),
        )
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay_body: Value = replay.json().await?;
    assert_eq!(replay_body["data"]["status"], "active");

    // Second publisher still assuming null active loses the optimistic race.
    let response = ctx
        .post(
            &format!("/api/v1/cache/namespaces/users/generations/{}/promote", ready[1]),
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_active_generation_id": null }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "cache_precondition_failed");

    // The winner remains active.
    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup",
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "external_id": "u1" }),
            Some(&token),
        )
        .await?;
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["generation_id"], ready[0]);
    assert_eq!(body["data"]["item"]["value"]["r"], "r1");
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_cursor_rejected_across_namespaces() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_cursor").await?;
    let (token, _) =
        register_user(&ctx, "cache_writer_cursor", pack_writer_grants(&pack.r#ref)).await?;

    for namespace in ["users", "locations"] {
        create_namespace(&ctx, &token, &pack.r#ref, namespace, json!({}))
            .await?
            .assert_status(StatusCode::CREATED);
        let generation_id = begin_generation(&ctx, &token, &pack.r#ref, namespace, "r1", 1).await?;
        upload_chunk(
            &ctx,
            &token,
            &pack.r#ref,
            namespace,
            generation_id,
            0,
            json!([
                { "external_id": "a1", "value": {} },
                { "external_id": "a2", "value": {} }
            ]),
        )
        .await?
        .assert_status(StatusCode::OK);
        ctx.post(
            &format!("/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/seal"),
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_chunk_count": 1 }),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::OK);
        ctx.post(
            &format!("/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/promote"),
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_active_generation_id": null }),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::OK);
    }

    // Grab a cursor from `users`.
    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces/users/entries?owner_type=pack&owner_ref={}&limit=1",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    let page: Value = response.json().await?;
    let cursor = page["data"]["next_cursor"]
        .as_str()
        .expect("cursor")
        .to_string();
    let generation = page["data"]["generation_id"].as_i64().unwrap();

    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces/users/entries?owner_type=pack&owner_ref={}&limit=1&cursor={cursor}",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Replaying it against `locations` fails closed.
    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces/locations/entries?owner_type=pack&owner_ref={}&limit=1&generation={generation}&cursor={cursor}",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "cache_cursor_invalid");
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_quota_rejected_before_promotion() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_quota").await?;
    let (token, identity_id) =
        register_user(&ctx, "cache_writer_quota", pack_writer_grants(&pack.r#ref)).await?;
    create_namespace(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        json!({ "max_records_per_generation": 2 }),
    )
    .await?
    .assert_status(StatusCode::CREATED);
    let generation_id = begin_generation(&ctx, &token, &pack.r#ref, "users", "r1", 1).await?;

    let response = upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        generation_id,
        0,
        json!([
            { "external_id": "u1", "value": {} },
            { "external_id": "u2", "value": {} },
            { "external_id": "u3", "value": {} }
        ]),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "cache_quota_exceeded");

    let filters = AuditEventFilters {
        category: Some(AuditCategory::Admin),
        event_type: Some("cache.generation.chunk_uploaded".to_string()),
        outcome: Some(AuditOutcome::Failure),
        actor_identity: Some(identity_id),
        resource_type: Some("cache_generation".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    ctx.flush_audit().await?;
    let audit_events = AuditRepository::search(&ctx.pool, &filters).await?;
    let audit = audit_events
        .first()
        .expect("quota rejection should be audited");
    assert_eq!(audit.details.as_ref().unwrap()["reason"], "quota");
    let audit_text = serde_json::to_string(&audit.details)?;
    for external_id in ["u1", "u2", "u3"] {
        assert!(!audit_text.contains(external_id));
    }

    // The namespace still has no active generation.
    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref={}",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["cache_not_populated"], true);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn successful_chunk_insert_and_replay_emit_redacted_audits() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "chunk_success_audit").await?;
    let (token, identity_id) = register_user(
        &ctx,
        "chunk_success_auditor",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    let generation_id = begin_generation(&ctx, &token, &pack.r#ref, "users", "audit-r1", 1).await?;
    let entries = json!([{
        "external_id": "secret-external-id",
        "value": {"secret": "sensitive-value"}
    }]);
    upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        generation_id,
        0,
        entries.clone(),
    )
    .await?
    .assert_status(StatusCode::OK);
    upload_chunk(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        generation_id,
        0,
        entries,
    )
    .await?
    .assert_status(StatusCode::OK);

    let filters = AuditEventFilters {
        category: Some(AuditCategory::Admin),
        event_type: Some("cache.generation.chunk_uploaded".to_string()),
        outcome: Some(AuditOutcome::Success),
        actor_identity: Some(identity_id),
        resource_type: Some("cache_generation".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    ctx.flush_audit().await?;
    let audit_events = AuditRepository::search(&ctx.pool, &filters).await?;
    assert_eq!(audit_events.len(), 2);
    let mut dispositions: Vec<&str> = audit_events
        .iter()
        .map(|event| {
            event.details.as_ref().unwrap()["disposition"]
                .as_str()
                .unwrap()
        })
        .collect();
    dispositions.sort_unstable();
    assert_eq!(dispositions, ["inserted", "replayed"]);
    for event in audit_events {
        let details = event.details.unwrap();
        assert_eq!(details["generation"].as_i64(), Some(generation_id));
        assert_eq!(details["chunk_index"].as_i64(), Some(0));
        assert_eq!(details["record_count"].as_i64(), Some(1));
        let audit_text = serde_json::to_string(&details)?;
        assert!(!audit_text.contains("secret-external-id"));
        assert!(!audit_text.contains("sensitive-value"));
        assert!(!audit_text.contains("request_checksum"));
    }
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn aggregate_namespace_quota_returns_stable_api_code() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new_with_cache_admission(CacheAdmissionConfig {
        max_live_namespaces: 10,
        max_live_namespaces_per_owner: 1,
        ..CacheAdmissionConfig::default()
    })
    .await?;
    let pack = create_test_pack(&ctx.pool, "aggregate_namespace_quota").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_writer_aggregate_namespace",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    create_namespace(&ctx, &token, &pack.r#ref, "first", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);

    let response = create_namespace(&ctx, &token, &pack.r#ref, "second", json!({})).await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "cache_owner_namespace_limit_exceeded");
    assert_eq!(body["error"], "cache owner live namespace limit exceeded");
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn api_namespace_recreate_still_conflicts_while_tombstone_drains() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_tombstone_recreate").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_tombstone_recreate_writer",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    ctx.delete(
        &format!(
            "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref={}",
            pack.r#ref
        ),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);

    let response = create_namespace(&ctx, &token, &pack.r#ref, "users", json!({})).await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "cache_conflict");
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_zero_record_snapshot_is_an_empty_dataset_not_unpopulated() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "salesforce_empty_snapshot").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_writer_empty_snapshot",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);

    // Publish an authoritative zero-record generation (no chunks).
    let generation_id = begin_generation(&ctx, &token, &pack.r#ref, "users", "r1", 0).await?;
    ctx.post(
        &format!("/api/v1/cache/namespaces/users/generations/{generation_id}/seal"),
        json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_chunk_count": 0 }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.post(
        &format!("/api/v1/cache/namespaces/users/generations/{generation_id}/promote"),
        json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "expected_active_generation_id": null }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);

    // A scan of a published-but-empty snapshot is an empty page pinned to the
    // active generation — NOT a cache_not_populated conflict.
    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces/users/entries?owner_type=pack&owner_ref={}&limit=10",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let page: Value = response.json().await?;
    assert_eq!(page["data"]["generation_id"], generation_id);
    assert!(page["data"]["items"].as_array().unwrap().is_empty());
    assert!(page["data"]["next_cursor"].is_null());
    assert_eq!(page["data"]["record_count"], 0);

    // Point lookup returns an authorized miss with the active generation, not a
    // not-populated error.
    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/entries/lookup",
            json!({ "owner_type": "pack", "owner_ref": pack.r#ref, "external_id": "anything" }),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["generation_id"], generation_id);
    assert!(body["data"]["item"].is_null());

    // The namespace reports populated (has an active generation).
    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref={}",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["cache_not_populated"], false);
    assert_eq!(body["data"]["record_count"], 0);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_metadata_lists_support_filters_and_keyset_cursors() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_metadata_pages").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_metadata_pages_writer",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;

    for namespace in ["alpha.users", "alpha.locations", "beta.users"] {
        let policy = if namespace == "alpha.users" {
            json!({"max_staging_generations": 4})
        } else {
            json!({})
        };
        create_namespace(&ctx, &token, &pack.r#ref, namespace, policy)
            .await?
            .assert_status(StatusCode::CREATED);
    }

    let first = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces?owner_type=pack&owner_ref={}&namespace=alpha&freshness=unpopulated&limit=1",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    let first = first.assert_status(StatusCode::OK);
    let first: Value = first.json().await?;
    assert_eq!(first["data"]["namespaces"].as_array().unwrap().len(), 1);
    let cursor = first["data"]["next_cursor"]
        .as_str()
        .expect("namespace cursor");

    let second = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces?owner_type=pack&owner_ref={}&cursor={cursor}",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    let second = second.assert_status(StatusCode::OK);
    let second: Value = second.json().await?;
    assert_eq!(second["data"]["namespaces"].as_array().unwrap().len(), 1);
    assert!(second["data"]["next_cursor"].is_null());
    assert!(second["data"]["namespaces"][0]["namespace"]
        .as_str()
        .unwrap()
        .starts_with("alpha."));

    let mismatch = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces?owner_type=pack&owner_ref={}&namespace=beta&cursor={cursor}",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    mismatch.assert_status(StatusCode::BAD_REQUEST);

    let mut created_generations = Vec::new();
    for refresh in ["metadata-1", "metadata-2", "metadata-3"] {
        created_generations
            .push(begin_generation(&ctx, &token, &pack.r#ref, "alpha.users", refresh, 0).await?);
    }
    ctx.post(
        &format!(
            "/api/v1/cache/namespaces/alpha.users/generations/{}/seal",
            created_generations[0]
        ),
        json!({
            "owner_type": "pack",
            "owner_ref": pack.r#ref,
            "expected_chunk_count": 0
        }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);
    ctx.post(
        &format!(
            "/api/v1/cache/namespaces/alpha.users/generations/{}/promote",
            created_generations[0]
        ),
        json!({
            "owner_type": "pack",
            "owner_ref": pack.r#ref,
            "expected_active_generation_id": null
        }),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);

    let fresh = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces?owner_type=pack&owner_ref={}&namespace=alpha.users&freshness=fresh",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::OK);
    let fresh: Value = fresh.json().await?;
    assert_eq!(fresh["data"]["namespaces"].as_array().unwrap().len(), 1);
    assert_eq!(
        fresh["data"]["namespaces"][0]["active_generation"],
        created_generations[0]
    );
    assert_eq!(fresh["data"]["namespaces"][0]["record_count"], 0);

    let mut cursor = None;
    let mut generation_ids = Vec::new();
    loop {
        let mut path = format!(
            "/api/v1/cache/namespaces/alpha.users/generations?owner_type=pack&owner_ref={}&limit=1",
            pack.r#ref
        );
        if let Some(cursor) = cursor.as_deref() {
            path.push_str("&cursor=");
            path.push_str(cursor);
        }
        let response = ctx.get(&path, Some(&token)).await?;
        let response = response.assert_status(StatusCode::OK);
        let body: Value = response.json().await?;
        generation_ids.push(
            body["data"]["generations"][0]["generation_id"]
                .as_i64()
                .unwrap(),
        );
        cursor = body["data"]["next_cursor"].as_str().map(ToOwned::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    generation_ids.sort_unstable();
    generation_ids.dedup();
    assert_eq!(generation_ids.len(), 3);
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn cache_rbac_honors_identity_attributes_and_audits_denials() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_attribute_authz").await?;
    let grants = json!([{
        "resource": "caches",
        "actions": ["read", "create"],
        "constraints": {
            "owner_types": ["pack"],
            "owner_refs": [pack.r#ref],
            "attributes": {"department": "sales"}
        }
    }]);
    let (token, identity_id) = register_user(&ctx, "cache_attribute_user", grants).await?;
    set_identity_attributes(&ctx, identity_id, json!({"department": "sales"})).await?;

    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);

    set_identity_attributes(&ctx, identity_id, json!({"department": "engineering"})).await?;
    let denied = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces/users?owner_type=pack&owner_ref={}",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    denied.assert_status(StatusCode::FORBIDDEN);

    let filters = AuditEventFilters {
        category: Some(AuditCategory::Rbac),
        event_type: Some("rbac.denied".to_string()),
        outcome: Some(AuditOutcome::Denied),
        actor_identity: Some(identity_id),
        resource_type: Some("caches".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    ctx.flush_audit().await?;
    let audited = !AuditRepository::search(&ctx.pool, &filters)
        .await?
        .is_empty();
    assert!(audited, "RBAC denial should be persisted to the audit log");
    Ok(())
}
