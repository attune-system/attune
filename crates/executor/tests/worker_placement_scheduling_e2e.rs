//! E2E-style scheduler tests for worker placement constraints.
//!
//! These tests use real database rows for packs, actions, executions, and workers,
//! then run the executor's worker selection path. They are ignored by default
//! because they require a PostgreSQL/TimescaleDB test database.

use attune_common::{
    config::Config,
    models::{
        action::Action, enums::ExecutionStatus, enums::WorkerStatus, enums::WorkerType,
        execution::WorkflowTaskMetadata, Pack, Worker,
    },
    repositories::{
        action::{ActionRepository, CreateActionInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        pack::{CreatePackInput, PackRepository},
        Create,
    },
    test_database::TestDatabase,
};
use attune_executor::scheduler::ExecutionScheduler;
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use std::sync::atomic::AtomicUsize;

async fn create_test_pool() -> anyhow::Result<TestDatabase> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let config_path = format!("{}/../../config.test.yaml", manifest_dir);
    let config = Config::load_from_file(&config_path)?;
    Ok(TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop())
}

async fn create_pack(pool: &PgPool, suffix: &str) -> anyhow::Result<Pack> {
    Ok(PackRepository::create(
        pool,
        CreatePackInput {
            r#ref: format!("placement_pack_{}", suffix),
            label: format!("Placement Pack {}", suffix),
            description: Some("Worker placement scheduler test pack".to_string()),
            version: "1.0.0".to_string(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: Vec::new(),
            runtime_deps: Vec::new(),
            dependencies: Vec::new(),
            is_standard: false,
            installers: json!({}),
        },
    )
    .await?)
}

async fn create_action(
    pool: &PgPool,
    pack: &Pack,
    suffix: &str,
    worker_selector: JsonValue,
    worker_tolerations: JsonValue,
    worker_affinity: JsonValue,
) -> anyhow::Result<Action> {
    Ok(ActionRepository::create(
        pool,
        CreateActionInput {
            r#ref: format!("{}.{}", pack.r#ref, suffix),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: format!("Placement Action {}", suffix),
            description: Some("Worker placement scheduler test action".to_string()),
            entrypoint: "echo test".to_string(),
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: json!({}),
            worker_selector,
            worker_tolerations,
            worker_affinity,
            param_schema: None,
            out_schema: None,
            is_adhoc: false,
            accesses_mcp: false,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        },
    )
    .await?)
}

async fn create_execution(pool: &PgPool, action: &Action) -> anyhow::Result<i64> {
    create_execution_with_placement(pool, action, None, None, None).await
}

async fn create_execution_with_placement(
    pool: &PgPool,
    action: &Action,
    worker_selector: Option<JsonValue>,
    worker_tolerations: Option<JsonValue>,
    worker_affinity: Option<JsonValue>,
) -> anyhow::Result<i64> {
    Ok(ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector,
            worker_tolerations,
            worker_affinity,
            worker: None,
            status: ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await?
    .id)
}

async fn create_workflow_task_execution_with_placement(
    pool: &PgPool,
    action: &Action,
    worker_selector: Option<JsonValue>,
    worker_tolerations: Option<JsonValue>,
    worker_affinity: Option<JsonValue>,
) -> anyhow::Result<i64> {
    Ok(ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: Some(1),
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector,
            worker_tolerations,
            worker_affinity,
            worker: None,
            status: ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: Some(WorkflowTaskMetadata {
                workflow_execution: 1,
                task_name: "placement_task".to_string(),
                triggered_by: None,
                task_index: None,
                task_batch: None,
                retry_count: 0,
                max_retries: 0,
                next_retry_at: None,
                timeout_seconds: None,
                timed_out: false,
                duration_ms: None,
                started_at: None,
                completed_at: None,
            }),
            timeout_seconds: None,
        },
    )
    .await?
    .id)
}

