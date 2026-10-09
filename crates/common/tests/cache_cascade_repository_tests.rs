//! Parent-first repository deletion against real generation partition cleanup.

mod helpers;

use attune_common::{
    config::CacheRetentionConfig,
    models::{
        EnforcementCondition, EnforcementStatus, ExecutionStatus, WorkflowCacheIterationState,
    },
    repositories::{
        cache::{
            CacheEntryInput, CacheEntryRepository, CacheGenerationCleanupOutcome,
            CacheGenerationRepository, CacheIngestRepository, CacheNamespacePolicy,
            CacheNamespaceRepository, CacheOwnerScope, CacheTransactionMode,
            CreateCacheGenerationInput, CreateCacheGenerationResult, CreateCacheNamespaceInput,
        },
        event::{CreateEnforcementInput, EnforcementRepository},
        execution::{CreateExecutionInput, ExecutionRepository},
        retention::RetentionRepository,
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
        workflow_cache_iteration::{
            CreateWorkflowCacheIterationInput, WorkflowCacheIterationRepository,
        },
        Create, FindById, PackRepository,
    },
    test_database::TestDatabase,
};
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use std::time::Duration as StdDuration;

#[derive(Clone, Copy, Debug)]
enum DeleteTarget {
    Pack,
    Definition,
    Workflow,
}

struct Fixture {
    db: TestDatabase,
    pack: i64,
    definition: i64,
    workflow: i64,
    execution: i64,
    generation: i64,
    iterations: Vec<i64>,
}

