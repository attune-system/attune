// Database integration tests for the production scheduler's cache helpers.
//
// Include inside scheduler::tests to retain access to private scheduler functions
// without making execution internals public. These test actual initialization
// helpers and transaction entry. Broker fixtures also call production wrappers
// and verify actual publication. All external resources belong to the run.
use attune_common::{
    config::CacheRetentionConfig,
    repositories::{
        cache::{
            CacheEntryInput, CacheGenerationCleanupOutcome, CacheIngestRepository,
            CacheNamespacePolicy, CreateCacheGenerationInput, CreateCacheGenerationResult,
            CreateCacheNamespaceInput,
        },
        workflow_cache_iteration::CreateWorkflowCacheIterationInput,
        Delete,
    },
};

struct CachePartitionFixture {
    database: TestDatabase,
    parent: Execution,
    workflow_execution_id: i64,
    graph: TaskGraph,
    context: WorkflowContext,
}

impl CachePartitionFixture {
    async fn create() -> Self {
        let config = Config::load_from_file(&format!(
            "{}/../../config.test.yaml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let database = TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop();
        let pool = database.pool();
        let pack = PackRepository::create(
            pool,
            CreatePackInput {
                r#ref: "cache_partition_it".into(),
                label: "Cache partition test".into(),
                description: None,
                version: "1.0.0".into(),
                conf_schema: serde_json::json!({}),
                config: serde_json::json!({}),
                meta: serde_json::json!({}),
                tags: vec![],
                runtime_deps: vec![],
                dependencies: vec![],
                is_standard: false,
                installers: serde_json::json!({}),
            },
        )
        .await
        .unwrap();
        let mut actions = Vec::new();
        for name in ["workflow", "consume"] {
            actions.push(
                ActionRepository::create(
                    pool,
                    CreateActionInput {
                        r#ref: format!("{}.{}", pack.r#ref, name),
                        pack: pack.id,
                        pack_ref: pack.r#ref.clone(),
                        label: name.into(),
                        description: None,
                        entrypoint: format!("{name}.sh"),
                        runtime: None,
                        enabled: true,
                        runtime_version_constraint: None,
                        required_worker_runtimes: serde_json::json!({}),
                        worker_selector: serde_json::json!({}),
                        worker_tolerations: serde_json::json!([]),
                        worker_affinity: serde_json::json!({}),
                        param_schema: None,
                        out_schema: None,
                        is_adhoc: false,
                        accesses_mcp: false,
                        default_execution_permission_set_refs: vec!["standard".into()],
                        reference_visibility: Default::default(),
                        reference_allowed_pack_refs: vec![],
                        log_retention_policy: None,
                        log_retention_limit: None,
                        artifact_retention_policy: None,
                        artifact_retention_limit: None,
                        timeout_seconds: None,
                    },
                )
                .await
                .unwrap(),
            );
        }
        let workflow = attune_common::workflow::parse_workflow_yaml(
            "name: cache_partition_workflow\nversion: 1.0.0\ntasks:\n  - name: consume\n    action: cache_partition_it.consume\n    permission_set_refs: [standard]\n    input:\n      entry: '{{ item }}'\n    concurrency: 1\n    iterate_cache:\n      owner_type: pack\n      owner_ref: cache_partition_it\n      namespace: partition_ordering\n      generation: '{{ parameters.generation }}'\n      page_size: 1\n",
        ).unwrap();
        let graph = TaskGraph::from_workflow(&workflow).unwrap();
        let definition = WorkflowDefinitionRepository::create(
            pool,
            CreateWorkflowDefinitionInput {
                r#ref: actions[0].r#ref.clone(),
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                label: "Cache partition workflow".into(),
                description: None,
                version: "1.0.0".into(),
                param_schema: None,
                out_schema: None,
                definition: serde_json::to_value(&workflow).unwrap(),
                tags: vec![],
            },
        )
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let release = PackReleaseRepository::create_or_get(
            &mut tx,
            CreatePackReleaseInput {
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                version: "1.0.0".into(),
                digest: "b".repeat(64),
                object_key: "cache-partition-integration".into(),
                provider_version: "test".into(),
                content_path: "/tmp/opencode/cache-partition-integration".into(),
                archive_size: 1,
                manifest: serde_json::json!({}),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let release = PackReleasePin {
            id: release.id,
            digest: release.digest,
            content_path: release.content_path,
        };
        let child_executable = ActionExecutableSnapshot {
            action: actions[1].clone(),
            runtime: None,
            runtime_versions: vec![],
            workflow_definition: None,
        };
        let snapshot = ExecutionExecutableSnapshot {
            release: release.clone(),
            executable: ActionExecutableSnapshot {
                action: actions[0].clone(),
                runtime: None,
                runtime_versions: vec![],
                workflow_definition: Some(definition.clone()),
            },
            pack_executables: BTreeMap::from([(
                actions[1].r#ref.clone(),
                ReleasedActionExecutableSnapshot {
                    release,
                    executable: child_executable,
                },
            )]),
        };
        let identity = IdentityRepository::create(
            pool,
            CreateIdentityInput {
                login: "cache-partition-owner".into(),
                display_name: None,
                password_hash: None,
                attributes: serde_json::json!({}),
            },
        )
        .await
        .unwrap();
        let parameters = serde_json::json!({"generation": "active"});
        let parent = ExecutionRepository::create_pinned(
            pool,
            CreateExecutionInput {
                action: Some(actions[0].id),
                action_ref: actions[0].r#ref.clone(),
                config: Some(parameters.clone()),
                executor: Some(identity.id),
                permission_set_refs: vec!["standard".into()],
                status: ExecutionStatus::Running,
                ..Default::default()
            },
            &snapshot,
        )
        .await
        .unwrap();
        let workflow_execution = WorkflowExecutionRepository::create(
            pool,
            CreateWorkflowExecutionInput {
                execution: parent.id,
                workflow_def: definition.id,
                task_graph: serde_json::to_value(&graph).unwrap(),
                variables: serde_json::json!({}),
                status: ExecutionStatus::Running,
            },
        )
        .await
        .unwrap();
        Self {
            database,
            parent,
            workflow_execution_id: workflow_execution.id,
            graph,
            context: WorkflowContext::new(parameters, HashMap::new()),
        }
    }

    fn task(&self) -> TaskNode {
        self.graph.get_task("consume").unwrap().clone()
    }
}

async fn seed_cache(fixture: &CachePartitionFixture) -> i64 {
    let pool = fixture.database.pool();
    let action = fixture
        .parent
        .executable_snapshot
        .as_ref()
        .unwrap()
        .executable
        .action
        .clone();
    let namespace = CacheNamespaceRepository::create(
        pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::pack(action.pack, Some(action.pack_ref)),
            namespace: "partition_ordering".into(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap();
    let CreateCacheGenerationResult::Created(generation) =
        CacheGenerationRepository::create_or_get(
            pool,
            &CreateCacheGenerationInput {
                namespace: namespace.id,
                client_refresh_id: "partition-ordering".into(),
                expected_active_generation: None,
                expected_chunk_count: 1,
                expected_count: Some(3),
                expected_bytes: None,
                checksum_algorithm: None,
                checksum: None,
                source_revision: None,
                created_by: None,
                created_by_execution: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("fixture generation must be new");
    };
    CacheIngestRepository::insert_chunk(
        pool,
        generation.id,
        0,
        "partition-ordering-chunk",
        &["a", "b", "c"]
            .into_iter()
            .map(|id| CacheEntryInput {
                external_id: id.into(),
                value: serde_json::json!({"id": id}),
                source_updated_at: None,
                source_checksum: None,
            })
            .collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    CacheGenerationRepository::seal(pool, generation.id)
        .await
        .unwrap();
    CacheGenerationRepository::promote(
        pool,
        namespace.id,
        generation.id,
        None,
        Utc::now() + chrono::Duration::hours(1),
    )
    .await
    .unwrap();
    generation.id
}

async fn retire_generation(fixture: &CachePartitionFixture, generation: i64) {
    let pool = fixture.database.pool();
    let original = CacheGenerationRepository::find_by_id(pool, generation)
        .await
        .unwrap()
        .unwrap();
    let CreateCacheGenerationResult::Created(replacement) =
        CacheGenerationRepository::create_or_get(
            pool,
            &CreateCacheGenerationInput {
                namespace: original.namespace,
                client_refresh_id: "replacement".into(),
                expected_active_generation: Some(generation),
                expected_chunk_count: 0,
                expected_count: Some(0),
                expected_bytes: None,
                checksum_algorithm: None,
                checksum: None,
                source_revision: None,
                created_by: None,
                created_by_execution: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!("replacement must be new");
    };
    CacheGenerationRepository::seal(pool, replacement.id)
        .await
        .unwrap();
    CacheGenerationRepository::promote(
        pool,
        original.namespace,
        replacement.id,
        Some(generation),
        Utc::now() + chrono::Duration::hours(1),
    )
    .await
    .unwrap();
}

fn cache_task(fixture: &CachePartitionFixture, generation: i64) -> TaskNode {
    let mut task = fixture.task();
    task.wait_for = None;
    task.iterate_cache = Some(
        serde_json::from_value(serde_json::json!({
            "owner_type": "pack",
            "owner_ref": "cache_partition_it",
            "namespace": "partition_ordering",
            "generation": generation.to_string(),
            "page_size": 1,
        }))
        .unwrap(),
    );
    task.permission_set_refs = Some(serde_json::json!(["standard"]));
    task.concurrency = Some(1);
    task
}

async fn dispatch(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    fixture: &CachePartitionFixture,
    parent: &Execution,
    task: &TaskNode,
) -> (
    Vec<PendingExecutionRequested>,
    Vec<PendingExecutionCompleted>,
) {
    // Same pre-row protection as activate_entry_workflow_task. This does not
    // replace a broker-backed test of that outer transaction-entry function.
    CacheEntryRepository::protect_transaction(transaction, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    WorkflowExecutionRepository::find_by_id_for_update(
        &mut **transaction,
        fixture.workflow_execution_id,
    )
    .await
    .unwrap()
    .unwrap();
    let mut messages = Vec::new();
    let mut completions = Vec::new();
    ExecutionScheduler::activate_workflow_task_with_conn(
        transaction,
        &AtomicUsize::new(0),
        parent,
        &fixture.workflow_execution_id,
        task,
        &fixture.context,
        None,
        None,
        &mut messages,
        &mut completions,
    )
    .await
    .unwrap();
    (messages, completions)
}

async fn wait_for_parent_lock(pool: &PgPool, mode: &str, granted: bool, blocker: i32) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let seen: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks \
                 WHERE relation = 'cache_entry'::regclass AND mode = $1 AND granted = $2 \
                 AND ($3 = ANY(pg_blocking_pids(pid))))",
            )
            .bind(mode)
            .bind(granted)
            .bind(blocker)
            .fetch_one(pool)
            .await
            .unwrap();
            if seen {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("expected cache parent lock waiter was not observed");
}

async fn wait_for_backend_blocked(pool: &PgPool, waiting: i32, blocker: i32) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let seen: bool = sqlx::query_scalar("SELECT $2=ANY(pg_blocking_pids($1))")
                .bind(waiting)
                .bind(blocker)
                .fetch_one(pool)
                .await
                .unwrap();
            if seen {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("expected owned backend lock wait was not observed");
}

async fn wait_for_mutation_admission(pool: &PgPool, blocker: i32) -> i32 {
    let mut connection = pool.acquire().await.unwrap();
    wait_for_mutation_admission_on_connection(&mut connection, blocker).await
}

async fn wait_for_mutation_admission_on_connection(
    connection: &mut sqlx::PgConnection,
    blocker: i32,
) -> i32 {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_locks WHERE locktype='advisory' AND classid=7821101 AND objid=0 \
                 AND objsubid=2 AND mode='ExclusiveLock' AND NOT granted AND $1=ANY(pg_blocking_pids(pid))",
            ).bind(blocker).fetch_optional(&mut *connection).await.unwrap();
            if let Some(pid) = waiting { return pid; }
            tokio::task::yield_now().await;
        }
    }).await.expect("expected cache mutation admission waiter was not observed")
}

#[tokio::test]
async fn workflow_first_commits_scanning_pin_before_cleanup_rechecks_eligibility() {
    let fixture = CachePartitionFixture::create().await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    retire_generation(&fixture, generation).await;
    let parent = fixture.parent.clone();
    let task = cache_task(&fixture, generation);
    let mut workflow = pool.begin().await.unwrap();
    let (messages, completions) = dispatch(&mut workflow, &fixture, &parent, &task).await;
    assert_eq!(messages.len(), 1);
    assert!(completions.is_empty());
    // Advance this owned fixture's expiry after the scanner has selected the
    // unexpired retired generation. No wall-clock delay is needed.
    sqlx::query(
        "UPDATE cache_generation SET readable_until = NOW() - INTERVAL '1 second' WHERE id = $1",
    )
    .bind(generation)
    .execute(&mut *workflow)
    .await
    .unwrap();
    let workflow_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *workflow)
        .await
        .unwrap();
    let config = CacheRetentionConfig {
        min_traversal_window_seconds: 0,
        ..Default::default()
    };
    // Connection startup is fixture setup, not part of the maintenance lock SLA.
    let mut observer = pool.acquire().await.unwrap();
    let cleanup = CacheGenerationRepository::drop_if_cleanup_eligible(pool, generation, &config);
    let observed = async {
        wait_for_mutation_admission_on_connection(&mut observer, workflow_pid).await;
        workflow.commit().await.unwrap();
    };
    let (outcome, ()) = tokio::join!(cleanup, observed);
    drop(observer);
    assert_eq!(outcome.unwrap(), CacheGenerationCleanupOutcome::Ineligible);
    let iteration = WorkflowCacheIterationRepository::find_by_workflow_task(
        pool,
        fixture.workflow_execution_id,
        &task.name,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(iteration.state, WorkflowCacheIterationState::Scanning);
    assert_eq!(iteration.generation, generation);
    assert_eq!(iteration.dispatched_count, 1);
    assert_eq!(iteration.last_external_id.as_deref(), Some("a"));
    assert!(CacheGenerationRepository::find_by_id(pool, generation)
        .await
        .unwrap()
        .is_some());
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
async fn cleanup_first_blocks_dispatch_before_workflow_and_iteration_row_locks() {
    let fixture = CachePartitionFixture::create().await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    retire_generation(&fixture, generation).await;
    sqlx::query(
        "UPDATE cache_generation SET readable_until = NOW() - INTERVAL '1 second' WHERE id = $1",
    )
    .bind(generation)
    .execute(pool)
    .await
    .unwrap();
    let parent = fixture.parent.clone();
    let task = cache_task(&fixture, generation);
    let mut cleanup = pool.begin().await.unwrap();
    let cleanup_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *cleanup)
        .await
        .unwrap();
    let reclaimed: (String, i64, i64) = sqlx::query_as(
        "SELECT outcome, records_reclaimed, bytes_reclaimed FROM drop_cleanup_cache_generation($1, 0)",
    )
    .bind(generation)
    .fetch_one(&mut *cleanup)
    .await
    .unwrap();
    assert_eq!(reclaimed.0, "dropped");
    assert_eq!(reclaimed.1, 3);
    assert!(reclaimed.2 > 0);
    let mut workflow = pool.begin().await.unwrap();
    let workflow_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *workflow)
        .await
        .unwrap();
    let activation = dispatch(&mut workflow, &fixture, &parent, &task);
    let observed = async {
        assert_eq!(
            wait_for_mutation_admission(pool, cleanup_pid).await,
            workflow_pid
        );
        let row_relations_locked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_locks WHERE pid = $1 AND granted \
             AND relation IN ('cache_entry'::regclass, 'workflow_execution'::regclass, 'workflow_cache_iteration'::regclass)",
        )
        .bind(workflow_pid)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(row_relations_locked, 0);
        cleanup.commit().await.unwrap();
    };
    let ((messages, completions), ()) = tokio::join!(activation, observed);
    assert!(messages.is_empty());
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].status, ExecutionStatus::Failed);
    workflow.commit().await.unwrap();
    assert!(CacheGenerationRepository::find_by_id(pool, generation)
        .await
        .unwrap()
        .is_none());
    assert!(WorkflowCacheIterationRepository::find_by_workflow_task(
        pool,
        fixture.workflow_execution_id,
        &task.name
    )
    .await
    .unwrap()
    .is_none());
    fixture.database.cleanup().await.unwrap();
}

