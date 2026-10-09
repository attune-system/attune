//! Integration tests for the owner-scoped cache API.
//!
//! These exercise the deployed router against a schema-per-test PostgreSQL
//! database: refresh lifecycle, generation-pinned reads, RBAC visibility,
//! fail-closed token handling, cursor integrity, conflict/precondition, and
//! quota behavior.

use anyhow::Context;
use axum::http::StatusCode;
use helpers::*;
use serde_json::{json, Value};

use attune_common::{
    audit::{AuditCategory, AuditEventFilters, AuditOutcome, AuditRepository},
    auth::jwt::{
        generate_execution_token_with_permission_sets, generate_sensor_token,
        generate_sensor_token_with_cache_authority_and_workload_fence, generate_token,
        generate_worker_token_with_instance, validate_token, JwtConfig, TokenType,
    },
    config::CacheAdmissionConfig,
    models::{
        enums::WorkerStatus, enums::WorkerType, ActionReferenceVisibility, ExecutionStatus,
        OwnerType, SensorWorkloadFence, WorkflowCacheIterationState,
    },
    repositories::{
        cache::{
            CacheEntryRepository, CacheGenerationRepository, CacheNamespacePolicy,
            CacheNamespaceRepository, CacheOwnerScope, CacheTransactionMode,
            CreateCacheNamespaceInput, ManagedCacheNamespaceDefinition,
        },
        component_lifecycle::PackProjectionIds,
        execution::{CreateExecutionInput, ExecutionRepository},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository, UpdateIdentityInput,
        },
        key::{CreateKeyInput, KeyRepository},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        runtime::{CreateRuntimeInput, CreateWorkerInput, RuntimeRepository, WorkerRepository},
        sensor_workload::{
            AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadRepository,
        },
        trigger::{CreateSensorInput, CreateTriggerInput, SensorRepository, TriggerRepository},
        workflow::{CreateWorkflowExecutionInput, WorkflowExecutionRepository},
        workflow_cache_iteration::{
            CreateWorkflowCacheIterationInput, WorkflowCacheIterationRepository,
        },
        Create, FindById, Update,
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

fn coordination_request(pack_ref: &str, refresh_id: &str) -> Value {
    json!({
        "owner_type": "pack",
        "owner_ref": pack_ref,
        "client_refresh_id": refresh_id,
        "expected_active_generation_id": null,
        "expected_chunk_count": 0,
    })
}

async fn cache_partition_count(pool: &sqlx::PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_inherits WHERE inhparent = 'cache_entry'::regclass",
    )
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn partition_cap_preserves_matching_retry_and_reuse_without_extra_storage() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new_with_cache_admission(CacheAdmissionConfig {
        max_entry_partitions: 1,
        ..CacheAdmissionConfig::default()
    })
    .await?;
    let pack = create_test_pack(&ctx.pool, "cache_partition_cap").await?;
    let (token, _) =
        register_user(&ctx, "cache_partition_cap", pack_writer_grants(&pack.r#ref)).await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    let id = begin_generation(&ctx, &token, &pack.r#ref, "users", "original", 0).await?;
    let path = "/api/v1/cache/namespaces/users/generations";
    let retry = ctx
        .post(
            path,
            coordination_request(&pack.r#ref, "original"),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::OK);
    let retry: Value = retry.json().await?;
    assert_eq!(retry["data"]["generation_id"], id);
    let rejected = ctx
        .post(path, coordination_request(&pack.r#ref, "new"), Some(&token))
        .await?
        .assert_status(StatusCode::CONFLICT);
    let rejected: Value = rejected.json().await?;
    assert_eq!(rejected["code"], "cache_entry_partition_limit_exceeded");
    ctx.put(
        "/api/v1/cache/namespaces/users",
        json!({"owner_type": "pack", "owner_ref": pack.r#ref, "refresh_concurrency": "reuse"}),
        Some(&token),
    )
    .await?
    .assert_status(StatusCode::OK);
    let reused = ctx
        .post(
            path,
            coordination_request(&pack.r#ref, "reused"),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::OK);
    let reused: Value = reused.json().await?;
    assert_eq!(reused["data"], retry["data"]);
    assert_eq!(cache_partition_count(&ctx.pool).await?, 1);
    let usage: (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT, COALESCE(SUM(record_count), 0)::BIGINT, \
         COALESCE(SUM(physical_bytes), 0)::BIGINT FROM cache_generation_entry_usage",
    )
    .fetch_one(&ctx.pool)
    .await?;
    assert_eq!(usage, (1, 0, 0));
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn partition_lock_timeout_returns_stable_503_and_same_refresh_retry_succeeds() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_partition_busy").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_partition_busy",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    let mut blocker = ctx.pool.begin().await?;
    sqlx::query("LOCK TABLE ONLY cache_entry IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let observed = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_locks \
                     WHERE relation = 'cache_entry'::regclass AND NOT granted \
                     AND mode = 'ShareUpdateExclusiveLock' \
                     AND $1 = ANY(pg_blocking_pids(pid)))",
                )
                .bind(blocker_pid)
                .fetch_one(&ctx.pool)
                .await?;
                if waiting {
                    return Ok::<_, sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let request = ctx.post(
        "/api/v1/cache/namespaces/users/generations",
        coordination_request(&pack.r#ref, "retry-me"),
        Some(&token),
    );
    let (response, observed) = tokio::join!(request, observed);
    // Release the owned blocker even when the request or observation failed.
    blocker.rollback().await?;
    observed?;
    let response = response?.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    let error: Value = response.json().await?;
    assert_eq!(error["code"], "cache_storage_busy");
    assert!(error.get("details").is_none(), "no relation or SQL details");
    assert_eq!(cache_partition_count(&ctx.pool).await?, 0);
    let resources: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM cache_generation), \
         (SELECT COUNT(*) FROM cache_generation_entry_usage), \
         (SELECT COUNT(*) FROM cache_ingest_chunk)",
    )
    .fetch_one(&ctx.pool)
    .await?;
    assert_eq!(resources, (0, 0, 0));
    let id = begin_generation(&ctx, &token, &pack.r#ref, "users", "retry-me", 0).await?;
    let retry = ctx
        .post(
            "/api/v1/cache/namespaces/users/generations",
            coordination_request(&pack.r#ref, "retry-me"),
            Some(&token),
        )
        .await?
        .assert_status(StatusCode::OK);
    let retry: Value = retry.json().await?;
    assert_eq!(retry["data"]["generation_id"], id);
    assert_eq!(cache_partition_count(&ctx.pool).await?, 1);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn partition_creation_failure_rolls_back_metadata_and_retry_contract() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_partition_failure").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_partition_failure",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    create_namespace(&ctx, &token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    // Owned event trigger fails actual ATTACH DDL after generation/usage insertion.
    sqlx::raw_sql(
        "CREATE FUNCTION test_reject_cache_attach() RETURNS event_trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF EXISTS(SELECT 1 FROM pg_event_trigger_ddl_commands() \
                         WHERE object_identity LIKE '%cache_entry') THEN \
              RAISE EXCEPTION 'owned ATTACH failure' USING ERRCODE = 'XX000'; END IF; END $$; \
         CREATE EVENT TRIGGER test_cache_attach_failure ON ddl_command_end \
         WHEN TAG IN ('ALTER TABLE') EXECUTE FUNCTION test_reject_cache_attach();",
    )
    .execute(&ctx.pool)
    .await?;
    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/generations",
            coordination_request(&pack.r#ref, "ddl-retry"),
            Some(&token),
        )
        .await?;
    sqlx::raw_sql(
        "DROP EVENT TRIGGER test_cache_attach_failure; DROP FUNCTION test_reject_cache_attach();",
    )
    .execute(&ctx.pool)
    .await?;
    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(cache_partition_count(&ctx.pool).await?, 0);
    let resources: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM cache_generation), \
         (SELECT COUNT(*) FROM cache_generation_entry_usage), \
          (SELECT COUNT(*) FROM pg_class WHERE relkind = 'r' AND relname ~ '^cache_entry_g_[0-9]+$')",
    )
    .fetch_one(&ctx.pool)
    .await?;
    assert_eq!(resources, (0, 0, 0));
    begin_generation(&ctx, &token, &pack.r#ref, "users", "ddl-retry", 0).await?;
    assert_eq!(cache_partition_count(&ctx.pool).await?, 1);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn pack_delete_handler_waits_for_pin_admission_before_pack_sensor_and_cascade_locks(
) -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let pack_ref = format!("cache_delete_handler_{}", &suffix[..8]);
    let pack = create_test_pack(&ctx.pool, &pack_ref).await?;
    let mut grants = pack_writer_grants(&pack.r#ref);
    grants
        .as_array_mut()
        .unwrap()
        .push(json!({"resource":"packs", "actions":["read","delete"]}));
    let (access_token, _) = register_user(&ctx, "cache_delete_handler", grants).await?;
    let token = access_token.as_str();
    let projection = ctx.test_packs_dir.join(&pack.r#ref);
    let runtime = std::path::Path::new(&ctx.state.config.runtime_envs_dir)
        .join(&pack.r#ref)
        .join("python");
    std::fs::create_dir_all(&projection)?;
    std::fs::write(
        projection.join("pack.yaml"),
        format!("ref: {}\n", pack.r#ref),
    )?;
    std::fs::create_dir_all(&runtime)?;
    std::fs::write(runtime.join("installed"), "true")?;
    let digest = "d".repeat(64);
    let release_tree = ctx.test_packs_dir.join(".releases/sha256").join(&digest);
    let release_content = release_tree.join("pack");
    std::fs::create_dir_all(&release_content)?;
    std::fs::write(
        release_content.join("pack.yaml"),
        format!("ref: {}\n", pack.r#ref),
    )?;
    let mut setup = ctx.pool.begin().await?;
    PackReleaseRepository::create_or_get(
        &mut setup,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest,
            object_key: "owned-cache-delete-handler".into(),
            provider_version: "test".into(),
            content_path: release_content.to_str().unwrap().into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await?;
    setup.commit().await?;
    create_namespace(&ctx, token, &pack.r#ref, "users", json!({}))
        .await?
        .assert_status(StatusCode::CREATED);
    let generation =
        begin_generation(&ctx, token, &pack.r#ref, "users", "delete-handler", 0).await?;
    for operation in ["seal", "promote"] {
        ctx.post(
            &format!("/api/v1/cache/namespaces/users/generations/{generation}/{operation}"),
            json!({"owner_type":"pack", "owner_ref":pack.r#ref,
                "expected_chunk_count":0,"expected_active_generation_id":null}),
            Some(token),
        )
        .await?
        .assert_status(StatusCode::OK);
    }
    let namespace = CacheGenerationRepository::find_by_id(&ctx.pool, generation)
        .await?
        .unwrap()
        .namespace;
    let action = create_test_action(
        &ctx.pool,
        pack.id,
        &pack.r#ref,
        &format!("{}.workflow", pack.r#ref),
    )
    .await?;
    let definition = create_test_workflow(&ctx.pool, pack.id, &pack.r#ref, &action.r#ref).await?;
    let root = ExecutionRepository::create(
        &ctx.pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref,
            status: ExecutionStatus::Completed,
            ..Default::default()
        },
    )
    .await?;
    let workflow = WorkflowExecutionRepository::create(
        &ctx.pool,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: definition.id,
            task_graph: json!({}),
            variables: json!({}),
            status: ExecutionStatus::Completed,
        },
    )
    .await?;
    let mut pin = ctx.pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut pin, CacheTransactionMode::PinMutation).await?;
    let iteration = WorkflowCacheIterationRepository::create_or_find_for_update(
        &mut pin,
        CreateWorkflowCacheIterationInput {
            workflow_execution: workflow.id,
            task_name: "consume".into(),
            namespace,
            generation,
            page_size: 1,
            batch_size: 1,
            concurrency: 1,
        },
    )
    .await?;
    WorkflowCacheIterationRepository::mark_terminal(
        &mut *pin,
        iteration.id,
        WorkflowCacheIterationState::Completed,
        None,
    )
    .await?;
    pin.commit().await?;
    let mut gate = ctx.pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut gate, CacheTransactionMode::PinMutation).await?;
    let gate_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await?;
    let mut observer = ctx.pool.acquire().await?;
    let request_path = format!("/api/v1/packs/{}", pack.r#ref);
    let read_path = format!(
        "/api/v1/cache/namespaces/users/entries?owner_type=pack&owner_ref={}&limit=1",
        pack.r#ref
    );
    let deletion = ctx.delete(&request_path, Some(token));
    let observed = async {
        let result: Result<_> = async {
            let waiting = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let pid: Option<i32> = sqlx::query_scalar(
                        "SELECT pid FROM pg_locks WHERE locktype='advisory' AND classid=7821101 AND objid=0 \
                         AND objsubid=2 AND NOT granted AND $1=ANY(pg_blocking_pids(pid))",
                    ).bind(gate_pid).fetch_optional(&mut *observer).await?;
                    if let Some(pid) = pid { return Ok::<_, sqlx::Error>(pid); }
                    tokio::task::yield_now().await;
                }
            }).await??;
            let locks: (i64, i64) = sqlx::query_as(
                "SELECT (SELECT COUNT(*) FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND granted), \
                 (SELECT COUNT(*) FROM pg_locks WHERE pid=$1 AND relation IN \
                  ('cache_entry'::regclass,'pack'::regclass,'pack_release'::regclass,'sensor'::regclass, \
                   'workflow_definition'::regclass,'workflow_execution'::regclass,'workflow_cache_iteration'::regclass, \
                   'cache_namespace'::regclass,'cache_generation_entry_usage'::regclass))",
            ).bind(waiting).fetch_one(&mut *observer).await?;
            let before: (bool, i64, i64) = sqlx::query_as(
                "SELECT (SELECT tombstoned_at IS NULL FROM cache_namespace WHERE id=$1), \
                 (SELECT retained_iterations FROM cache_generation_entry_usage WHERE generation=$2), \
                 (SELECT COUNT(*) FROM workflow_cache_iteration WHERE generation=$2)",
            ).bind(namespace).bind(generation).fetch_one(&mut *observer).await?;
            let read = tokio::time::timeout(std::time::Duration::from_secs(10), ctx.get(&read_path, Some(token))).await??;
            let read_status = read.status();
            let read_body: Value = read.json().await?;
            Ok((locks, before, read_status, read_body,
                projection.join("pack.yaml").is_file(), runtime.join("installed").is_file()))
        }.await;
        gate.rollback().await?;
        result
    };
    let (response, observation) = tokio::join!(deletion, observed);
    drop(observer);
    let response = response?;
    let status = response.status();
    let observation = observation?;
    let after: (bool, bool, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT tombstoned_at IS NOT NULL AND owner_pack IS NULL AND active_generation IS NULL \
                 FROM cache_namespace WHERE id=$1), \
         EXISTS(SELECT 1 FROM cache_generation WHERE id=$2), \
         (SELECT retained_iterations FROM cache_generation_entry_usage WHERE generation=$2), \
         (SELECT COUNT(*) FROM workflow_cache_iteration WHERE generation=$2), \
         (SELECT COUNT(*) FROM pack_release WHERE pack=$3)",
    ).bind(namespace).bind(generation).bind(pack.id).fetch_one(&ctx.pool).await?;
    let pack_gone = ctx.get(&request_path, Some(token)).await?.status();
    let removed_files = !projection.exists() && !runtime.exists() && !release_tree.exists();
    ctx.cleanup().await?;
    assert_eq!(
        observation.0,
        (0, 0),
        "handler acquired pack/sensor advisory or source relation locks before admission"
    );
    assert_eq!(
        observation.1,
        (true, 1, 1),
        "namespace and retained counters changed while deletion was blocked"
    );
    assert_eq!(
        observation.2,
        StatusCode::OK,
        "ordinary API cache reads must stay concurrent with held PinMutation"
    );
    assert_eq!(observation.3["data"]["generation_id"], generation);
    assert!(
        observation.4 && observation.5,
        "projection/runtime staging must wait for admission"
    );
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pack_gone, StatusCode::NOT_FOUND);
    assert_eq!(after, (true, true, 0, 0, 0), "pack deletion must atomically tombstone cache ownership and release iteration metadata, not storage");
    assert!(removed_files, "committed pack deletion must remove projection, runtime environment and unreferenced release tree");
    Ok(())
}

#[tokio::test]
async fn refresh_reuse_is_atomic_and_returns_original_generation_metadata() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_coord_reuse").await?;
    let (token, identity_id) =
        register_user(&ctx, "cache_coord_reuse", pack_writer_grants(&pack.r#ref)).await?;
    let response = create_namespace(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        json!({"refresh_concurrency": "reuse"}),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let namespace: Value = response.json().await?;
    assert_eq!(namespace["data"]["refresh_concurrency"], "reuse");
    let namespace_id = namespace["data"]["id"].as_i64().unwrap();
    let path = "/api/v1/cache/namespaces/users/generations";
    let (first, second) = tokio::join!(
        ctx.post(
            path,
            coordination_request(&pack.r#ref, "refresh-a"),
            Some(&token)
        ),
        ctx.post(
            path,
            coordination_request(&pack.r#ref, "refresh-b"),
            Some(&token)
        ),
    );
    let first = first?;
    let second = second?;
    assert!(matches!(
        (first.status(), second.status()),
        (StatusCode::CREATED, StatusCode::OK) | (StatusCode::OK, StatusCode::CREATED)
    ));
    let first: Value = first.json().await?;
    let second: Value = second.json().await?;
    assert_eq!(first["data"], second["data"]);
    assert_eq!(first["data"]["created_by"], identity_id);
    assert!(first["data"]["created_by_execution"].is_null());
    assert_eq!(
        CacheGenerationRepository::list_for_namespace(&ctx.pool, namespace_id, 10)
            .await?
            .len(),
        1
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn refresh_conflict_exposes_safe_metadata_only_after_cache_authorization() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_coord_conflict").await?;
    let (token, _) = register_user(
        &ctx,
        "cache_coord_conflict",
        pack_writer_grants(&pack.r#ref),
    )
    .await?;
    let response = create_namespace(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        json!({"refresh_concurrency": "conflict"}),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let id = begin_generation(&ctx, &token, &pack.r#ref, "users", "original", 0).await?;
    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/generations",
            coordination_request(&pack.r#ref, "different"),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await?;
    assert_eq!(body["code"], "cache_refresh_in_progress");
    assert_eq!(body["details"]["generation_id"], id);
    assert!(body["details"]["created_by_execution"].is_null());
    assert_eq!(body["details"].as_object().unwrap().len(), 2);
    let (denied, _) = register_user(&ctx, "cache_coord_denied", json!([])).await?;
    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/generations",
            coordination_request(&pack.r#ref, "denied"),
            Some(&denied),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value = response.json().await?;
    assert!(body.get("details").is_none());
    assert_ne!(body["code"], "cache_refresh_in_progress");
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn namespace_refresh_policy_round_trips_and_keeps_parallel_default() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_coord_policy").await?;
    let (token, _) =
        register_user(&ctx, "cache_coord_policy", pack_writer_grants(&pack.r#ref)).await?;
    let response = create_namespace(&ctx, &token, &pack.r#ref, "users", json!({})).await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["refresh_concurrency"], "parallel");
    for policy in ["reuse", "conflict", "parallel"] {
        let response = ctx
            .put(
                "/api/v1/cache/namespaces/users",
                json!({
                    "owner_type": "pack", "owner_ref": pack.r#ref, "refresh_concurrency": policy,
                }),
                Some(&token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        assert_eq!(body["data"]["refresh_concurrency"], policy);
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
        assert_eq!(body["data"]["refresh_concurrency"], policy);
    }
    let response = ctx
        .put(
            "/api/v1/cache/namespaces/users",
            json!({
                "owner_type": "pack", "owner_ref": pack.r#ref, "refresh_concurrency": "invalid",
            }),
            Some(&token),
        )
        .await?;
    // Typed Json extraction rejects unsupported enum values before the route runs.
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let first = begin_generation(&ctx, &token, &pack.r#ref, "users", "parallel-a", 0).await?;
    let second = begin_generation(&ctx, &token, &pack.r#ref, "users", "parallel-b", 0).await?;
    assert_ne!(first, second);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn execution_refresh_attribution_survives_reuse_and_metadata_reads() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_coord_execution").await?;
    let action = create_test_action(
        &ctx.pool,
        pack.id,
        &pack.r#ref,
        "cache_coord_execution.populate",
    )
    .await?;
    let grants = pack_writer_grants(&pack.r#ref);
    let (token, identity_id) = register_user(&ctx, "cache_coord_execution", grants.clone()).await?;
    let permission_set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "test.cache_coord_execution".to_string(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity_id,
            permset: permission_set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity_id).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    let mut executions = Vec::new();
    for _ in 0..2 {
        executions.push(
            ExecutionRepository::create(
                &ctx.pool,
                CreateExecutionInput {
                    action: Some(action.id),
                    action_ref: action.r#ref.clone(),
                    executor: Some(identity_id),
                    status: ExecutionStatus::Running,
                    permission_set_refs: vec![permission_set.r#ref.clone()],
                    ..Default::default()
                },
            )
            .await?,
        );
    }
    let response = create_namespace(
        &ctx,
        &token,
        &pack.r#ref,
        "users",
        json!({"refresh_concurrency": "reuse"}),
    )
    .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let mut generation_id = None;
    for (index, execution) in executions.iter().enumerate() {
        let execution_token = generate_execution_token_with_permission_sets(
            identity_id,
            execution.id,
            &action.r#ref,
            &test_jwt_config(),
            Some(300),
            &[permission_set.r#ref.clone()],
        )?;
        let response = ctx
            .post(
                "/api/v1/cache/namespaces/users/generations",
                coordination_request(&pack.r#ref, &format!("execution-{index}")),
                Some(&execution_token),
            )
            .await?;
        assert_eq!(
            response.status(),
            if index == 0 {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            }
        );
        let body: Value = response.json().await?;
        assert_eq!(body["data"]["created_by_execution"], executions[0].id);
        assert_eq!(body["data"]["created_by"], identity_id);
        let id = body["data"]["generation_id"].as_i64().unwrap();
        assert_eq!(*generation_id.get_or_insert(id), id);
    }
    let id = generation_id.unwrap();
    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces/users/generations/{id}?owner_type=pack&owner_ref={}",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["created_by_execution"], executions[0].id);
    let response = ctx
        .get(
            &format!(
                "/api/v1/cache/namespaces/users/generations?owner_type=pack&owner_ref={}",
                pack.r#ref
            ),
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(
        body["data"]["generations"][0]["created_by_execution"],
        executions[0].id
    );
    let generation = CacheGenerationRepository::find_by_id(&ctx.pool, id)
        .await?
        .unwrap();
    assert_eq!(generation.created_by_execution, Some(executions[0].id));
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn generation_creation_rejects_client_supplied_execution_attribution() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new().await?;
    let pack = create_test_pack(&ctx.pool, "cache_coord_spoof").await?;
    let (token, _) =
        register_user(&ctx, "cache_coord_spoof", pack_writer_grants(&pack.r#ref)).await?;
    let response = create_namespace(&ctx, &token, &pack.r#ref, "users", json!({})).await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response.json().await?;
    let namespace_id = body["data"]["id"].as_i64().unwrap();
    let mut request = coordination_request(&pack.r#ref, "spoofed");
    request["created_by_execution"] = json!(12345);
    let response = ctx
        .post(
            "/api/v1/cache/namespaces/users/generations",
            request,
            Some(&token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        CacheGenerationRepository::list_for_namespace(&ctx.pool, namespace_id, 10)
            .await?
            .is_empty()
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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

    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn registered_sensor_tokens_use_exact_signed_read_only_authority() -> Result<()> {
    init_test_env();
    let ctx = TestContext::new()
        .await
        .map_err(|error| std::io::Error::other(format!("create test context: {error}")))?;
    let pack = create_test_pack(&ctx.pool, "sensor_cache_scope")
        .await
        .map_err(|error| std::io::Error::other(format!("create sensor pack: {error}")))?;
    let other_pack = create_test_pack(&ctx.pool, "sensor_cache_other")
        .await
        .map_err(|error| std::io::Error::other(format!("create other pack: {error}")))?;
    let (writer, writer_identity_id) = register_user(
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
    .await
    .map_err(|error| std::io::Error::other(format!("register cache writer: {error}")))?;
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
    .await
    .context("create sensor runtime")?;
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
    .await
    .context("create cache reader sensor")?;
    let trigger_ref = format!("{}.cache_probe", pack.r#ref);
    let trigger = TriggerRepository::create(
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
    .await
    .context("create cache probe trigger")?;
    activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &PackProjectionIds {
            runtimes: vec![runtime.id],
            triggers: vec![trigger.id],
            sensors: vec![sensor.id],
            ..PackProjectionIds::default()
        },
    )
    .await
    .map_err(|error| {
        std::io::Error::other(format!("activate projected sensor pack release: {error}"))
    })?;

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
    .await
    .context("create sensor worker")?;
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
    .await
    .context("acquire sensor workload")?
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

    let encryption_key = ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .expect("test encryption key");
    let own_key = KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            local_ref: "sensor_credentials".to_string(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(pack.id),
            owner_pack_ref: Some(pack.r#ref.clone()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Sensor credentials".to_string(),
            encrypted: true,
            encryption_key_hash: Some(attune_common::crypto::hash_encryption_key(encryption_key)),
            value: attune_common::crypto::encrypt_json(
                &json!({"token": "sensor-secret"}),
                encryption_key,
            )?,
        },
    )
    .await?;
    let other_key = KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            local_ref: "other_credentials".to_string(),
            owner_type: OwnerType::Pack,
            owner_identity: None,
            owner_pack: Some(other_pack.id),
            owner_pack_ref: Some(other_pack.r#ref.clone()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Other credentials".to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!({"token": "other-secret"}),
        },
    )
    .await?;
    let system_key = KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            local_ref: "system_credentials".to_string(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "System credentials".to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!({"token": "system-secret"}),
        },
    )
    .await?;
    let identity_key = KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            local_ref: "identity_credentials".to_string(),
            owner_type: OwnerType::Identity,
            owner_identity: Some(writer_identity_id),
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Identity credentials".to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!({"token": "identity-secret"}),
        },
    )
    .await?;
    let sensor_key = KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            local_ref: "owned_sensor_credentials".to_string(),
            owner_type: OwnerType::Sensor,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: Some(sensor.id),
            owner_sensor_ref: Some(sensor.r#ref.clone()),
            name: "Sensor credentials".to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: json!({"token": "sensor-owner-secret"}),
        },
    )
    .await?;

    let response = ctx
        .get("/api/v1/keys?per_page=100", Some(sensor_token))
        .await?
        .assert_status(StatusCode::OK);
    let key_list: Value = response.json().await?;
    let listed_keys = key_list["items"].as_array().expect("key list items");
    assert_eq!(listed_keys.len(), 1);
    assert_eq!(listed_keys[0]["ref"], own_key.r#ref);

    let response = ctx
        .get(
            &format!("/api/v1/keys/{}", own_key.r#ref),
            Some(sensor_token),
        )
        .await?
        .assert_status(StatusCode::OK);
    let own_key_body: Value = response.json().await?;
    assert_eq!(
        own_key_body["data"]["value"],
        json!({"token": "sensor-secret"})
    );
    for hidden_ref in [
        &other_key.r#ref,
        &system_key.r#ref,
        &identity_key.r#ref,
        &sensor_key.r#ref,
    ] {
        ctx.get(&format!("/api/v1/keys/{hidden_ref}"), Some(sensor_token))
            .await?
            .assert_status(StatusCode::NOT_FOUND);
    }

    ctx.get("/api/v1/keys", Some(&worker_token))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    ctx.get(
        &format!("/api/v1/keys/{}", own_key.r#ref),
        Some(&worker_token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    ctx.post(
        "/api/v1/keys",
        json!({
            "local_ref": "sensor_write",
            "owner_type": "pack",
            "owner_pack_ref": pack.r#ref,
            "name": "Sensor write",
            "value": "blocked",
            "encrypted": true
        }),
        Some(sensor_token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    ctx.put(
        &format!("/api/v1/keys/{}", own_key.r#ref),
        json!({"name": "Blocked update"}),
        Some(sensor_token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);
    ctx.delete(
        &format!("/api/v1/keys/{}", own_key.r#ref),
        Some(sensor_token),
    )
    .await?
    .assert_status(StatusCode::FORBIDDEN);

    let sensor_identity_id = validate_token(sensor_token, &test_jwt_config())?
        .sub
        .parse::<i64>()?;
    let wrong_pack_token = generate_sensor_token_with_cache_authority_and_workload_fence(
        sensor_identity_id,
        &sensor_ref,
        vec![trigger_ref.clone()],
        Some(&other_pack.r#ref),
        &[],
        &[],
        SensorWorkloadFence {
            workload_id: workload.workload_id,
            worker_id: worker.id,
            worker_instance,
            generation: workload.generation,
        },
        &test_jwt_config(),
        Some(300),
    )?;
    ctx.get("/api/v1/keys", Some(&wrong_pack_token))
        .await?
        .assert_status(StatusCode::UNAUTHORIZED);
    let stale_token = generate_sensor_token_with_cache_authority_and_workload_fence(
        sensor_identity_id,
        &sensor_ref,
        vec![trigger_ref.clone()],
        Some(&pack.r#ref),
        &[],
        &[],
        SensorWorkloadFence {
            workload_id: workload.workload_id,
            worker_id: worker.id,
            worker_instance,
            generation: workload.generation + 1,
        },
        &test_jwt_config(),
        Some(300),
    )?;
    ctx.get("/api/v1/keys", Some(&stale_token))
        .await?
        .assert_status(StatusCode::UNAUTHORIZED);

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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
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
    ctx.cleanup().await?;
    Ok(())
}