async fn selected_worker_for_execution_id(
    pool: &PgPool,
    execution_id: i64,
) -> anyhow::Result<Worker> {
    let round_robin = AtomicUsize::new(0);
    ExecutionScheduler::select_worker_for_execution(pool, execution_id, &round_robin).await
}

async fn create_worker(
    pool: &PgPool,
    suffix: &str,
    labels: JsonValue,
    taints: JsonValue,
) -> anyhow::Result<Worker> {
    let name = format!(
        "placement_worker_{}_{}",
        suffix,
        uuid::Uuid::new_v4().simple()
    );
    let capabilities = json!({
        "runtimes": [],
        "labels": labels,
        "taints": taints,
    });

    Ok(sqlx::query_as::<_, Worker>(
        r#"
        INSERT INTO worker (
            name, worker_type, worker_role, runtime, host, port, status,
            capabilities, meta, last_heartbeat
        )
         VALUES ($1, $2, 'action', NULL, 'localhost', NULL, $3, $4, '{}'::jsonb, NOW())
         RETURNING id, name, worker_type, worker_role, runtime, host, port, status,
                  capabilities, meta, last_heartbeat, cordoned, cordon_reason,
                  cordoned_by, cordoned_at, created, updated
        "#,
    )
    .bind(name)
    .bind(WorkerType::Local)
    .bind(WorkerStatus::Active)
    .bind(capabilities)
    .fetch_one(pool)
    .await?)
}

async fn selected_worker_for_execution(pool: &PgPool, action: &Action) -> anyhow::Result<Worker> {
    let execution_id = create_execution(pool, action).await?;
    selected_worker_for_execution_id(pool, execution_id).await
}

#[tokio::test]
#[ignore = "e2e test requires PostgreSQL/TimescaleDB"]
async fn schema_isolated_executions_do_not_reuse_cached_actions() -> anyhow::Result<()> {
    let first_pool = create_test_pool().await?;
    let second_pool = create_test_pool().await?;

    let first_pack = create_pack(&first_pool, "cache_scope").await?;
    let first_worker = create_worker(
        &first_pool,
        "first_scope",
        json!({"scope": "first"}),
        json!([]),
    )
    .await?;
    let first_action = create_action(
        &first_pool,
        &first_pack,
        "cache_scope",
        json!({"scope": "first"}),
        json!([]),
        json!({}),
    )
    .await?;

    let second_pack = create_pack(&second_pool, "cache_scope").await?;
    let _wrong_worker = create_worker(
        &second_pool,
        "first_scope",
        json!({"scope": "first"}),
        json!([]),
    )
    .await?;
    let second_worker = create_worker(
        &second_pool,
        "second_scope",
        json!({"scope": "second"}),
        json!([]),
    )
    .await?;
    let second_action = create_action(
        &second_pool,
        &second_pack,
        "cache_scope",
        json!({"scope": "second"}),
        json!([]),
        json!({}),
    )
    .await?;

    assert_eq!(first_action.id, second_action.id);
    let first_selected = selected_worker_for_execution(&first_pool, &first_action).await?;
    let second_selected = selected_worker_for_execution(&second_pool, &second_action).await?;

    assert_eq!(first_selected.id, first_worker.id);
    assert_eq!(second_selected.id, second_worker.id);
    Ok(())
}

#[tokio::test]
#[ignore = "e2e test requires PostgreSQL/TimescaleDB"]
async fn schedules_execution_on_worker_matching_selector_label() -> anyhow::Result<()> {
    let pool = create_test_pool().await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let pack = create_pack(&pool, &suffix).await?;
    let _plain_worker = create_worker(&pool, "plain", json!({}), json!([])).await?;
    let gpu_worker = create_worker(
        &pool,
        "gpu",
        json!({"gpu": "nvidia", "zone": "east"}),
        json!([]),
    )
    .await?;
    let action = create_action(
        &pool,
        &pack,
        "selector",
        json!({"gpu": "nvidia"}),
        json!([]),
        json!({}),
    )
    .await?;

    let selected = selected_worker_for_execution(&pool, &action).await?;

    assert_eq!(selected.id, gpu_worker.id);
    Ok(())
}