impl Fixture {
    async fn create() -> Self {
        let db = helpers::create_test_pool().await.unwrap();
        let pool = db.pool();
        // This suite measures delete ordering, not partition creation latency.
        // Allow fixture DDL to finish on the bounded four-thread database lane.
        let mut retention = RetentionRepository::load_config(pool).await.unwrap();
        retention
            .cache_retention
            .ddl_creation_statement_timeout_milliseconds = 30_000;
        RetentionRepository::update_config(pool, &retention)
            .await
            .unwrap();
        let pack = helpers::PackFixture::new_unique("cascade")
            .create(pool)
            .await
            .unwrap();
        let execution = ExecutionRepository::create(
            pool,
            CreateExecutionInput {
                action_ref: format!("{}.workflow", pack.r#ref),
                status: ExecutionStatus::Completed,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let definition = WorkflowDefinitionRepository::create(
            pool,
            CreateWorkflowDefinitionInput {
                r#ref: format!("{}.workflow", pack.r#ref),
                pack: pack.id,
                pack_ref: pack.r#ref,
                label: "Cascade".into(),
                description: None,
                version: "1.0.0".into(),
                param_schema: None,
                out_schema: None,
                definition: json!({}),
                tags: vec![],
            },
        )
        .await
        .unwrap();
        let workflow = WorkflowExecutionRepository::create(
            pool,
            CreateWorkflowExecutionInput {
                execution: execution.id,
                workflow_def: definition.id,
                task_graph: json!({}),
                variables: json!({}),
                status: ExecutionStatus::Running,
            },
        )
        .await
        .unwrap();
        let namespace = CacheNamespaceRepository::create(
            pool,
            CreateCacheNamespaceInput {
                owner: CacheOwnerScope::system(),
                namespace: format!("cascade_{}", helpers::unique_test_id()),
                policy: CacheNamespacePolicy::default(),
            },
        )
        .await
        .unwrap();
        let generation = publish(pool, namespace.id, "first", None).await;
        let mut iterations = vec![];
        let mut pins = pool.begin().await.unwrap();
        CacheEntryRepository::protect_transaction(&mut pins, CacheTransactionMode::PinMutation)
            .await
            .unwrap();
        for task in ["first", "second"] {
            let iteration = WorkflowCacheIterationRepository::create(
                &mut *pins,
                CreateWorkflowCacheIterationInput {
                    workflow_execution: workflow.id,
                    task_name: task.into(),
                    namespace: namespace.id,
                    generation,
                    page_size: 10,
                    batch_size: 1,
                    concurrency: 1,
                },
            )
            .await
            .unwrap();
            iterations.push(iteration.id);
        }
        pins.commit().await.unwrap();
        publish(pool, namespace.id, "replacement", Some(generation)).await;
        sqlx::query("UPDATE cache_generation SET retired=NOW()-INTERVAL '1 day', readable_until=NOW()-INTERVAL '1 hour' WHERE id=$1")
            .bind(generation).execute(pool).await.unwrap();
        Self {
            db,
            pack: pack.id,
            definition: definition.id,
            workflow: workflow.id,
            execution: execution.id,
            generation,
            iterations,
        }
    }

    async fn terminal(&self) {
        let mut pins = self.db.pool().begin().await.unwrap();
        CacheEntryRepository::protect_transaction(&mut pins, CacheTransactionMode::PinMutation)
            .await
            .unwrap();
        for iteration in &self.iterations {
            WorkflowCacheIterationRepository::mark_terminal(
                &mut *pins,
                *iteration,
                WorkflowCacheIterationState::Completed,
                None,
            )
            .await
            .unwrap()
            .unwrap();
        }
        pins.commit().await.unwrap();
    }

    async fn delete(&self, conn: &mut PgConnection, target: DeleteTarget) -> bool {
        match target {
            DeleteTarget::Pack => PackRepository::delete(conn, self.pack).await.unwrap(),
            DeleteTarget::Definition => WorkflowDefinitionRepository::delete(conn, self.definition)
                .await
                .unwrap(),
            DeleteTarget::Workflow => WorkflowExecutionRepository::delete(conn, self.workflow)
                .await
                .unwrap(),
        }
    }
}

async fn publish(pool: &PgPool, namespace: i64, refresh: &str, previous: Option<i64>) -> i64 {
    let result = CacheGenerationRepository::create_or_get(
        pool,
        &CreateCacheGenerationInput {
            namespace,
            client_refresh_id: refresh.into(),
            expected_active_generation: previous,
            expected_chunk_count: 1,
            expected_count: Some(1),
            expected_bytes: None,
            checksum_algorithm: None,
            checksum: None,
            source_revision: None,
            created_by: None,
            created_by_execution: None,
        },
    )
    .await
    .unwrap();
    let CreateCacheGenerationResult::Created(generation) = result else {
        panic!("new generation required")
    };
    CacheIngestRepository::insert_chunk(
        pool,
        generation.id,
        0,
        refresh,
        &[CacheEntryInput {
            external_id: "entry".into(),
            value: json!({"data": "retained"}),
            source_updated_at: None,
            source_checksum: None,
        }],
    )
    .await
    .unwrap();
    CacheGenerationRepository::seal(pool, generation.id)
        .await
        .unwrap();
    CacheGenerationRepository::promote(
        pool,
        namespace,
        generation.id,
        previous,
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap();
    generation.id
}

async fn counters(pool: &PgPool, generation: i64) -> (i64, i64, i64) {
    sqlx::query_as("SELECT retained_iterations, physical_bytes, (SELECT count(*) FROM workflow_cache_iteration WHERE generation=$1) FROM cache_generation_entry_usage WHERE generation=$1")
        .bind(generation).fetch_one(pool).await.unwrap()
}

async fn pid(conn: &mut PgConnection) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(conn)
        .await
        .unwrap()
}

async fn parent_wait(pool: &PgPool, waiter: i32, blocker: i32, mode: &str) {
    tokio::time::timeout(StdDuration::from_secs(15), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation='cache_entry'::regclass AND mode=$3 AND NOT granted) AND $2=ANY(pg_blocking_pids($1))")
                .bind(waiter).bind(blocker).bind(mode).fetch_one(pool).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("repository did not reach the expected parent lock wait");
}

async fn mutation_wait(pool: &PgPool, waiter: i32, blocker: i32) {
    tokio::time::timeout(StdDuration::from_secs(15), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND classid=7821101 AND objid=0 AND objsubid=2 AND NOT granted) AND $2=ANY(pg_blocking_pids($1))")
                .bind(waiter).bind(blocker).fetch_one(pool).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("repository did not reach the expected pin-mutation gate wait");
}

async fn cascade_locks(pool: &PgPool, backend: i32) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE pid=$1 AND granted AND relation IN ('pack'::regclass,'pack_release'::regclass,'workflow_definition'::regclass,'workflow_execution'::regclass,'workflow_cache_iteration'::regclass,'cache_generation_entry_usage'::regclass)")
        .bind(backend).fetch_one(pool).await.unwrap()
}

async fn assert_cleanup_first(target: DeleteTarget) {
    let fixture = Fixture::create().await;
    fixture.terminal().await;
    let pool = fixture.db.pool();
    let mut cleanup = pool.begin().await.unwrap();
    let outcome: String =
        sqlx::query_scalar("SELECT outcome FROM drop_cleanup_cache_generation($1,0)")
            .bind(fixture.generation)
            .fetch_one(&mut *cleanup)
            .await
            .unwrap();
    assert_eq!(outcome, "dropped");
    let cleanup_pid = pid(&mut cleanup).await;
    let mut delete = pool.begin().await.unwrap();
    let delete_pid = pid(&mut delete).await;
    let observation = async {
        mutation_wait(pool, delete_pid, cleanup_pid).await;
        let early_rows = cascade_locks(pool, delete_pid).await;
        cleanup.commit().await.unwrap();
        early_rows
    };
    let (deleted, early_rows) = tokio::join!(fixture.delete(&mut delete, target), observation);
    assert!(deleted);
    assert_eq!(
        early_rows, 0,
        "{target:?} acquired cascade rows before parent protection"
    );
    delete.commit().await.unwrap();
    assert!(
        CacheGenerationRepository::find_by_id(pool, fixture.generation)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        WorkflowExecutionRepository::find_by_id(pool, fixture.workflow)
            .await
            .unwrap()
            .is_none()
    );
    fixture.db.cleanup().await.unwrap();
}

async fn assert_delete_first(target: DeleteTarget) {
    let fixture = Fixture::create().await;
    fixture.terminal().await;
    let pool = fixture.db.pool();
    let before = counters(pool, fixture.generation).await;
    assert_eq!((before.0, before.2), (2, 2));
    let mut deletion = pool.begin().await.unwrap();
    let delete_pid = pid(&mut deletion).await;
    assert!(fixture.delete(&mut deletion, target).await);
    let mut cleanup = pool.begin().await.unwrap();
    let cleanup_pid = pid(&mut cleanup).await;
    let drop = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT outcome,records_reclaimed,bytes_reclaimed FROM drop_cleanup_cache_generation($1,0)",
    )
    .bind(fixture.generation)
    .fetch_one(&mut *cleanup);
    let observation = async {
        mutation_wait(pool, cleanup_pid, delete_pid).await;
        assert_eq!(
            counters(pool, fixture.generation).await,
            before,
            "delete must remain invisible before commit"
        );
        deletion.commit().await.unwrap();
    };
    let (dropped, ()) = tokio::join!(drop, observation);
    let (outcome, records, bytes) = dropped.unwrap();
    assert_eq!(outcome, "dropped");
    assert_eq!(records, 1);
    assert_eq!(bytes, before.1);
    cleanup.commit().await.unwrap();
    assert!(
        WorkflowExecutionRepository::find_by_id(pool, fixture.workflow)
            .await
            .unwrap()
            .is_none()
    );
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn pack_cleanup_first() {
    assert_cleanup_first(DeleteTarget::Pack).await;
}
#[tokio::test]
async fn definition_cleanup_first() {
    assert_cleanup_first(DeleteTarget::Definition).await;
}
#[tokio::test]
async fn workflow_cleanup_first() {
    assert_cleanup_first(DeleteTarget::Workflow).await;
}
#[tokio::test]
async fn pack_delete_first() {
    assert_delete_first(DeleteTarget::Pack).await;
}
#[tokio::test]
async fn definition_delete_first() {
    assert_delete_first(DeleteTarget::Definition).await;
}
#[tokio::test]
async fn workflow_delete_first() {
    assert_delete_first(DeleteTarget::Workflow).await;
}

#[tokio::test]
async fn pool_deletes_release_counters_without_reclaiming_storage() {
    for target in [
        DeleteTarget::Pack,
        DeleteTarget::Definition,
        DeleteTarget::Workflow,
    ] {
        let fixture = Fixture::create().await;
        let pool = fixture.db.pool();
        let before = counters(pool, fixture.generation).await;
        let deleted = match target {
            DeleteTarget::Pack => PackRepository::delete(pool, fixture.pack).await.unwrap(),
            DeleteTarget::Definition => {
                WorkflowDefinitionRepository::delete(pool, fixture.definition)
                    .await
                    .unwrap()
            }
            DeleteTarget::Workflow => WorkflowExecutionRepository::delete(pool, fixture.workflow)
                .await
                .unwrap(),
        };
        assert!(deleted);
        assert_eq!(counters(pool, fixture.generation).await, (0, before.1, 0));
        assert!(
            CacheGenerationRepository::find_by_id(pool, fixture.generation)
                .await
                .unwrap()
                .is_some()
        );
        fixture.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn rollback_restores_cascade_and_active_pin_protection() {
    for target in [
        DeleteTarget::Pack,
        DeleteTarget::Definition,
        DeleteTarget::Workflow,
    ] {
        let fixture = Fixture::create().await;
        let pool = fixture.db.pool();
        let before = counters(pool, fixture.generation).await;
        let config = CacheRetentionConfig {
            min_traversal_window_seconds: 0,
            ..Default::default()
        };
        assert!(matches!(
            CacheGenerationRepository::drop_if_cleanup_eligible(pool, fixture.generation, &config)
                .await
                .unwrap(),
            CacheGenerationCleanupOutcome::Ineligible
        ));
        let mut deletion = pool.begin().await.unwrap();
        assert!(fixture.delete(&mut deletion, target).await);
        let retained: i64 = sqlx::query_scalar(
            "SELECT retained_iterations FROM cache_generation_entry_usage WHERE generation=$1",
        )
        .bind(fixture.generation)
        .fetch_one(&mut *deletion)
        .await
        .unwrap();
        assert_eq!(retained, 0);
        deletion.rollback().await.unwrap();
        assert_eq!(counters(pool, fixture.generation).await, before);
        assert!(
            WorkflowExecutionRepository::find_by_id(pool, fixture.workflow)
                .await
                .unwrap()
                .is_some()
        );
        assert!(matches!(
            CacheGenerationRepository::drop_if_cleanup_eligible(pool, fixture.generation, &config)
                .await
                .unwrap(),
            CacheGenerationCleanupOutcome::Ineligible
        ));
        fixture.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn execution_deletion_keeps_workflow_and_active_pin() {
    let fixture = Fixture::create().await;
    let pool = fixture.db.pool();
    let before = counters(pool, fixture.generation).await;
    use attune_common::repositories::Delete;
    assert!(ExecutionRepository::delete(pool, fixture.execution)
        .await
        .unwrap());
    assert!(
        WorkflowExecutionRepository::find_by_id(pool, fixture.workflow)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(counters(pool, fixture.generation).await, before);
    let config = CacheRetentionConfig {
        min_traversal_window_seconds: 0,
        ..Default::default()
    };
    assert!(matches!(
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, fixture.generation, &config)
            .await
            .unwrap(),
        CacheGenerationCleanupOutcome::Ineligible
    ));
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cleanup_rollback_restores_storage_before_waiting_workflow_delete() {
    let fixture = Fixture::create().await;
    fixture.terminal().await;
    let pool = fixture.db.pool();
    let before = counters(pool, fixture.generation).await;
    let mut cleanup = pool.begin().await.unwrap();
    let outcome: String =
        sqlx::query_scalar("SELECT outcome FROM drop_cleanup_cache_generation($1,0)")
            .bind(fixture.generation)
            .fetch_one(&mut *cleanup)
            .await
            .unwrap();
    assert_eq!(outcome, "dropped");
    let cleanup_pid = pid(&mut cleanup).await;
    let mut deletion = pool.begin().await.unwrap();
    let deletion_pid = pid(&mut deletion).await;
    let observation = async {
        mutation_wait(pool, deletion_pid, cleanup_pid).await;
        cleanup.rollback().await.unwrap();
    };
    let (deleted, ()) = tokio::join!(
        fixture.delete(&mut deletion, DeleteTarget::Workflow),
        observation
    );
    assert!(deleted);
    deletion.commit().await.unwrap();
    assert_eq!(counters(pool, fixture.generation).await, (0, before.1, 0));
    assert!(
        CacheGenerationRepository::find_by_id(pool, fixture.generation)
            .await
            .unwrap()
            .is_some()
    );
    let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM cache_entry WHERE generation=$1")
        .bind(fixture.generation)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        entries, 1,
        "rolled-back partition DDL must restore the physical entries"
    );
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn execution_root_deletion_preserves_child_parent_id() {
    let fixture = Fixture::create().await;
    let pool = fixture.db.pool();
    let child = ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action_ref: "cascade.child".into(),
            parent: Some(fixture.execution),
            status: ExecutionStatus::Completed,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    use attune_common::repositories::Delete;
    assert!(ExecutionRepository::delete(pool, fixture.execution)
        .await
        .unwrap());
    assert!(ExecutionRepository::find_by_id(pool, fixture.execution)
        .await
        .unwrap()
        .is_none());
    let retained = ExecutionRepository::find_by_id(pool, child.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.parent, Some(fixture.execution));
    assert_eq!(retained.status, child.status);
    assert!(
        WorkflowExecutionRepository::find_by_id(pool, fixture.workflow)
            .await
            .unwrap()
            .is_some()
    );
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn enforcement_deletion_preserves_execution_enforcement_id() {
    let db = helpers::create_test_pool().await.unwrap();
    let pool = db.pool();
    let enforcement = EnforcementRepository::create(
        pool,
        CreateEnforcementInput {
            rule: None,
            rule_ref: "cascade.rule".into(),
            trigger_ref: "cascade.trigger".into(),
            config: None,
            event: None,
            status: EnforcementStatus::Processed,
            payload: json!({}),
            condition: EnforcementCondition::All,
            conditions: json!([]),
        },
    )
    .await
    .unwrap();
    let execution = ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action_ref: "cascade.enforced".into(),
            enforcement: Some(enforcement.id),
            status: ExecutionStatus::Completed,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    use attune_common::repositories::Delete;
    assert!(EnforcementRepository::delete(pool, enforcement.id)
        .await
        .unwrap());
    assert!(EnforcementRepository::find_by_id(pool, enforcement.id)
        .await
        .unwrap()
        .is_none());
    let retained = ExecutionRepository::find_by_id(pool, execution.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.enforcement, Some(enforcement.id));
    assert_eq!(retained.status, execution.status);
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn ordinary_reads_remain_concurrent_with_pin_mutation() {
    let fixture = Fixture::create().await;
    let pool = fixture.db.pool();
    let namespace = CacheGenerationRepository::find_by_id(pool, fixture.generation)
        .await
        .unwrap()
        .unwrap()
        .namespace;
    let mut mutation = pool.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut mutation, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut reader = pool.begin().await.unwrap();
    let reader_pid = pid(&mut reader).await;
    tokio::time::timeout(
        StdDuration::from_secs(15),
        CacheEntryRepository::protect_transaction(&mut reader, CacheTransactionMode::Read),
    )
    .await
    .expect("ordinary Read must not wait for mutation admission")
    .unwrap();
    let gate_locks: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND classid=7821101 AND objid=0 AND objsubid=2")
        .bind(reader_pid).fetch_one(pool).await.unwrap();
    assert_eq!(
        gate_locks, 0,
        "ordinary Read must not acquire mutation admission"
    );
    let entry = tokio::time::timeout(
        StdDuration::from_secs(15),
        CacheEntryRepository::find_active(pool, namespace, "entry"),
    )
    .await
    .expect("production cache reads must proceed while pin mutation is open")
    .unwrap()
    .unwrap();
    assert_eq!(entry.value, json!({"data": "retained"}));
    reader.commit().await.unwrap();
    mutation.rollback().await.unwrap();
    fixture.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cascade_parent_protection_precedes_source_rows_after_mutation_admission() {
    for target in [
        DeleteTarget::Pack,
        DeleteTarget::Definition,
        DeleteTarget::Workflow,
    ] {
        let fixture = Fixture::create().await;
        let pool = fixture.db.pool();
        let mut parent = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE ONLY cache_entry IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *parent)
            .await
            .unwrap();
        let parent_pid = pid(&mut parent).await;
        let mut deletion = pool.begin().await.unwrap();
        let deletion_pid = pid(&mut deletion).await;
        let observation = async {
            parent_wait(pool, deletion_pid, parent_pid, "AccessShareLock").await;
            assert_eq!(cascade_locks(pool, deletion_pid).await, 0);
            let admitted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND classid=7821101 AND objid=0 AND objsubid=2 AND granted)")
                .bind(deletion_pid).fetch_one(pool).await.unwrap();
            assert!(
                admitted,
                "cascade protection must acquire admission before the parent"
            );
            parent.commit().await.unwrap();
        };
        let (deleted, ()) = tokio::join!(fixture.delete(&mut deletion, target), observation);
        assert!(deleted);
        deletion.commit().await.unwrap();
        assert_eq!(counters(pool, fixture.generation).await.0, 0);
        fixture.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn multi_generation_cascades_wait_before_rows_for_reverse_order_pin_mutation() {
    for target in [
        DeleteTarget::Pack,
        DeleteTarget::Definition,
        DeleteTarget::Workflow,
    ] {
        let fixture = Fixture::create().await;
        let pool = fixture.db.pool();
        let namespace = CacheGenerationRepository::find_by_id(pool, fixture.generation)
            .await
            .unwrap()
            .unwrap()
            .namespace;
        let generation_b = CacheNamespaceRepository::find_by_id(pool, namespace)
            .await
            .unwrap()
            .unwrap()
            .active_generation
            .unwrap();
        let second_execution = ExecutionRepository::create(
            pool,
            CreateExecutionInput {
                action_ref: "cascade.pinner".into(),
                status: ExecutionStatus::Completed,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let second_workflow = WorkflowExecutionRepository::create(
            pool,
            CreateWorkflowExecutionInput {
                execution: second_execution.id,
                workflow_def: fixture.definition,
                task_graph: json!({}),
                variables: json!({}),
                status: ExecutionStatus::Running,
            },
        )
        .await
        .unwrap();
        let input =
            |workflow_execution, generation, task_name: &str| CreateWorkflowCacheIterationInput {
                workflow_execution,
                generation,
                namespace,
                task_name: task_name.into(),
                page_size: 10,
                batch_size: 1,
                concurrency: 1,
            };
        let mut initial = pool.begin().await.unwrap();
        CacheEntryRepository::protect_transaction(&mut initial, CacheTransactionMode::PinMutation)
            .await
            .unwrap();
        WorkflowCacheIterationRepository::create(
            &mut *initial,
            input(fixture.workflow, generation_b, "generation-b"),
        )
        .await
        .unwrap();
        initial.commit().await.unwrap();
        let before_a = counters(pool, fixture.generation).await;
        let before_b = counters(pool, generation_b).await;
        assert_eq!((before_a.0, before_b.0), (2, 1));

        let mut pinning = pool.begin().await.unwrap();
        CacheEntryRepository::protect_transaction(&mut pinning, CacheTransactionMode::PinMutation)
            .await
            .unwrap();
        WorkflowCacheIterationRepository::create(
            &mut *pinning,
            input(second_workflow.id, generation_b, "b-first"),
        )
        .await
        .unwrap();
        let pinning_pid = pid(&mut pinning).await;
        let mut deletion = pool.begin().await.unwrap();
        let deletion_pid = pid(&mut deletion).await;
        let observation = async {
            mutation_wait(pool, deletion_pid, pinning_pid).await;
            assert_eq!(
                cascade_locks(pool, deletion_pid).await,
                0,
                "a waiting cascade must not lock source or usage rows"
            );
            let parent_locks: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks WHERE pid=$1 AND relation='cache_entry'::regclass",
            )
            .bind(deletion_pid)
            .fetch_one(pool)
            .await
            .unwrap();
            assert_eq!(parent_locks, 0, "admission must precede parent protection");
            // Without admission, deletion takes A then waits on B while this
            // transaction takes B then waits on A, reproducing the usage cycle.
            WorkflowCacheIterationRepository::create(
                &mut *pinning,
                input(second_workflow.id, fixture.generation, "a-second"),
            )
            .await
            .unwrap();
            pinning.commit().await.unwrap();
        };
        let (deleted, ()) = tokio::join!(fixture.delete(&mut deletion, target), observation);
        assert!(deleted);
        deletion.commit().await.unwrap();
        let retained = if matches!(target, DeleteTarget::Workflow) {
            1
        } else {
            0
        };
        assert_eq!(
            counters(pool, fixture.generation).await,
            (retained, before_a.1, retained)
        );
        assert_eq!(
            counters(pool, generation_b).await,
            (retained, before_b.1, retained)
        );
        assert_eq!(
            WorkflowExecutionRepository::find_by_id(pool, second_workflow.id)
                .await
                .unwrap()
                .is_some(),
            matches!(target, DeleteTarget::Workflow)
        );
        fixture.db.cleanup().await.unwrap();
    }
}