async fn expire_retired_cache(pool: &PgPool, generation: i64) {
    sqlx::query(
        "UPDATE cache_generation SET readable_until = NOW() - INTERVAL '1 second' WHERE id = $1",
    )
    .bind(generation)
    .execute(pool)
    .await
    .unwrap();
}

async fn finish_cache_child(
    fixture: &CachePartitionFixture,
    child_id: i64,
    status: ExecutionStatus,
) -> WorkflowAdvanceOutcome {
    let child = ExecutionRepository::update(
        fixture.database.pool(),
        child_id,
        UpdateExecutionInput {
            status: Some(status),
            result: Some(serde_json::json!({"processed": true})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tx = fixture.database.pool().begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let outcome = ExecutionScheduler::advance_workflow_serialized(
        &mut tx,
        &AtomicUsize::new(0),
        None,
        &child,
        &SchedulerMetadataCaches::new(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    outcome
}

async fn cache_iteration(fixture: &CachePartitionFixture) -> WorkflowCacheIteration {
    WorkflowCacheIterationRepository::find_by_workflow_task(
        fixture.database.pool(),
        fixture.workflow_execution_id,
        "consume",
    )
    .await
    .unwrap()
    .unwrap()
}

async fn reclaim_terminal_cache(fixture: &CachePartitionFixture, generation: i64) {
    let pool = fixture.database.pool();
    let bytes: i64 = sqlx::query_scalar(
        "SELECT physical_bytes FROM cache_generation_entry_usage WHERE generation=$1",
    )
    .bind(generation)
    .fetch_one(pool)
    .await
    .unwrap();
    let config = CacheRetentionConfig {
        min_traversal_window_seconds: 0,
        ..Default::default()
    };
    assert_eq!(
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, generation, &config)
            .await
            .unwrap(),
        CacheGenerationCleanupOutcome::Dropped {
            records: 3,
            bytes: bytes as u64
        }
    );
    let remaining: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM workflow_cache_iteration WHERE generation=$1), \
         (SELECT COUNT(*) FROM cache_generation_entry_usage WHERE generation=$1), \
         (SELECT COUNT(*) FROM cache_ingest_chunk WHERE generation=$1)",
    )
    .bind(generation)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(remaining, (0, 0, 0));
    assert_eq!(
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, generation, &config)
            .await
            .unwrap(),
        CacheGenerationCleanupOutcome::Absent
    );
    let charged: i64 = sqlx::query_scalar(
        "SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id=1",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(charged, 0, "empty replacement has no charged entries");
}

#[tokio::test]
async fn cache_refill_keeps_expired_generation_pinned_until_completion_and_cascade() {
    let fixture = CachePartitionFixture::create().await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    let mut tx = pool.begin().await.unwrap();
    let (messages, completions) =
        dispatch(&mut tx, &fixture, &fixture.parent, &fixture.task()).await;
    tx.commit().await.unwrap();
    assert_eq!(messages.len(), 1);
    assert!(completions.is_empty());
    retire_generation(&fixture, generation).await;
    expire_retired_cache(pool, generation).await;
    let config = CacheRetentionConfig {
        min_traversal_window_seconds: 0,
        ..Default::default()
    };
    assert_eq!(
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, generation, &config)
            .await
            .unwrap(),
        CacheGenerationCleanupOutcome::Ineligible
    );
    let mut child = messages[0].execution_id;
    for (index, expected_id) in ["a", "b", "c"].into_iter().enumerate() {
        let execution = ExecutionRepository::find_by_id(pool, child)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            execution.config.as_ref().unwrap()["entry"]["external_id"],
            expected_id
        );
        assert_eq!(
            execution.workflow_task.as_ref().unwrap().task_index,
            Some(index as i32)
        );
        let outcome = finish_cache_child(&fixture, child, ExecutionStatus::Completed).await;
        let iteration = cache_iteration(&fixture).await;
        assert_eq!(
            iteration.generation, generation,
            "refill never switches to the replacement"
        );
        if index < 2 {
            assert_eq!(outcome.execution_requests.len(), 1);
            assert_eq!(iteration.state, WorkflowCacheIterationState::Scanning);
            child = outcome.execution_requests[0].execution_id;
        } else {
            assert!(outcome.execution_requests.is_empty());
            assert_eq!(iteration.state, WorkflowCacheIterationState::Completed);
            assert_eq!(iteration.scanned_count, 3);
            assert_eq!(iteration.dispatched_count, 3);
        }
    }
    let parent = ExecutionRepository::find_by_id(pool, fixture.parent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.status, ExecutionStatus::Completed);
    reclaim_terminal_cache(&fixture, generation).await;
    assert_eq!(
        ExecutionRepository::find_by_parent(pool, fixture.parent.id)
            .await
            .unwrap()
            .len(),
        3,
        "metadata reclamation must not remove child execution records"
    );
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
async fn cancelled_cache_child_stops_refill_releases_pin_and_cascades_metadata() {
    let fixture = CachePartitionFixture::create().await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    let mut tx = pool.begin().await.unwrap();
    let (messages, _) = dispatch(&mut tx, &fixture, &fixture.parent, &fixture.task()).await;
    tx.commit().await.unwrap();
    assert_eq!(messages.len(), 1);
    retire_generation(&fixture, generation).await;
    expire_retired_cache(pool, generation).await;
    let outcome = finish_cache_child(
        &fixture,
        messages[0].execution_id,
        ExecutionStatus::Cancelled,
    )
    .await;
    assert!(outcome.execution_requests.is_empty());
    let iteration = cache_iteration(&fixture).await;
    assert_eq!(iteration.state, WorkflowCacheIterationState::Cancelled);
    assert_eq!(iteration.dispatched_count, 1);
    let parent = ExecutionRepository::find_by_id(pool, fixture.parent.id)
        .await
        .unwrap()
        .unwrap();
    let workflow = WorkflowExecutionRepository::find_by_id(pool, fixture.workflow_execution_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.status, ExecutionStatus::Cancelled);
    assert_eq!(workflow.status, ExecutionStatus::Cancelled);
    reclaim_terminal_cache(&fixture, generation).await;
    assert_eq!(
        ExecutionRepository::find_by_parent(pool, fixture.parent.id)
            .await
            .unwrap()
            .len(),
        1
    );
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
async fn deleting_root_execution_preserves_dangling_workflow_and_active_cache_pin() {
    let fixture = CachePartitionFixture::create().await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    let mut tx = pool.begin().await.unwrap();
    let (messages, _) = dispatch(&mut tx, &fixture, &fixture.parent, &fixture.task()).await;
    tx.commit().await.unwrap();
    assert_eq!(messages.len(), 1);
    retire_generation(&fixture, generation).await;
    expire_retired_cache(pool, generation).await;
    assert!(ExecutionRepository::delete(pool, fixture.parent.id)
        .await
        .unwrap());
    assert!(ExecutionRepository::find_by_id(pool, fixture.parent.id)
        .await
        .unwrap()
        .is_none());
    let workflow = WorkflowExecutionRepository::find_by_id(pool, fixture.workflow_execution_id)
        .await
        .unwrap()
        .expect("root retention must not cascade into workflow state");
    assert_eq!(workflow.execution, fixture.parent.id);
    assert_eq!(workflow.status, ExecutionStatus::Running);
    assert_eq!(
        cache_iteration(&fixture).await.state,
        WorkflowCacheIterationState::Scanning
    );
    assert_eq!(iteration_count(pool, generation).await, (1, 1));
    assert_eq!(
        CacheGenerationRepository::drop_if_cleanup_eligible(
            pool,
            generation,
            &CacheRetentionConfig {
                min_traversal_window_seconds: 0,
                ..Default::default()
            }
        )
        .await
        .unwrap(),
        CacheGenerationCleanupOutcome::Ineligible
    );
    assert!(
        WorkflowExecutionRepository::delete(pool, fixture.workflow_execution_id)
            .await
            .unwrap()
    );
    assert_eq!(iteration_count(pool, generation).await, (0, 0));
    reclaim_terminal_cache(&fixture, generation).await;
    assert!(
        ExecutionRepository::find_by_id(pool, messages[0].execution_id)
            .await
            .unwrap()
            .is_some(),
        "dangling child lineage survives independent root and cache retention"
    );
    fixture.database.cleanup().await.unwrap();
}

fn iteration_input(
    fixture: &CachePartitionFixture,
    namespace: i64,
    generation: i64,
    task: &str,
) -> CreateWorkflowCacheIterationInput {
    CreateWorkflowCacheIterationInput {
        workflow_execution: fixture.workflow_execution_id,
        task_name: task.into(),
        namespace,
        generation,
        page_size: 1,
        batch_size: 1,
        concurrency: 1,
    }
}

async fn iteration_count(pool: &PgPool, generation: i64) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT COUNT(*) FROM workflow_cache_iteration WHERE generation=$1), retained_iterations \
        FROM cache_generation_entry_usage WHERE generation=$1")
        .bind(generation).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn retained_iteration_cap_allows_replay_rejects_growth_and_releases_deleted_rows() {
    let fixture = CachePartitionFixture::create().await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    let namespace = CacheGenerationRepository::find_by_id(pool, generation)
        .await
        .unwrap()
        .unwrap()
        .namespace;
    // All 10,000 rows use the real INSERT triggers. Bulk seeding avoids 10,000
    // network round trips; terminal rows must still consume metadata admission.
    sqlx::query("INSERT INTO workflow_cache_iteration \
        (workflow_execution, task_name, namespace, generation, page_size, batch_size, concurrency, state, completed_at) \
        SELECT $1, 'retained-' || n, $2, $3, 1, 1, 1, 'completed', NOW() FROM generate_series(1,10000) n")
        .bind(fixture.workflow_execution_id).bind(namespace).bind(generation).execute(pool).await.unwrap();
    assert_eq!(iteration_count(pool, generation).await, (10000, 10000));
    let mut replay = pool.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut replay, CacheTransactionMode::Read)
        .await
        .unwrap();
    let original = WorkflowCacheIterationRepository::create_or_find_for_update(
        &mut replay,
        iteration_input(&fixture, namespace, generation, "retained-1"),
    )
    .await
    .unwrap();
    replay.commit().await.unwrap();
    assert_eq!(original.state, WorkflowCacheIterationState::Completed);
    assert_eq!(iteration_count(pool, generation).await, (10000, 10000));
    let rejected = WorkflowCacheIterationRepository::create(
        pool,
        iteration_input(&fixture, namespace, generation, "overflow"),
    )
    .await
    .expect_err("the retained metadata cap must reject a new row");
    let attune_common::Error::Database(error) = rejected else {
        panic!("expected database cap error");
    };
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    assert_eq!(iteration_count(pool, generation).await, (10000, 10000));
    sqlx::query("DELETE FROM workflow_cache_iteration WHERE id=$1")
        .bind(original.id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(iteration_count(pool, generation).await, (9999, 9999));
    WorkflowCacheIterationRepository::create(
        pool,
        iteration_input(&fixture, namespace, generation, "replacement-metadata"),
    )
    .await
    .unwrap();
    assert_eq!(iteration_count(pool, generation).await, (10000, 10000));
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
async fn concurrent_iteration_admission_has_only_one_winner_at_the_final_slot() {
    let fixture = CachePartitionFixture::create().await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    let namespace = CacheGenerationRepository::find_by_id(pool, generation)
        .await
        .unwrap()
        .unwrap()
        .namespace;
    sqlx::query("INSERT INTO workflow_cache_iteration \
        (workflow_execution, task_name, namespace, generation, page_size, batch_size, concurrency, state, completed_at) \
        SELECT $1, 'retained-' || n, $2, $3, 1, 1, 1, 'completed', NOW() FROM generate_series(1,9999) n")
        .bind(fixture.workflow_execution_id).bind(namespace).bind(generation).execute(pool).await.unwrap();
    let mut winner = pool.begin().await.unwrap();
    let accepted = WorkflowCacheIterationRepository::create(
        &mut *winner,
        iteration_input(&fixture, namespace, generation, "last-slot-winner"),
    )
    .await
    .unwrap();
    let winner_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *winner)
        .await
        .unwrap();
    let mut loser = pool.begin().await.unwrap();
    let loser_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *loser)
        .await
        .unwrap();
    let admission = WorkflowCacheIterationRepository::create(
        &mut *loser,
        iteration_input(&fixture, namespace, generation, "last-slot-loser"),
    );
    let observed = async {
        wait_for_backend_blocked(pool, loser_pid, winner_pid).await;
        winner.commit().await.unwrap();
    };
    let (rejected, ()) = tokio::join!(admission, observed);
    let attune_common::Error::Database(error) =
        rejected.expect_err("second admission must not exceed the cap")
    else {
        panic!("expected database metadata-cap error");
    };
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    loser.rollback().await.unwrap();
    assert_eq!(iteration_count(pool, generation).await, (10000, 10000));
    assert!(
        WorkflowCacheIterationRepository::find_by_id(pool, accepted.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(WorkflowCacheIterationRepository::find_by_workflow_task(
        pool,
        fixture.workflow_execution_id,
        "last-slot-loser"
    )
    .await
    .unwrap()
    .is_none());
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
async fn inverse_generation_pin_initialization_does_not_deadlock() {
    let fixture = CachePartitionFixture::create().await;
    let pool = fixture.database.pool();
    let first_generation = seed_cache(&fixture).await;
    retire_generation(&fixture, first_generation).await;
    let namespace = CacheGenerationRepository::find_by_id(pool, first_generation)
        .await
        .unwrap()
        .unwrap()
        .namespace;
    let second_generation = CacheNamespaceRepository::find_by_id(pool, namespace)
        .await
        .unwrap()
        .unwrap()
        .active_generation
        .unwrap();
    assert_ne!(first_generation, second_generation);
    let snapshot = fixture.parent.executable_snapshot.as_ref().unwrap();
    let second_root = ExecutionRepository::create_pinned(
        pool,
        CreateExecutionInput {
            action: fixture.parent.action,
            action_ref: fixture.parent.action_ref.clone(),
            config: fixture.parent.config.clone(),
            executor: fixture.parent.executor,
            permission_set_refs: vec!["standard".into()],
            status: ExecutionStatus::Running,
            ..Default::default()
        },
        snapshot,
    )
    .await
    .unwrap();
    let second_workflow = WorkflowExecutionRepository::create(
        pool,
        CreateWorkflowExecutionInput {
            execution: second_root.id,
            workflow_def: snapshot.executable.workflow_definition.as_ref().unwrap().id,
            task_graph: serde_json::to_value(&fixture.graph).unwrap(),
            variables: serde_json::json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    let mut left = pool.begin().await.unwrap();
    let mut right = pool.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut left, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    WorkflowExecutionRepository::find_by_id_for_update(&mut *left, fixture.workflow_execution_id)
        .await
        .unwrap()
        .unwrap();
    WorkflowCacheIterationRepository::create_or_find_for_update(
        &mut left,
        iteration_input(&fixture, namespace, first_generation, "left-first"),
    )
    .await
    .unwrap();
    let left_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *left)
        .await
        .unwrap();
    let right_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *right)
        .await
        .unwrap();
    let left_second = async {
        assert_eq!(wait_for_mutation_admission(pool, left_pid).await, right_pid);
        let early_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_locks WHERE pid=$1 AND relation IN \
             ('cache_entry'::regclass,'workflow_execution'::regclass,'workflow_cache_iteration'::regclass, \
              'cache_namespace'::regclass,'cache_generation'::regclass,'cache_generation_entry_usage'::regclass)",
        ).bind(right_pid).fetch_one(pool).await.unwrap();
        assert_eq!(
            early_rows, 0,
            "admission must precede even the first workflow/generation row lock"
        );
        // Ordinary readers must not join the pin mutation gate. Keep the winner
        // uncommitted until this independently owned read transaction completes.
        let mut reader = pool.begin().await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            CacheEntryRepository::protect_transaction(&mut reader, CacheTransactionMode::Read),
        )
        .await
        .expect("ordinary Read must remain concurrent with held PinMutation")
        .unwrap();
        assert!(
            CacheGenerationRepository::find_by_id(&mut *reader, first_generation)
                .await
                .unwrap()
                .is_some()
        );
        let reader_admission: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_locks WHERE pid=pg_backend_pid() AND locktype='advisory' \
             AND classid=7821101 AND objid=0 AND objsubid=2",
        )
        .fetch_one(&mut *reader)
        .await
        .unwrap();
        assert_eq!(
            reader_admission, 0,
            "ordinary Read must not acquire mutation admission"
        );
        reader.rollback().await.unwrap();
        let result = WorkflowCacheIterationRepository::create_or_find_for_update(
            &mut left,
            iteration_input(&fixture, namespace, second_generation, "left-second"),
        )
        .await;
        if result.is_ok() {
            left.commit().await.unwrap();
        } else {
            left.rollback().await.unwrap();
        }
        result
    };
    let right_second = async {
        CacheEntryRepository::protect_transaction(&mut right, CacheTransactionMode::PinMutation)
            .await
            .unwrap();
        WorkflowExecutionRepository::find_by_id_for_update(&mut *right, second_workflow.id)
            .await
            .unwrap()
            .unwrap();
        let mut right_first =
            iteration_input(&fixture, namespace, second_generation, "right-first");
        right_first.workflow_execution = second_workflow.id;
        WorkflowCacheIterationRepository::create_or_find_for_update(&mut right, right_first)
            .await
            .unwrap();
        let mut input = iteration_input(&fixture, namespace, first_generation, "right-second");
        input.workflow_execution = second_workflow.id;
        let result =
            WorkflowCacheIterationRepository::create_or_find_for_update(&mut right, input).await;
        if result.is_ok() {
            right.commit().await.unwrap();
        } else {
            right.rollback().await.unwrap();
        }
        result
    };
    let (left_result, right_result) = tokio::join!(left_second, right_second);
    let code = |result: &attune_common::Result<WorkflowCacheIteration>| match result {
        Err(attune_common::Error::Database(error)) => error
            .as_database_error()
            .and_then(|error| error.code().map(|code| code.into_owned())),
        _ => None,
    };
    let left_code = code(&left_result);
    let right_code = code(&right_result);
    let first_count = iteration_count(pool, first_generation).await;
    let second_count = iteration_count(pool, second_generation).await;
    fixture.database.cleanup().await.unwrap();
    assert!(left_result.is_ok() && right_result.is_ok(),
        "inverse generation pins failed: left SQLSTATE={left_code:?}, right SQLSTATE={right_code:?}, \
         generation counts={first_count:?}/{second_count:?}");
    assert_eq!(first_count, (2, 2));
    assert_eq!(second_count, (2, 2));
}