#[tokio::test]
#[ignore = "e2e test requires PostgreSQL/TimescaleDB"]
async fn preferred_affinity_schedules_execution_on_labelled_worker() -> anyhow::Result<()> {
    let pool = create_test_pool().await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let pack = create_pack(&pool, &suffix).await?;
    let _hdd_worker = create_worker(&pool, "hdd", json!({"disk": "hdd"}), json!([])).await?;
    let ssd_worker = create_worker(&pool, "ssd", json!({"disk": "ssd"}), json!([])).await?;
    let action = create_action(
        &pool,
        &pack,
        "preferred_affinity",
        json!({}),
        json!([]),
        json!({
            "preferred": [{
                "weight": 100,
                "preference": {
                    "match_labels": {"disk": "ssd"}
                }
            }]
        }),
    )
    .await?;

    let execution_id = create_execution_with_placement(
        &pool,
        &action,
        None,
        None,
        Some(json!({
            "preferred": [{
                "weight": 100,
                "preference": {"match_labels": {"disk": "ssd"}}
            }]
        })),
    )
    .await?;
    let selected = selected_worker_for_execution_id(&pool, execution_id).await?;

    assert_eq!(selected.id, ssd_worker.id);
    Ok(())
}

#[tokio::test]
#[ignore = "e2e test requires PostgreSQL/TimescaleDB"]
async fn avoids_no_schedule_tainted_worker_without_toleration() -> anyhow::Result<()> {
    let pool = create_test_pool().await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let pack = create_pack(&pool, &suffix).await?;
    let clean_worker = create_worker(&pool, "clean", json!({}), json!([])).await?;
    let _tainted_worker = create_worker(
        &pool,
        "tainted",
        json!({"gpu": "nvidia"}),
        json!([{"key": "gpu", "value": "true", "effect": "no_schedule"}]),
    )
    .await?;
    let action = create_action(
        &pool,
        &pack,
        "no_toleration",
        json!({}),
        json!([]),
        json!({}),
    )
    .await?;

    let selected = selected_worker_for_execution(&pool, &action).await?;

    assert_eq!(selected.id, clean_worker.id);
    Ok(())
}

#[tokio::test]
#[ignore = "e2e test requires PostgreSQL/TimescaleDB"]
async fn schedules_execution_on_tainted_worker_when_tolerated() -> anyhow::Result<()> {
    let pool = create_test_pool().await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let pack = create_pack(&pool, &suffix).await?;
    let _plain_worker = create_worker(&pool, "plain", json!({}), json!([])).await?;
    let tainted_gpu_worker = create_worker(
        &pool,
        "tainted_gpu",
        json!({"gpu": "nvidia"}),
        json!([{"key": "gpu", "value": "true", "effect": "no_schedule"}]),
    )
    .await?;
    let action = create_action(
        &pool,
        &pack,
        "tolerates_gpu",
        json!({"gpu": "nvidia"}),
        json!([{"key": "gpu", "operator": "exists", "effect": "no_schedule"}]),
        json!({}),
    )
    .await?;

    let execution_id = create_execution_with_placement(
        &pool,
        &action,
        Some(json!({"gpu": "nvidia"})),
        Some(json!([{"key": "gpu", "operator": "exists", "effect": "no_schedule"}])),
        None,
    )
    .await?;
    let selected = selected_worker_for_execution_id(&pool, execution_id).await?;

    assert_eq!(selected.id, tainted_gpu_worker.id);
    Ok(())
}

#[tokio::test]
#[ignore = "e2e test requires PostgreSQL/TimescaleDB"]
async fn execution_worker_selector_override_replaces_action_default() -> anyhow::Result<()> {
    let pool = create_test_pool().await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let pack = create_pack(&pool, &suffix).await?;
    let _cpu_worker = create_worker(&pool, "cpu", json!({"pool": "cpu"}), json!([])).await?;
    let gpu_worker = create_worker(&pool, "gpu", json!({"pool": "gpu"}), json!([])).await?;
    let action = create_action(
        &pool,
        &pack,
        "execution_override",
        json!({"pool": "cpu"}),
        json!([]),
        json!({}),
    )
    .await?;

    let execution_id = create_workflow_task_execution_with_placement(
        &pool,
        &action,
        Some(json!({"pool": "gpu"})),
        None,
        None,
    )
    .await?;
    let selected = selected_worker_for_execution_id(&pool, execution_id).await?;

    assert_eq!(selected.id, gpu_worker.id);
    Ok(())
}

#[tokio::test]
#[ignore = "e2e test requires PostgreSQL/TimescaleDB"]
async fn execution_empty_selector_override_clears_action_default() -> anyhow::Result<()> {
    let pool = create_test_pool().await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let pack = create_pack(&pool, &suffix).await?;
    let _gpu_worker = create_worker(
        &pool,
        "gpu",
        json!({"pool": "gpu"}),
        json!([{"key": "dedicated", "effect": "no_schedule"}]),
    )
    .await?;
    let plain_worker = create_worker(&pool, "plain", json!({}), json!([])).await?;
    let action = create_action(
        &pool,
        &pack,
        "clear_selector",
        json!({"pool": "gpu"}),
        json!([]),
        json!({}),
    )
    .await?;

    let execution_id =
        create_execution_with_placement(&pool, &action, Some(json!({})), None, None).await?;
    let selected = selected_worker_for_execution_id(&pool, execution_id).await?;

    assert_eq!(selected.id, plain_worker.id);
    Ok(())
}

#[tokio::test]
#[ignore = "e2e test requires PostgreSQL/TimescaleDB"]
async fn workflow_task_worker_selector_override_controls_child_execution() -> anyhow::Result<()> {
    let pool = create_test_pool().await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let pack = create_pack(&pool, &suffix).await?;
    let _plain_worker = create_worker(&pool, "plain", json!({}), json!([])).await?;
    let workflow_worker = create_worker(
        &pool,
        "workflow",
        json!({"task_pool": "workflow"}),
        json!([]),
    )
    .await?;
    let action = create_action(
        &pool,
        &pack,
        "workflow_task_selector",
        json!({}),
        json!([]),
        json!({}),
    )
    .await?;

    let execution_id = create_workflow_task_execution_with_placement(
        &pool,
        &action,
        Some(json!({"task_pool": "workflow"})),
        None,
        None,
    )
    .await?;
    let selected = selected_worker_for_execution_id(&pool, execution_id).await?;

    assert_eq!(selected.id, workflow_worker.id);
    Ok(())
}

#[tokio::test]
#[ignore = "e2e test requires PostgreSQL/TimescaleDB"]
async fn workflow_task_toleration_override_allows_tainted_child_execution() -> anyhow::Result<()> {
    let pool = create_test_pool().await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let pack = create_pack(&pool, &suffix).await?;
    let _plain_worker = create_worker(&pool, "plain", json!({}), json!([])).await?;
    let isolated_worker = create_worker(
        &pool,
        "isolated",
        json!({"pool": "isolated"}),
        json!([{"key": "dedicated", "value": "workflow", "effect": "no_schedule"}]),
    )
    .await?;
    let action = create_action(
        &pool,
        &pack,
        "workflow_task_toleration",
        json!({}),
        json!([]),
        json!({}),
    )
    .await?;

    let execution_id = create_execution_with_placement(
        &pool,
        &action,
        Some(json!({"pool": "isolated"})),
        Some(json!([{
            "key": "dedicated",
            "operator": "equal",
            "value": "workflow",
            "effect": "no_schedule"
        }])),
        None,
    )
    .await?;
    let selected = selected_worker_for_execution_id(&pool, execution_id).await?;

    assert_eq!(selected.id, isolated_worker.id);
    Ok(())
}