#[tokio::test]
async fn repeatable_read_iteration_admission_rejects_stale_counter_and_retries_cleanly() {
    let fixture = CachePartitionFixture::create().await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    let namespace = CacheGenerationRepository::find_by_id(pool, generation)
        .await
        .unwrap()
        .unwrap()
        .namespace;
    let mut stale = pool.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *stale)
        .await
        .unwrap();
    let before: i64 = sqlx::query_scalar(
        "SELECT retained_iterations FROM cache_generation_entry_usage WHERE generation=$1",
    )
    .bind(generation)
    .fetch_one(&mut *stale)
    .await
    .unwrap();
    assert_eq!(before, 0);
    let stale_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *stale)
        .await
        .unwrap();
    let mut winning_transaction = pool.begin().await.unwrap();
    let winner = WorkflowCacheIterationRepository::create(
        &mut *winning_transaction,
        iteration_input(&fixture, namespace, generation, "winner"),
    )
    .await
    .unwrap();
    let winner_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *winning_transaction)
        .await
        .unwrap();
    let attempt = WorkflowCacheIterationRepository::create(
        &mut *stale,
        iteration_input(&fixture, namespace, generation, "stale"),
    );
    let observed = async {
        wait_for_backend_blocked(pool, stale_pid, winner_pid).await;
        winning_transaction.commit().await.unwrap();
    };
    let (rejected, ()) = tokio::join!(attempt, observed);
    let rejected = rejected.expect_err("a stale REPEATABLE READ counter must fail serialization");
    let attune_common::Error::Database(error) = rejected else {
        panic!("expected database serialization error");
    };
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("40001")
    );
    stale.rollback().await.unwrap();
    assert_eq!(iteration_count(pool, generation).await, (1, 1));
    let retry = WorkflowCacheIterationRepository::create(
        pool,
        iteration_input(&fixture, namespace, generation, "stale"),
    )
    .await
    .unwrap();
    assert_ne!(winner.id, retry.id);
    assert_eq!(iteration_count(pool, generation).await, (2, 2));
    sqlx::query("DELETE FROM workflow_cache_iteration WHERE id IN ($1,$2)")
        .bind(winner.id)
        .bind(retry.id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(iteration_count(pool, generation).await, (0, 0));
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
async fn workflow_execution_and_definition_deletion_release_retained_iteration_counters() {
    for delete_definition in [false, true] {
        let fixture = CachePartitionFixture::create().await;
        let pool = fixture.database.pool();
        let generation = seed_cache(&fixture).await;
        let namespace = CacheGenerationRepository::find_by_id(pool, generation)
            .await
            .unwrap()
            .unwrap()
            .namespace;
        WorkflowCacheIterationRepository::create(
            pool,
            iteration_input(&fixture, namespace, generation, "cascade"),
        )
        .await
        .unwrap();
        assert_eq!(iteration_count(pool, generation).await, (1, 1));
        if delete_definition {
            let definition = fixture
                .parent
                .executable_snapshot
                .as_ref()
                .unwrap()
                .executable
                .workflow_definition
                .as_ref()
                .unwrap()
                .id;
            assert!(WorkflowDefinitionRepository::delete(pool, definition)
                .await
                .unwrap());
        } else {
            assert!(
                WorkflowExecutionRepository::delete(pool, fixture.workflow_execution_id)
                    .await
                    .unwrap()
            );
        }
        assert_eq!(iteration_count(pool, generation).await, (0, 0));
        assert!(
            CacheGenerationRepository::find_by_id(pool, generation)
                .await
                .unwrap()
                .is_some(),
            "workflow deletion releases metadata only, not cache storage or admitted bytes"
        );
        fixture.database.cleanup().await.unwrap();
    }
}

async fn assert_workflow_cascade_delete_parent_first(delete_definition: bool) {
    let fixture = CachePartitionFixture::create().await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    let namespace = CacheGenerationRepository::find_by_id(pool, generation)
        .await
        .unwrap()
        .unwrap()
        .namespace;
    let iteration = WorkflowCacheIterationRepository::create(
        pool,
        iteration_input(&fixture, namespace, generation, "terminal-cascade"),
    )
    .await
    .unwrap();
    WorkflowCacheIterationRepository::mark_terminal(
        pool,
        iteration.id,
        WorkflowCacheIterationState::Completed,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    retire_generation(&fixture, generation).await;
    expire_retired_cache(pool, generation).await;
    let mut cleanup = pool.begin().await.unwrap();
    let reclaimed: (String, i64, i64) = sqlx::query_as(
        "SELECT outcome, records_reclaimed, bytes_reclaimed FROM drop_cleanup_cache_generation($1,0)",
    ).bind(generation).fetch_one(&mut *cleanup).await.unwrap();
    assert_eq!(reclaimed.0, "dropped");
    let cleanup_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *cleanup)
        .await
        .unwrap();
    let mut deletion = pool.begin().await.unwrap();
    let deletion_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *deletion)
        .await
        .unwrap();
    let definition_id = fixture
        .parent
        .executable_snapshot
        .as_ref()
        .unwrap()
        .executable
        .workflow_definition
        .as_ref()
        .unwrap()
        .id;
    let delete = async {
        if delete_definition {
            WorkflowDefinitionRepository::delete(&mut *deletion, definition_id)
                .await
                .unwrap()
        } else {
            WorkflowExecutionRepository::delete(&mut *deletion, fixture.workflow_execution_id)
                .await
                .unwrap()
        }
    };
    let observed = async {
        wait_for_backend_blocked(pool, deletion_pid, cleanup_pid).await;
        let admission_wait: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' \
             AND classid=7821101 AND objid=0 AND objsubid=2 AND mode='ExclusiveLock' AND NOT granted)",
        ).bind(deletion_pid).fetch_one(pool).await.unwrap();
        let early_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_locks WHERE pid=$1 AND granted \
             AND relation IN ('cache_entry'::regclass,'workflow_definition'::regclass,'workflow_execution'::regclass,'workflow_cache_iteration'::regclass)",
        ).bind(deletion_pid).fetch_one(pool).await.unwrap();
        cleanup.commit().await.unwrap();
        (admission_wait, early_rows)
    };
    let (deleted, (admission_wait, early_rows)) = tokio::join!(delete, observed);
    deletion.commit().await.unwrap();
    fixture.database.cleanup().await.unwrap();
    assert!(deleted);
    assert!(
        admission_wait,
        "cascade deletion must wait for mutation admission before parent/source row locks"
    );
    assert_eq!(
        early_rows, 0,
        "parent or cascade source/iteration rows were locked before mutation admission"
    );
}

#[tokio::test]
async fn workflow_execution_delete_waits_for_cleanup_parent_before_cascade_rows() {
    assert_workflow_cascade_delete_parent_first(false).await;
}

#[tokio::test]
async fn workflow_definition_delete_waits_for_cleanup_parent_before_cascade_rows() {
    assert_workflow_cascade_delete_parent_first(true).await;
}

struct CacheBrokerFixture {
    connection: attune_common::mq::Connection,
    publisher: Publisher,
    channel: lapin::Channel,
    queue: String,
}

impl CacheBrokerFixture {
    async fn connect(vhost: &str) -> Self {
        let base = std::env::var("ATTUNE_TEST_CACHE_BROKER_URL")
            .expect("broker tests require the owned cache protocol runner");
        let connection = attune_common::mq::Connection::connect(&format!("{base}/{vhost}"))
            .await
            .unwrap();
        let channel = connection.create_channel().await.unwrap();
        channel
            .exchange_declare(
                "attune.executions".into(),
                lapin::ExchangeKind::Topic,
                lapin::options::ExchangeDeclareOptions {
                    durable: true,
                    ..Default::default()
                },
                lapin::types::FieldTable::default(),
            )
            .await
            .unwrap();
        let queue = channel
            .queue_declare(
                "".into(),
                lapin::options::QueueDeclareOptions {
                    exclusive: true,
                    auto_delete: true,
                    ..Default::default()
                },
                lapin::types::FieldTable::default(),
            )
            .await
            .unwrap()
            .name()
            .as_str()
            .to_string();
        channel
            .queue_bind(
                queue.as_str().into(),
                "attune.executions".into(),
                "#".into(),
                lapin::options::QueueBindOptions::default(),
                lapin::types::FieldTable::default(),
            )
            .await
            .unwrap();
        let publisher = Publisher::new(
            &connection,
            attune_common::mq::PublisherConfig {
                confirm_publish: true,
                timeout_secs: 10,
                exchange: "attune.executions".into(),
            },
        )
        .await
        .unwrap();
        Self {
            connection,
            publisher,
            channel,
            queue,
        }
    }

    async fn receive(&self) -> serde_json::Value {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(delivery) = self
                    .channel
                    .basic_get(
                        self.queue.as_str().into(),
                        lapin::options::BasicGetOptions::default(),
                    )
                    .await
                    .unwrap()
                {
                    let body = serde_json::from_slice(&delivery.delivery.data).unwrap();
                    delivery
                        .delivery
                        .ack(lapin::options::BasicAckOptions::default())
                        .await
                        .unwrap();
                    return body;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("expected broker publication was not received")
    }

    async fn assert_empty(&self) {
        assert!(
            self.channel
                .basic_get(
                    self.queue.as_str().into(),
                    lapin::options::BasicGetOptions::default()
                )
                .await
                .unwrap()
                .is_none(),
            "unexpected duplicate or post-cancellation publication"
        );
    }

    async fn close(self) {
        self.channel
            .queue_delete(
                self.queue.as_str().into(),
                lapin::options::QueueDeleteOptions::default(),
            )
            .await
            .unwrap();
        drop(self.publisher);
        self.channel
            .close(200, "owned cache fixture complete".into())
            .await
            .unwrap();
        self.connection.close().await.unwrap();
    }
}

async fn activate_cache_via_broker(
    fixture: &CachePartitionFixture,
    broker: &CacheBrokerFixture,
    task: &TaskNode,
) {
    ExecutionScheduler::activate_entry_workflow_task(
        fixture.database.pool(),
        &broker.publisher,
        &AtomicUsize::new(0),
        &fixture.parent,
        &fixture.workflow_execution_id,
        task,
        &fixture.context,
        None,
        None,
    )
    .await
    .unwrap();
}

async fn advance_cache_via_broker(
    fixture: &CachePartitionFixture,
    broker: &CacheBrokerFixture,
    child_id: i64,
    status: ExecutionStatus,
) {
    let child = ExecutionRepository::update(
        fixture.database.pool(),
        child_id,
        UpdateExecutionInput {
            status: Some(status),
            result: Some(serde_json::json!({"processed": true})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let transport: Arc<dyn attune_common::artifact_transport::ArtifactFileTransport> = Arc::new(
        attune_common::artifact_transport::VolumeTransport::new(root.path().to_str().unwrap()),
    );
    ExecutionScheduler::advance_workflow(
        fixture.database.pool(),
        &broker.publisher,
        &AtomicUsize::new(0),
        root.path().to_str().unwrap(),
        &transport,
        1024 * 1024,
        1000,
        None,
        &child,
        &SchedulerMetadataCaches::new(),
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires the owned RabbitMQ cache protocol fixture"]
async fn broker_cache_refill_completion_replay_and_terminal_cascade() {
    let fixture = CachePartitionFixture::create().await;
    let broker = CacheBrokerFixture::connect("cache-completion").await;
    let generation = seed_cache(&fixture).await;
    activate_cache_via_broker(&fixture, &broker, &fixture.task()).await;
    let first = broker.receive().await;
    assert_eq!(first["message_type"], "ExecutionRequested");
    let mut child = first["payload"]["execution_id"].as_i64().unwrap();
    retire_generation(&fixture, generation).await;
    expire_retired_cache(fixture.database.pool(), generation).await;
    for index in 0..3 {
        advance_cache_via_broker(&fixture, &broker, child, ExecutionStatus::Completed).await;
        if index < 2 {
            let next = broker.receive().await;
            assert_eq!(next["message_type"], "ExecutionRequested");
            let next_child = next["payload"]["execution_id"].as_i64().unwrap();
            assert_ne!(next_child, child);
            child = next_child;
            assert_eq!(
                cache_iteration(&fixture).await.state,
                WorkflowCacheIterationState::Scanning
            );
        }
    }
    assert_eq!(
        cache_iteration(&fixture).await.state,
        WorkflowCacheIterationState::Completed
    );
    assert_eq!(cache_iteration(&fixture).await.dispatched_count, 3);
    assert_eq!(
        ExecutionRepository::find_by_id(fixture.database.pool(), fixture.parent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Completed
    );
    // A duplicate final completion must neither create a fourth child nor publish.
    advance_cache_via_broker(&fixture, &broker, child, ExecutionStatus::Completed).await;
    broker.assert_empty().await;
    assert_eq!(
        ExecutionRepository::find_by_parent(fixture.database.pool(), fixture.parent.id)
            .await
            .unwrap()
            .len(),
        3
    );
    reclaim_terminal_cache(&fixture, generation).await;
    broker.close().await;
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the owned RabbitMQ cache protocol fixture"]
async fn broker_cache_cancellation_stops_publication_and_releases_terminal_metadata() {
    let fixture = CachePartitionFixture::create().await;
    let broker = CacheBrokerFixture::connect("cache-cancellation").await;
    let generation = seed_cache(&fixture).await;
    activate_cache_via_broker(&fixture, &broker, &fixture.task()).await;
    let first = broker.receive().await;
    let child = first["payload"]["execution_id"].as_i64().unwrap();
    retire_generation(&fixture, generation).await;
    expire_retired_cache(fixture.database.pool(), generation).await;
    advance_cache_via_broker(&fixture, &broker, child, ExecutionStatus::Cancelled).await;
    broker.assert_empty().await;
    assert_eq!(
        cache_iteration(&fixture).await.state,
        WorkflowCacheIterationState::Cancelled
    );
    assert_eq!(cache_iteration(&fixture).await.dispatched_count, 1);
    assert_eq!(
        ExecutionRepository::find_by_id(fixture.database.pool(), fixture.parent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Cancelled
    );
    reclaim_terminal_cache(&fixture, generation).await;
    broker.close().await;
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the owned RabbitMQ cache protocol fixture"]
async fn broker_cache_cleanup_first_blocks_real_transaction_entry_before_rows() {
    let fixture = CachePartitionFixture::create().await;
    let broker = CacheBrokerFixture::connect("cache-cleanup-first").await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    retire_generation(&fixture, generation).await;
    expire_retired_cache(pool, generation).await;
    let task = cache_task(&fixture, generation);
    let mut cleanup = pool.begin().await.unwrap();
    let reclaimed: (String, i64, i64) = sqlx::query_as(
        "SELECT outcome, records_reclaimed, bytes_reclaimed FROM drop_cleanup_cache_generation($1,0)",
    ).bind(generation).fetch_one(&mut *cleanup).await.unwrap();
    assert_eq!(reclaimed.0, "dropped");
    let cleanup_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *cleanup)
        .await
        .unwrap();
    let activation = activate_cache_via_broker(&fixture, &broker, &task);
    let observed = async {
        let activation_pid = wait_for_mutation_admission(pool, cleanup_pid).await;
        let early_row_locks: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_locks WHERE granted \
             AND relation IN ('cache_entry'::regclass,'workflow_execution'::regclass,'workflow_cache_iteration'::regclass) \
             AND pid=$1",
        ).bind(activation_pid).fetch_one(pool).await.unwrap();
        assert_eq!(early_row_locks, 0);
        cleanup.commit().await.unwrap();
    };
    let ((), ()) = tokio::join!(activation, observed);
    let terminal = broker.receive().await;
    assert_eq!(terminal["message_type"], "ExecutionCompleted");
    assert_eq!(terminal["payload"]["status"], "failed");
    broker.assert_empty().await;
    broker.close().await;
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the owned RabbitMQ cache protocol fixture"]
async fn broker_cache_workflow_first_protects_parent_before_blocked_workflow_row() {
    let fixture = CachePartitionFixture::create().await;
    let broker = CacheBrokerFixture::connect("cache-workflow-first").await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    let mut gate = pool.begin().await.unwrap();
    WorkflowExecutionRepository::find_by_id_for_update(&mut *gate, fixture.workflow_execution_id)
        .await
        .unwrap()
        .unwrap();
    let gate_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await
        .unwrap();
    let task = fixture.task();
    let activation = activate_cache_via_broker(&fixture, &broker, &task);
    let (signal, start_cleanup) = tokio::sync::oneshot::channel();
    let cleanup = async {
        start_cleanup.await.unwrap();
        CacheGenerationRepository::drop_if_cleanup_eligible(
            pool,
            generation,
            &CacheRetentionConfig {
                min_traversal_window_seconds: 0,
                ..Default::default()
            },
        )
        .await
        .unwrap()
    };
    let observed = async {
        wait_for_parent_lock(pool, "AccessShareLock", true, gate_pid).await;
        let activation_pid: i32 = sqlx::query_scalar(
            "SELECT pid FROM pg_locks WHERE relation='cache_entry'::regclass \
             AND mode='AccessShareLock' AND granted AND $1=ANY(pg_blocking_pids(pid))",
        )
        .bind(gate_pid)
        .fetch_one(pool)
        .await
        .unwrap();
        signal.send(()).unwrap();
        wait_for_mutation_admission(pool, activation_pid).await;
        gate.rollback().await.unwrap();
    };
    let ((), outcome, ()) = tokio::join!(activation, cleanup, observed);
    assert_eq!(outcome, CacheGenerationCleanupOutcome::Ineligible);
    let publication = broker.receive().await;
    assert_eq!(publication["message_type"], "ExecutionRequested");
    assert_eq!(cache_iteration(&fixture).await.generation, generation);
    assert_eq!(cache_iteration(&fixture).await.dispatched_count, 1);
    broker.assert_empty().await;
    broker.close().await;
    fixture.database.cleanup().await.unwrap();
}

async fn assert_broker_terminal_advancement_waits_before_rows(
    status: ExecutionStatus,
    vhost: &str,
) {
    let fixture = CachePartitionFixture::create().await;
    let broker = CacheBrokerFixture::connect(vhost).await;
    let pool = fixture.database.pool();
    let generation = seed_cache(&fixture).await;
    activate_cache_via_broker(&fixture, &broker, &fixture.task()).await;
    let mut child = broker.receive().await["payload"]["execution_id"]
        .as_i64()
        .unwrap();
    retire_generation(&fixture, generation).await;
    expire_retired_cache(pool, generation).await;
    if status == ExecutionStatus::Completed {
        for _ in 0..2 {
            advance_cache_via_broker(&fixture, &broker, child, ExecutionStatus::Completed).await;
            child = broker.receive().await["payload"]["execution_id"]
                .as_i64()
                .unwrap();
        }
    }
    let mut cleanup = pool.begin().await.unwrap();
    let outcome: String =
        sqlx::query_scalar("SELECT outcome FROM drop_cleanup_cache_generation($1,0)")
            .bind(generation)
            .fetch_one(&mut *cleanup)
            .await
            .unwrap();
    assert_eq!(outcome, "ineligible");
    let cleanup_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *cleanup)
        .await
        .unwrap();
    let advancement = advance_cache_via_broker(&fixture, &broker, child, status.clone());
    let observed = async {
        let advancement_pid = wait_for_mutation_admission(pool, cleanup_pid).await;
        let early_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_locks WHERE granted \
             AND relation IN ('cache_entry'::regclass,'workflow_execution'::regclass,'workflow_cache_iteration'::regclass) \
             AND pid=$1",
        ).bind(advancement_pid).fetch_one(pool).await.unwrap();
        assert_eq!(
            early_rows, 0,
            "terminal advancement must protect the parent before workflow rows"
        );
        cleanup.commit().await.unwrap();
    };
    let ((), ()) = tokio::join!(advancement, observed);
    let iteration = cache_iteration(&fixture).await;
    assert_eq!(
        iteration.state,
        if status == ExecutionStatus::Completed {
            WorkflowCacheIterationState::Completed
        } else {
            WorkflowCacheIterationState::Cancelled
        }
    );
    assert_eq!(
        ExecutionRepository::find_by_id(pool, fixture.parent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        status
    );
    broker.assert_empty().await;
    reclaim_terminal_cache(&fixture, generation).await;
    broker.close().await;
    fixture.database.cleanup().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the owned RabbitMQ cache protocol fixture"]
async fn broker_cache_completion_waits_for_cleanup_parent_before_workflow_rows() {
    assert_broker_terminal_advancement_waits_before_rows(
        ExecutionStatus::Completed,
        "cache-held-completion",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires the owned RabbitMQ cache protocol fixture"]
async fn broker_cache_cancellation_waits_for_cleanup_parent_before_workflow_rows() {
    assert_broker_terminal_advancement_waits_before_rows(
        ExecutionStatus::Cancelled,
        "cache-held-cancellation",
    )
    .await;
}
