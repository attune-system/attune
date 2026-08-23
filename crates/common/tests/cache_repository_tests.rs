//! Repository integration tests for owner-scoped cache persistence.

mod helpers;

use attune_common::{
    config::{CacheAdmissionConfig, RetentionConfig},
    models::{
        CacheGenerationState, ExecutionStatus, OwnerType, WorkflowCacheIterationState,
        WorkflowTaskMetadata,
    },
    pack_registry::PackComponentLoader,
    repositories::{
        action::ActionRepository,
        cache::{
            CacheEntryInput, CacheEntryRepository, CacheGenerationRepository,
            CacheIngestRepository, CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope,
            CreateCacheGenerationInput, CreateCacheGenerationResult, CreateCacheNamespaceInput,
            InsertCacheChunkResult, ManagedCacheNamespaceDefinition, SealCacheGenerationInput,
            MAX_MULTI_LOOKUP_BYTES, MAX_MULTI_LOOKUP_IDS, MAX_SCAN_MATERIALIZATION_BYTES,
        },
        execution::{CreateExecutionInput, ExecutionRepository},
        pack::{PackRepository, UpdatePackInput},
        retention::RetentionRepository,
        trigger::SensorRepository,
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            UpdateWorkflowExecutionInput, WorkflowDefinitionRepository,
            WorkflowExecutionRepository,
        },
        workflow_cache_iteration::{
            CreateWorkflowCacheIterationInput, UpdateWorkflowCacheIterationProgressInput,
            WorkflowCacheIterationRepository,
        },
        Create, Delete, FindById, FindByRef, Update,
    },
    Error,
};
use chrono::{Duration, Utc};
use helpers::{unique_test_id, PackFixture, RuntimeFixture};
use serde_json::json;
use sqlx::PgPool;
use std::fs;
use tempfile::TempDir;

fn namespace_input(namespace: String, policy: CacheNamespacePolicy) -> CreateCacheNamespaceInput {
    CreateCacheNamespaceInput {
        owner: CacheOwnerScope::system(),
        namespace,
        policy,
    }
}

fn generation_input(
    namespace: i64,
    refresh_id: impl Into<String>,
    expected_chunk_count: i32,
    expected_count: Option<i64>,
) -> CreateCacheGenerationInput {
    CreateCacheGenerationInput {
        namespace,
        client_refresh_id: refresh_id.into(),
        expected_active_generation: None,
        expected_chunk_count,
        expected_count,
        expected_bytes: None,
        checksum_algorithm: None,
        checksum: None,
        source_revision: None,
        created_by: None,
    }
}

fn entries(ids: &[&str]) -> Vec<CacheEntryInput> {
    ids.iter()
        .map(|id| CacheEntryInput {
            external_id: (*id).to_string(),
            value: json!({"id": id}),
            source_updated_at: None,
            source_checksum: None,
        })
        .collect()
}

async fn create_generation(
    pool: &PgPool,
    namespace: i64,
    refresh_id: impl Into<String>,
    expected_chunk_count: i32,
    expected_count: Option<i64>,
) -> attune_common::models::CacheGeneration {
    match CacheGenerationRepository::create_or_get(
        pool,
        &generation_input(namespace, refresh_id, expected_chunk_count, expected_count),
    )
    .await
    .unwrap()
    {
        CreateCacheGenerationResult::Created(generation)
        | CreateCacheGenerationResult::Existing(generation) => generation,
    }
}

async fn seal_generation(
    pool: &PgPool,
    namespace: i64,
    refresh_id: &str,
    ids: &[&str],
) -> attune_common::models::CacheGeneration {
    let expected_active_generation = CacheNamespaceRepository::find_by_id(pool, namespace)
        .await
        .unwrap()
        .unwrap()
        .active_generation;
    let mut input = generation_input(
        namespace,
        refresh_id,
        1,
        Some(i64::try_from(ids.len()).unwrap()),
    );
    input.expected_active_generation = expected_active_generation;
    let generation = match CacheGenerationRepository::create_or_get(pool, &input)
        .await
        .unwrap()
    {
        CreateCacheGenerationResult::Created(generation)
        | CreateCacheGenerationResult::Existing(generation) => generation,
    };
    CacheIngestRepository::insert_chunk(pool, generation.id, 0, "request-v1", &entries(ids))
        .await
        .unwrap();
    CacheGenerationRepository::seal(pool, generation.id)
        .await
        .unwrap()
}

async fn publish_generation(
    pool: &PgPool,
    namespace: i64,
    refresh_id: &str,
    ids: &[&str],
    expected_active_generation: Option<i64>,
) -> attune_common::models::CacheGeneration {
    let ready = seal_generation(pool, namespace, refresh_id, ids).await;
    CacheGenerationRepository::promote(
        pool,
        namespace,
        ready.id,
        expected_active_generation,
        Utc::now() + Duration::minutes(10),
    )
    .await
    .unwrap()
    .activated_generation
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn owner_constraints_and_canonical_owner_namespace_uniqueness() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace_name = format!("cache_{}", unique_test_id());

    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(namespace_name.clone(), CacheNamespacePolicy::default()),
    )
    .await
    .unwrap();
    assert_eq!(namespace.owner, "system");

    assert!(CacheNamespaceRepository::create(
        &pool,
        namespace_input(namespace_name, CacheNamespacePolicy::default()),
    )
    .await
    .is_err());

    let mut invalid_owner = CacheOwnerScope::system();
    invalid_owner.owner_pack = Some(123);
    assert!(CacheNamespaceRepository::create(
        &pool,
        CreateCacheNamespaceInput {
            owner: invalid_owner,
            namespace: format!("invalid_{}", unique_test_id()),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn cache_retention_config_is_persisted_with_runtime_retention() {
    let pool = helpers::create_test_pool().await.unwrap();
    let mut config = RetentionConfig::default();
    config.cache_retention.batch_size = 321;
    config.cache_retention.max_batches_per_generation = 7;
    config.cache_retention.staging_expiry_seconds = 1234;
    config.cache_retention.freshness_alerts_enabled = false;

    let stored = RetentionRepository::update_config(&pool, &config)
        .await
        .unwrap();
    assert_eq!(stored.cache_retention, config.cache_retention);
    assert_eq!(
        RetentionRepository::load_config(&pool)
            .await
            .unwrap()
            .cache_retention,
        config.cache_retention
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn bytewise_ordering_and_duplicate_ids_are_enforced() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("order_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();

    let generation =
        publish_generation(&pool, namespace.id, "order-v1", &["z", "ä", "A", "a"], None).await;
    let scanned = CacheEntryRepository::scan_pinned(&pool, namespace.id, generation.id, None, 10)
        .await
        .unwrap();
    assert_eq!(
        scanned
            .into_iter()
            .map(|entry| entry.external_id)
            .collect::<Vec<_>>(),
        vec!["A", "a", "z", "ä"]
    );

    let duplicate = create_generation(&pool, namespace.id, "duplicates-v1", 1, None).await;
    assert!(matches!(
        CacheIngestRepository::insert_chunk(
            &pool,
            duplicate.id,
            0,
            "duplicate-request",
            &entries(&["same", "same"]),
        )
        .await,
        Err(Error::CacheDuplicateExternalId)
    ));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn chunk_replays_and_seal_write_boundary_are_enforced() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("chunks_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let generation = create_generation(&pool, namespace.id, "chunks-v1", 2, Some(2)).await;
    assert!(CacheIngestRepository::insert_chunk(
        &pool,
        generation.id,
        2,
        "out-of-range",
        &entries(&["never"]),
    )
    .await
    .is_err());

    assert!(matches!(
        CacheIngestRepository::insert_chunk(
            &pool,
            generation.id,
            0,
            "chunk-zero",
            &entries(&["first"]),
        )
        .await
        .unwrap(),
        InsertCacheChunkResult::Inserted(_)
    ));
    assert!(matches!(
        CacheIngestRepository::insert_chunk(
            &pool,
            generation.id,
            0,
            "chunk-zero",
            &entries(&["first"]),
        )
        .await
        .unwrap(),
        InsertCacheChunkResult::Replayed(_)
    ));
    assert!(CacheIngestRepository::insert_chunk(
        &pool,
        generation.id,
        0,
        "different-payload",
        &entries(&["changed"]),
    )
    .await
    .is_err());
    assert!(CacheGenerationRepository::seal(&pool, generation.id)
        .await
        .is_err());

    CacheIngestRepository::insert_chunk(
        &pool,
        generation.id,
        1,
        "chunk-one",
        &entries(&["second"]),
    )
    .await
    .unwrap();
    assert!(CacheGenerationRepository::seal_with_expectations(
        &pool,
        generation.id,
        Some(SealCacheGenerationInput {
            expected_chunk_count: 2,
            expected_count: Some(3),
            expected_bytes: None,
        }),
    )
    .await
    .is_err());
    assert_eq!(
        CacheGenerationRepository::find_by_id(&pool, generation.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        CacheGenerationState::Staging,
        "failed seal expectations must not publish a ready generation"
    );
    assert_eq!(
        CacheGenerationRepository::seal(&pool, generation.id)
            .await
            .unwrap()
            .state,
        CacheGenerationState::Ready
    );
    assert!(CacheIngestRepository::insert_chunk(
        &pool,
        generation.id,
        2,
        "after-seal",
        &entries(&["late"]),
    )
    .await
    .is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn promotion_is_optimistic_and_pinned_reads_do_not_mix_generations() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("promote_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let first = publish_generation(&pool, namespace.id, "first", &["a", "b"], None).await;
    let second_ready = seal_generation(&pool, namespace.id, "second", &["c", "d"]).await;

    CacheGenerationRepository::promote(
        &pool,
        namespace.id,
        second_ready.id,
        Some(first.id),
        Utc::now() + Duration::minutes(10),
    )
    .await
    .unwrap();
    let pinned_first =
        CacheEntryRepository::scan_pinned(&pool, namespace.id, first.id, Some("a"), 10)
            .await
            .unwrap();
    assert_eq!(
        pinned_first
            .into_iter()
            .map(|entry| entry.external_id)
            .collect::<Vec<_>>(),
        vec!["b"]
    );
    assert_eq!(
        CacheEntryRepository::find_active(&pool, namespace.id, "c")
            .await
            .unwrap()
            .unwrap()
            .external_id,
        "c"
    );

    let third = seal_generation(&pool, namespace.id, "third", &["e"]).await;
    let fourth = seal_generation(&pool, namespace.id, "fourth", &["f"]).await;
    let expected_active = second_ready.id;
    let first_pool = pool.clone();
    let second_pool = pool.clone();
    let (first_result, second_result) = tokio::join!(
        CacheGenerationRepository::promote(
            &first_pool,
            namespace.id,
            third.id,
            Some(expected_active),
            Utc::now() + Duration::minutes(10),
        ),
        CacheGenerationRepository::promote(
            &second_pool,
            namespace.id,
            fourth.id,
            Some(expected_active),
            Utc::now() + Duration::minutes(10),
        )
    );
    assert_eq!(
        [first_result.is_ok(), second_result.is_ok()]
            .into_iter()
            .filter(|result| *result)
            .count(),
        1
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn namespace_isolation_and_generation_quota_rejection() {
    let pool = helpers::create_test_pool().await.unwrap();
    let first_namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("isolation_a_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let second_namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("isolation_b_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    publish_generation(
        &pool,
        first_namespace.id,
        "isolation-a",
        &["shared-id"],
        None,
    )
    .await;
    publish_generation(
        &pool,
        second_namespace.id,
        "isolation-b",
        &["shared-id"],
        None,
    )
    .await;
    let first_entry = CacheEntryRepository::find_active(&pool, first_namespace.id, "shared-id")
        .await
        .unwrap()
        .unwrap();
    let second_entry = CacheEntryRepository::find_active(&pool, second_namespace.id, "shared-id")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first_entry.generation,
        CacheNamespaceRepository::find_by_id(&pool, first_namespace.id)
            .await
            .unwrap()
            .unwrap()
            .active_generation
            .unwrap()
    );
    assert_ne!(first_entry.generation, second_entry.generation);

    let quota_namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("quota_{}", unique_test_id()),
            CacheNamespacePolicy {
                max_records_per_generation: 1,
                ..CacheNamespacePolicy::default()
            },
        ),
    )
    .await
    .unwrap();
    let over_quota = create_generation(&pool, quota_namespace.id, "quota-v1", 1, None).await;
    assert!(CacheIngestRepository::insert_chunk(
        &pool,
        over_quota.id,
        0,
        "over-quota",
        &entries(&["one", "two"]),
    )
    .await
    .is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn expiration_tombstone_and_bounded_cleanup_primitives() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("cleanup_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let first = publish_generation(&pool, namespace.id, "cleanup-first", &["a", "b"], None).await;
    let second_ready = seal_generation(&pool, namespace.id, "cleanup-second", &["c"]).await;
    CacheGenerationRepository::promote(
        &pool,
        namespace.id,
        second_ready.id,
        Some(first.id),
        Utc::now() - Duration::seconds(1),
    )
    .await
    .unwrap();

    assert!(
        CacheGenerationRepository::find_readable_pinned(&pool, namespace.id, first.id)
            .await
            .unwrap()
            .is_none()
    );
    let candidates = CacheGenerationRepository::select_cleanup_candidates(&pool, 10)
        .await
        .unwrap();
    assert!(candidates
        .iter()
        .any(|generation| generation.id == first.id));
    assert_eq!(
        CacheEntryRepository::delete_cleanup_batch(&pool, first.id, 1)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        CacheEntryRepository::delete_cleanup_batch(&pool, first.id, 10)
            .await
            .unwrap(),
        1
    );
    assert!(CacheGenerationRepository::delete_if_empty(&pool, first.id)
        .await
        .unwrap());

    assert!(CacheNamespaceRepository::tombstone(&pool, namespace.id)
        .await
        .unwrap());
    assert!(CacheEntryRepository::find_active(&pool, namespace.id, "c")
        .await
        .unwrap()
        .is_none());
    assert!(CacheIngestRepository::insert_chunk(
        &pool,
        second_ready.id,
        1,
        "tombstoned",
        &entries(&["late"]),
    )
    .await
    .is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn only_nonterminal_workflow_iteration_pins_generation() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("iteration_pin_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let pinned = publish_generation(&pool, namespace.id, "iteration-first", &["a"], None).await;
    let replacement = seal_generation(&pool, namespace.id, "iteration-second", &["b"]).await;
    CacheGenerationRepository::promote(
        &pool,
        namespace.id,
        replacement.id,
        Some(pinned.id),
        Utc::now() - Duration::seconds(1),
    )
    .await
    .unwrap();

    let pack = PackFixture::new_unique("cache_iteration")
        .create(&pool)
        .await
        .unwrap();
    let action = helpers::ActionFixture::new_unique(pack.id, &pack.r#ref, "workflow")
        .create(&pool)
        .await
        .unwrap();
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            status: ExecutionStatus::Running,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let workflow_definition = WorkflowDefinitionRepository::create(
        &pool,
        CreateWorkflowDefinitionInput {
            r#ref: format!("{}.cache_iteration", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref,
            label: "Cache iteration".to_string(),
            description: None,
            version: "1.0.0".to_string(),
            param_schema: None,
            out_schema: None,
            definition: json!({}),
            tags: Vec::new(),
        },
    )
    .await
    .unwrap();
    let workflow_execution = WorkflowExecutionRepository::create(
        &pool,
        CreateWorkflowExecutionInput {
            execution: execution.id,
            workflow_def: workflow_definition.id,
            task_graph: json!({}),
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();

    let iteration = WorkflowCacheIterationRepository::create(
        &pool,
        CreateWorkflowCacheIterationInput {
            workflow_execution: workflow_execution.id,
            task_name: "iterate".to_string(),
            namespace: namespace.id,
            generation: pinned.id,
            page_size: 100,
            batch_size: 10,
            concurrency: 4,
        },
    )
    .await
    .unwrap();
    assert_eq!(iteration.state, WorkflowCacheIterationState::Scanning);

    let synthetic_child = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            parent: Some(execution.id),
            status: ExecutionStatus::Completed,
            result: Some(json!({"cache_iteration": {"state": "completed"}})),
            workflow_task: Some(WorkflowTaskMetadata {
                workflow_execution: workflow_execution.id,
                task_name: "empty_iterate".to_string(),
                triggered_by: None,
                task_index: None,
                task_batch: Some(0),
                retry_count: 0,
                max_retries: 0,
                next_retry_at: None,
                timeout_seconds: None,
                timed_out: false,
                duration_ms: Some(0),
                started_at: Some(Utc::now()),
                completed_at: Some(Utc::now()),
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let candidates =
        WorkflowCacheIterationRepository::find_stale_synthetic_completions(&pool, 0, 10)
            .await
            .unwrap();
    assert!(candidates
        .iter()
        .any(|candidate| candidate.execution_id == synthetic_child.id));

    WorkflowExecutionRepository::update(
        &pool,
        workflow_execution.id,
        UpdateWorkflowExecutionInput {
            completed_tasks: Some(vec!["empty_iterate".to_string()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        WorkflowCacheIterationRepository::find_stale_synthetic_completions(&pool, 0, 10)
            .await
            .unwrap()
            .iter()
            .all(|candidate| candidate.execution_id != synthetic_child.id)
    );
    WorkflowExecutionRepository::update(
        &pool,
        workflow_execution.id,
        UpdateWorkflowExecutionInput {
            completed_tasks: Some(Vec::new()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    for (task_name, page_size, batch_size, concurrency) in [
        ("page-too-large", 1001, 1, 1),
        ("batch-too-large", 1, 1001, 1),
        ("concurrency-too-large", 1, 1, 101),
    ] {
        assert!(WorkflowCacheIterationRepository::create(
            &pool,
            CreateWorkflowCacheIterationInput {
                workflow_execution: workflow_execution.id,
                task_name: task_name.to_string(),
                namespace: namespace.id,
                generation: pinned.id,
                page_size,
                batch_size,
                concurrency,
            },
        )
        .await
        .is_err());
    }
    assert!(
        CacheGenerationRepository::select_cleanup_candidates(&pool, 10)
            .await
            .unwrap()
            .iter()
            .all(|candidate| candidate.id != pinned.id)
    );
    assert_eq!(
        CacheEntryRepository::delete_cleanup_batch(&pool, pinned.id, 10)
            .await
            .unwrap(),
        0
    );
    assert!(
        !CacheGenerationRepository::delete_if_empty(&pool, pinned.id)
            .await
            .unwrap()
    );

    assert!(WorkflowCacheIterationRepository::update_scan_progress(
        &pool,
        iteration.id,
        UpdateWorkflowCacheIterationProgressInput {
            last_external_id: "a".to_string(),
            next_batch_index: 2,
            scanned_count: 1,
            dispatched_count: 1,
        },
    )
    .await
    .is_err());
    assert!(WorkflowCacheIterationRepository::update_scan_progress(
        &pool,
        iteration.id,
        UpdateWorkflowCacheIterationProgressInput {
            last_external_id: "a".to_string(),
            next_batch_index: 2,
            scanned_count: 1,
            dispatched_count: 2,
        },
    )
    .await
    .is_err());

    let progressed = WorkflowCacheIterationRepository::update_scan_progress(
        &pool,
        iteration.id,
        UpdateWorkflowCacheIterationProgressInput {
            last_external_id: "a".to_string(),
            next_batch_index: 1,
            scanned_count: 1,
            dispatched_count: 1,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(progressed.last_external_id.as_deref(), Some("a"));
    assert_eq!(progressed.next_batch_index, 1);

    WorkflowExecutionRepository::update(
        &pool,
        workflow_execution.id,
        UpdateWorkflowExecutionInput {
            status: Some(ExecutionStatus::Abandoned),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let remediation =
        WorkflowCacheIterationRepository::remediate_scanning_for_terminal_workflows(&pool, 10)
            .await
            .unwrap();
    assert_eq!(remediation.completed, 0);
    assert_eq!(remediation.failed, 1);
    assert_eq!(remediation.cancelled, 0);
    assert_eq!(
        WorkflowCacheIterationRepository::find_by_id(&pool, iteration.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        WorkflowCacheIterationState::Failed
    );
    assert_eq!(
        WorkflowCacheIterationRepository::remediate_scanning_for_terminal_workflows(&pool, 10)
            .await
            .unwrap()
            .total(),
        0
    );
    assert!(
        CacheEntryRepository::scan_pinned(&pool, namespace.id, pinned.id, None, 10)
            .await
            .is_err()
    );
    assert!(
        CacheGenerationRepository::select_cleanup_candidates(&pool, 10)
            .await
            .unwrap()
            .iter()
            .any(|candidate| candidate.id == pinned.id)
    );
    assert_eq!(
        CacheEntryRepository::delete_cleanup_batch(&pool, pinned.id, 10)
            .await
            .unwrap(),
        1
    );
    assert!(CacheGenerationRepository::delete_if_empty(&pool, pinned.id)
        .await
        .unwrap());
}

/// A share lock during the scan keeps readability and the scan on one snapshot,
/// so an unreadable generation is a typed error while a genuine end-of-page is
/// an empty `Vec`.
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn scan_pinned_distinguishes_expired_from_end_of_generation() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("scanexp_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let other = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("scanexp_other_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();

    // Active snapshot, cursor past the last id: a genuine empty page.
    let first = publish_generation(&pool, namespace.id, "scanexp-first", &["a", "b"], None).await;
    assert!(
        CacheEntryRepository::scan_pinned(&pool, namespace.id, first.id, Some("b"), 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        CacheEntryRepository::find_pinned(&pool, namespace.id, first.id, "a")
            .await
            .unwrap()
            .unwrap()
            .external_id,
        "a"
    );
    assert_eq!(
        CacheEntryRepository::find_pinned_many(
            &pool,
            namespace.id,
            first.id,
            &["b".to_string(), "missing".to_string(), "a".to_string()],
        )
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.external_id)
        .collect::<Vec<_>>(),
        vec!["b", "a"]
    );

    // Retire `first` but keep it readable for now.
    let second = seal_generation(&pool, namespace.id, "scanexp-second", &["c"]).await;
    CacheGenerationRepository::promote(
        &pool,
        namespace.id,
        second.id,
        Some(first.id),
        Utc::now() + Duration::minutes(10),
    )
    .await
    .unwrap();

    // Retired-but-readable snapshot still serves its own records and reports a
    // genuine empty page past its last id — never a typed error.
    assert_eq!(
        CacheEntryRepository::scan_pinned(&pool, namespace.id, first.id, None, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.external_id)
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert!(
        CacheEntryRepository::scan_pinned(&pool, namespace.id, first.id, Some("b"), 10)
            .await
            .unwrap()
            .is_empty()
    );

    // Simulate the retained window elapsing for `first`.
    sqlx::query(
        "UPDATE cache_generation SET readable_until = NOW() - INTERVAL '1 second' WHERE id = $1",
    )
    .bind(first.id)
    .execute(&pool)
    .await
    .unwrap();

    // Expired snapshot: a typed error, regardless of cursor position.
    assert!(matches!(
        CacheEntryRepository::scan_pinned(&pool, namespace.id, first.id, None, 10).await,
        Err(Error::CacheSnapshotExpired(_))
    ));
    assert!(matches!(
        CacheEntryRepository::scan_pinned(&pool, namespace.id, first.id, Some("b"), 10).await,
        Err(Error::CacheSnapshotExpired(_))
    ));
    assert!(matches!(
        CacheEntryRepository::find_pinned(&pool, namespace.id, first.id, "a").await,
        Err(Error::CacheSnapshotExpired(_))
    ));

    // The active snapshot still reports a genuine empty page.
    assert!(
        CacheEntryRepository::scan_pinned(&pool, namespace.id, second.id, Some("c"), 10)
            .await
            .unwrap()
            .is_empty()
    );

    // Unknown and wrong-namespace generations are typed errors, never empty.
    assert!(matches!(
        CacheEntryRepository::scan_pinned(&pool, namespace.id, i64::MAX, None, 10).await,
        Err(Error::CacheSnapshotExpired(_))
    ));
    assert!(matches!(
        CacheEntryRepository::scan_pinned(&pool, other.id, second.id, None, 10).await,
        Err(Error::CacheSnapshotExpired(_))
    ));
}

/// Multi-id reads preserve request order, omit missing ids, stay bounded, and
/// never cross namespace boundaries.
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn multi_id_lookup_orders_omits_missing_and_isolates_namespaces() {
    let pool = helpers::create_test_pool().await.unwrap();
    let first_namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("multi_a_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    publish_generation(&pool, first_namespace.id, "multi-a", &["a", "b", "c"], None).await;

    // Found records return in request order; missing ids are omitted.
    let found = CacheEntryRepository::find_active_many(
        &pool,
        first_namespace.id,
        &[
            "c".to_string(),
            "zzz-missing".to_string(),
            "a".to_string(),
            "c".to_string(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|entry| entry.external_id.clone())
            .collect::<Vec<_>>(),
        vec!["c", "a"]
    );

    // Empty input short-circuits; oversized input is rejected.
    assert!(
        CacheEntryRepository::find_active_many(&pool, first_namespace.id, &[])
            .await
            .unwrap()
            .is_empty()
    );
    let too_many: Vec<String> = (0..=MAX_MULTI_LOOKUP_IDS)
        .map(|index| format!("id-{index}"))
        .collect();
    assert!(matches!(
        CacheEntryRepository::find_active_many(&pool, first_namespace.id, &too_many).await,
        Err(Error::Validation(_))
    ));

    // A second namespace sharing external ids stays isolated.
    let second_namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("multi_b_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    publish_generation(&pool, second_namespace.id, "multi-b", &["a"], None).await;
    let isolated = CacheEntryRepository::find_active_many(
        &pool,
        second_namespace.id,
        &["a".to_string(), "b".to_string(), "c".to_string()],
    )
    .await
    .unwrap();
    assert_eq!(
        isolated
            .iter()
            .map(|entry| entry.external_id.clone())
            .collect::<Vec<_>>(),
        vec!["a"]
    );
    let first_active = CacheNamespaceRepository::find_by_id(&pool, first_namespace.id)
        .await
        .unwrap()
        .unwrap()
        .active_generation
        .unwrap();
    assert_ne!(isolated[0].generation, first_active);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn source_checksum_is_bounded_and_included_in_storage_accounting() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("checksum_accounting_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let generation = create_generation(&pool, namespace.id, "checksum-v1", 1, None).await;
    let entry = CacheEntryInput {
        external_id: "record".to_string(),
        value: json!({"value": "payload"}),
        source_updated_at: None,
        source_checksum: Some("source-digest".to_string()),
    };
    let chunk = CacheIngestRepository::insert_chunk(&pool, generation.id, 0, "request", &[entry])
        .await
        .unwrap();
    let InsertCacheChunkResult::Inserted(chunk) = chunk else {
        panic!("first chunk upload must insert");
    };
    let expected: i64 = sqlx::query_scalar(
        "SELECT pg_column_size(value)::BIGINT + octet_length(external_id)::BIGINT \
             + octet_length(source_checksum)::BIGINT \
         FROM cache_entry WHERE generation = $1",
    )
    .bind(generation.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(chunk.size_bytes, expected);

    let oversized = CacheEntryInput {
        external_id: "oversized".to_string(),
        value: json!({}),
        source_updated_at: None,
        source_checksum: Some("x".repeat(1025)),
    };
    assert!(matches!(
        CacheIngestRepository::insert_chunk(&pool, generation.id, 1, "request-two", &[oversized],)
            .await,
        Err(Error::Validation(_))
    ));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn namespace_and_generation_metadata_support_keyset_pages() {
    let pool = helpers::create_test_pool().await.unwrap();
    let mut namespaces = Vec::new();
    for suffix in ["a", "b", "c"] {
        let policy = CacheNamespacePolicy {
            max_staging_generations: if suffix == "a" { 3 } else { 2 },
            ..CacheNamespacePolicy::default()
        };
        namespaces.push(
            CacheNamespaceRepository::create(
                &pool,
                namespace_input(format!("page_{suffix}_{}", unique_test_id()), policy),
            )
            .await
            .unwrap(),
        );
    }
    namespaces.sort_by_key(|namespace| namespace.id);

    let first = CacheNamespaceRepository::list_metadata_page(
        &pool,
        Some(&CacheOwnerScope::system()),
        None,
        2,
    )
    .await
    .unwrap();
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.next_after_id, Some(first.items[1].id));
    let second = CacheNamespaceRepository::list_metadata_page(
        &pool,
        Some(&CacheOwnerScope::system()),
        first.next_after_id,
        2,
    )
    .await
    .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].id, namespaces[2].id);
    assert_eq!(second.next_after_id, None);

    for index in 0..3 {
        create_generation(
            &pool,
            namespaces[0].id,
            format!("page-generation-{index}"),
            0,
            Some(0),
        )
        .await;
    }
    let first =
        CacheGenerationRepository::list_for_namespace_page(&pool, namespaces[0].id, None, 2)
            .await
            .unwrap();
    assert_eq!(first.items.len(), 2);
    let second = CacheGenerationRepository::list_for_namespace_page(
        &pool,
        namespaces[0].id,
        first.next_before,
        2,
    )
    .await
    .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(first
        .items
        .iter()
        .all(|item| second.items.iter().all(|next| next.id != item.id)));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn failed_generations_release_slots_but_continue_consuming_byte_quotas() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("failed_quota_{}", unique_test_id()),
            CacheNamespacePolicy {
                max_staging_generations: 2,
                ..CacheNamespacePolicy::default()
            },
        ),
    )
    .await
    .unwrap();
    let failed = create_generation(&pool, namespace.id, "failed-v1", 1, None).await;
    CacheIngestRepository::insert_chunk(&pool, failed.id, 0, "failed", &entries(&["a"]))
        .await
        .unwrap();
    let failed = CacheGenerationRepository::fail(&pool, failed.id, "upstream failed")
        .await
        .unwrap();
    CacheNamespaceRepository::update_policy(
        &pool,
        namespace.id,
        &CacheNamespacePolicy {
            max_retained_bytes: failed.size_bytes,
            max_staging_generations: 2,
            ..CacheNamespacePolicy::default()
        },
    )
    .await
    .unwrap();

    let next = create_generation(&pool, namespace.id, "failed-v2", 1, None).await;
    assert!(matches!(
        CacheIngestRepository::insert_chunk(&pool, next.id, 0, "next", &entries(&["b"])).await,
        Err(Error::Validation(_))
    ));
    CacheGenerationRepository::fail(&pool, next.id, "also failed")
        .await
        .unwrap();
    CacheGenerationRepository::create_or_get(
        &pool,
        &generation_input(namespace.id, "failed-v3", 1, None),
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn refresh_failure_streak_is_idempotent_and_resets_on_promotion() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("failure_streak_{}", unique_test_id()),
            CacheNamespacePolicy {
                max_staging_generations: 3,
                ..CacheNamespacePolicy::default()
            },
        ),
    )
    .await
    .unwrap();

    let first = create_generation(&pool, namespace.id, "failure-streak-1", 0, Some(0)).await;
    CacheGenerationRepository::fail(&pool, first.id, "upstream unavailable")
        .await
        .unwrap();
    CacheGenerationRepository::fail(&pool, first.id, "upstream unavailable")
        .await
        .unwrap();
    let second = create_generation(&pool, namespace.id, "failure-streak-2", 0, Some(0)).await;
    CacheGenerationRepository::fail(&pool, second.id, "upstream unavailable")
        .await
        .unwrap();

    let failed_namespace = CacheNamespaceRepository::find_by_id(&pool, namespace.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed_namespace.consecutive_refresh_failures, 2);
    assert!(failed_namespace.last_refresh_failure_at.is_some());

    let successful =
        create_generation(&pool, namespace.id, "failure-streak-success", 0, Some(0)).await;
    CacheGenerationRepository::seal(&pool, successful.id)
        .await
        .unwrap();
    CacheGenerationRepository::promote(
        &pool,
        namespace.id,
        successful.id,
        None,
        Utc::now() + Duration::minutes(10),
    )
    .await
    .unwrap();
    let recovered = CacheNamespaceRepository::find_by_id(&pool, namespace.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.consecutive_refresh_failures, 0);
    assert!(recovered.last_refresh_failure_at.is_none());
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn empty_tombstoned_namespace_cleanup_is_independently_bounded() {
    let pool = helpers::create_test_pool().await.unwrap();
    let first = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("empty_tombstone_a_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let second = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("empty_tombstone_b_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    CacheNamespaceRepository::tombstone(&pool, first.id)
        .await
        .unwrap();
    CacheNamespaceRepository::tombstone(&pool, second.id)
        .await
        .unwrap();

    assert_eq!(
        CacheNamespaceRepository::delete_empty_tombstoned_batch(&pool, 1)
            .await
            .unwrap(),
        1
    );
    let mut remaining = 0;
    for id in [first.id, second.id] {
        if CacheNamespaceRepository::find_by_id(&pool, id)
            .await
            .unwrap()
            .is_some()
        {
            remaining += 1;
        }
    }
    assert_eq!(remaining, 1);
    assert_eq!(
        CacheNamespaceRepository::delete_empty_tombstoned_batch(&pool, 1)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn large_reads_are_deduplicated_and_byte_bounded() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("read_budget_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let large_value = "x".repeat(900_000);
    let ids = (0..10)
        .map(|index| format!("id-{index}"))
        .collect::<Vec<_>>();
    let input = ids
        .iter()
        .map(|external_id| CacheEntryInput {
            external_id: external_id.clone(),
            value: json!({"payload": large_value.clone()}),
            source_updated_at: None,
            source_checksum: None,
        })
        .collect::<Vec<_>>();
    let generation = create_generation(&pool, namespace.id, "read-budget", 1, None).await;
    CacheIngestRepository::insert_chunk(&pool, generation.id, 0, "large", &input)
        .await
        .unwrap();
    let ready = CacheGenerationRepository::seal(&pool, generation.id)
        .await
        .unwrap();
    let active = CacheGenerationRepository::promote(
        &pool,
        namespace.id,
        ready.id,
        None,
        Utc::now() + Duration::minutes(10),
    )
    .await
    .unwrap()
    .activated_generation;

    assert!(matches!(
        CacheEntryRepository::find_active_many(&pool, namespace.id, &ids).await,
        Err(Error::Validation(_))
    ));
    let page = CacheEntryRepository::scan_pinned_page(&pool, namespace.id, active.id, None, 1000)
        .await
        .unwrap();
    assert!(page.has_more);
    assert!(page.entries.len() < ids.len());
    let materialized: i64 = page
        .entries
        .iter()
        .map(|entry| {
            i64::try_from(serde_json::to_vec(&entry.value).unwrap().len()).unwrap()
                + i64::try_from(entry.external_id.len()).unwrap()
                + 256
        })
        .sum();
    assert!(materialized <= MAX_SCAN_MATERIALIZATION_BYTES + 1024 * 1024 + 256);
    assert!(MAX_MULTI_LOOKUP_BYTES < materialized + 1024 * 1024);
}

/// Identical concurrent chunk uploads collapse to one insert plus idempotent
/// replays; conflicting checksums for the same index yield exactly one error.
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn concurrent_chunk_uploads_replay_or_conflict() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("concurrent_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let generation = create_generation(&pool, namespace.id, "concurrent-v1", 2, None).await;

    let identical = entries(&["x"]);
    let (pool_a, pool_b) = (pool.clone(), pool.clone());
    let (first, second) = tokio::join!(
        CacheIngestRepository::insert_chunk(&pool_a, generation.id, 0, "chunk-0", &identical),
        CacheIngestRepository::insert_chunk(&pool_b, generation.id, 0, "chunk-0", &identical),
    );
    let results = [first.unwrap(), second.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, InsertCacheChunkResult::Inserted(_)))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, InsertCacheChunkResult::Replayed(_)))
            .count(),
        1
    );

    let left_entries = entries(&["y"]);
    let right_entries = entries(&["z"]);
    let (pool_c, pool_d) = (pool.clone(), pool.clone());
    let (left, right) = tokio::join!(
        CacheIngestRepository::insert_chunk(&pool_c, generation.id, 1, "chunk-1-a", &left_entries),
        CacheIngestRepository::insert_chunk(&pool_d, generation.id, 1, "chunk-1-b", &right_entries),
    );
    assert_eq!(
        [left.is_ok(), right.is_ok()]
            .into_iter()
            .filter(|ok| *ok)
            .count(),
        1
    );
}

/// The unpublished-generation quota bounds staging and sealed-ready refreshes,
/// while a failed refresh immediately releases its writer slot.
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn max_staging_generations_quota_is_enforced() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("staging_quota_{}", unique_test_id()),
            CacheNamespacePolicy {
                max_staging_generations: 2,
                ..CacheNamespacePolicy::default()
            },
        ),
    )
    .await
    .unwrap();
    let first = create_generation(&pool, namespace.id, "stg-1", 1, Some(1)).await;
    CacheIngestRepository::insert_chunk(&pool, first.id, 0, "stg-1", &entries(&["a"]))
        .await
        .unwrap();
    CacheGenerationRepository::seal(&pool, first.id)
        .await
        .unwrap();
    create_generation(&pool, namespace.id, "stg-2", 1, None).await;
    assert!(matches!(
        CacheGenerationRepository::create_or_get(
            &pool,
            &generation_input(namespace.id, "stg-3", 1, None),
        )
        .await,
        Err(Error::Validation(_))
    ));
    CacheGenerationRepository::fail(&pool, first.id, "refresh failed")
        .await
        .unwrap();
    create_generation(&pool, namespace.id, "stg-3", 1, None).await;
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn aggregate_admission_limits_are_atomic_across_racing_writers() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace_policy = CacheNamespacePolicy::default();
    let admission = CacheAdmissionConfig {
        max_live_namespaces: 10,
        max_live_namespaces_per_owner: 1,
        max_physical_bytes: 1024 * 1024,
        max_physical_bytes_per_owner: 1024 * 1024,
        max_unpublished_generations_per_owner: 1,
    };
    let first_input = namespace_input(
        format!("aggregate_race_a_{}", unique_test_id()),
        namespace_policy.clone(),
    );
    let second_input = namespace_input(
        format!("aggregate_race_b_{}", unique_test_id()),
        namespace_policy,
    );
    let (pool_a, pool_b) = (pool.clone(), pool.clone());
    let (first, second) = tokio::join!(
        CacheNamespaceRepository::create_api_with_policy(&pool_a, first_input, &admission),
        CacheNamespaceRepository::create_api_with_policy(&pool_b, second_input, &admission),
    );
    let namespace = match (first, second) {
        (Ok(namespace), Err(rejection)) | (Err(rejection), Ok(namespace)) => {
            assert!(matches!(
                rejection,
                Error::CacheQuotaExceeded {
                    code: "cache_owner_namespace_limit_exceeded",
                    ..
                }
            ));
            namespace
        }
        results => panic!("expected one admitted namespace and one rejection: {results:?}"),
    };
    let generation_one = generation_input(namespace.id, "aggregate-generation-a", 0, Some(0));
    let generation_two = generation_input(namespace.id, "aggregate-generation-b", 0, Some(0));
    let (pool_c, pool_d) = (pool.clone(), pool.clone());
    let (first, second) = tokio::join!(
        CacheGenerationRepository::create_or_get_with_policy(&pool_c, &generation_one, &admission),
        CacheGenerationRepository::create_or_get_with_policy(&pool_d, &generation_two, &admission),
    );
    match (first, second) {
        (Ok(_), Err(rejection)) | (Err(rejection), Ok(_)) => assert!(matches!(
            rejection,
            Error::CacheQuotaExceeded {
                code: "cache_owner_unpublished_generations_limit_exceeded",
                ..
            }
        )),
        results => panic!("expected one admitted generation and one rejection: {results:?}"),
    }
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn managed_namespace_admission_is_atomic_and_updates_remain_idempotent() {
    let pool = helpers::create_test_pool().await.unwrap();
    let first_pack = PackFixture::new_unique("managed_admission_a")
        .create(&pool)
        .await
        .unwrap();
    let second_pack = PackFixture::new_unique("managed_admission_b")
        .create(&pool)
        .await
        .unwrap();
    let admission = CacheAdmissionConfig {
        max_live_namespaces: 1,
        max_live_namespaces_per_owner: 1,
        ..CacheAdmissionConfig::default()
    };
    let first_definition = ManagedCacheNamespaceDefinition {
        definition_ref: format!("{}.catalog", first_pack.r#ref),
        owner: CacheOwnerScope::pack(first_pack.id, Some(first_pack.r#ref.clone())),
        namespace: format!("managed_race_a_{}", unique_test_id()),
        policy: CacheNamespacePolicy::default(),
    };
    let second_definition = ManagedCacheNamespaceDefinition {
        definition_ref: format!("{}.catalog", second_pack.r#ref),
        owner: CacheOwnerScope::pack(second_pack.id, Some(second_pack.r#ref.clone())),
        namespace: format!("managed_race_b_{}", unique_test_id()),
        policy: CacheNamespacePolicy::default(),
    };

    let (first, second) = tokio::join!(
        CacheNamespaceRepository::upsert_managed_definitions(
            &pool,
            first_pack.id,
            &first_pack.r#ref,
            std::slice::from_ref(&first_definition),
            &admission,
        ),
        CacheNamespaceRepository::upsert_managed_definitions(
            &pool,
            second_pack.id,
            &second_pack.r#ref,
            std::slice::from_ref(&second_definition),
            &admission,
        ),
    );

    let (pack, definition) = match (first, second) {
        (Ok(summary), Err(rejection)) => {
            assert_eq!(summary.created, 1);
            assert!(matches!(
                rejection,
                Error::CacheQuotaExceeded {
                    code: "cache_global_namespace_limit_exceeded",
                    ..
                }
            ));
            (&first_pack, first_definition)
        }
        (Err(rejection), Ok(summary)) => {
            assert_eq!(summary.created, 1);
            assert!(matches!(
                rejection,
                Error::CacheQuotaExceeded {
                    code: "cache_global_namespace_limit_exceeded",
                    ..
                }
            ));
            (&second_pack, second_definition)
        }
        results => panic!("expected one managed namespace admission: {results:?}"),
    };

    let mut updated = definition;
    updated.policy.freshness_target_seconds = 60;
    let summary = CacheNamespaceRepository::upsert_managed_definitions(
        &pool,
        pack.id,
        &pack.r#ref,
        std::slice::from_ref(&updated),
        &admission,
    )
    .await
    .unwrap();
    assert_eq!(summary.updated, 1);
    let replay = CacheNamespaceRepository::upsert_managed_definitions(
        &pool,
        pack.id,
        &pack.r#ref,
        &[updated],
        &admission,
    )
    .await
    .unwrap();
    assert_eq!(replay.unchanged, 1);
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn aggregate_physical_bytes_include_staging_entries_and_roll_back_rejection() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("aggregate_bytes_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let generation = create_generation(&pool, namespace.id, "aggregate-bytes", 1, Some(1)).await;
    let admission = CacheAdmissionConfig {
        max_physical_bytes: 1,
        max_physical_bytes_per_owner: 1,
        ..CacheAdmissionConfig::default()
    };
    let result = CacheIngestRepository::insert_chunk_with_policy(
        &pool,
        generation.id,
        0,
        "aggregate-bytes",
        &entries(&["entry"]),
        &admission,
    )
    .await;
    assert!(matches!(
        result,
        Err(Error::CacheQuotaExceeded {
            code: "cache_global_physical_bytes_limit_exceeded",
            ..
        })
    ));
    let generation = CacheGenerationRepository::find_by_id(&pool, generation.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(generation.record_count, 0);
    assert_eq!(generation.size_bytes, 0);

    let (deployment_bytes, owner_bytes): (i64, i64) = sqlx::query_as(
        "SELECT deployment.physical_bytes, COALESCE(owner.physical_bytes, 0) \
         FROM cache_deployment_physical_byte_usage deployment \
         LEFT JOIN cache_owner_physical_byte_usage owner \
           ON owner.owner_type = 'system' AND owner.owner = 'system' \
         WHERE deployment.id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((deployment_bytes, owner_bytes), (0, 0));

    let inserted = CacheIngestRepository::insert_chunk(
        &pool,
        generation.id,
        0,
        "aggregate-bytes",
        &entries(&["entry"]),
    )
    .await
    .unwrap();
    let inserted_bytes = match inserted {
        InsertCacheChunkResult::Inserted(chunk) => chunk.size_bytes,
        InsertCacheChunkResult::Replayed(_) => panic!("rolled-back chunk must not replay"),
    };
    CacheGenerationRepository::fail(&pool, generation.id, "refresh failed")
        .await
        .unwrap();

    let charged: (i64, i64) = sqlx::query_as(
        "SELECT deployment.physical_bytes, owner.physical_bytes \
         FROM cache_deployment_physical_byte_usage deployment \
         JOIN cache_owner_physical_byte_usage owner \
           ON owner.owner_type = 'system' AND owner.owner = 'system' \
         WHERE deployment.id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(charged, (inserted_bytes, inserted_bytes));

    let replacement =
        create_generation(&pool, namespace.id, "aggregate-bytes-next", 1, Some(1)).await;
    let charged_admission = CacheAdmissionConfig {
        max_physical_bytes: inserted_bytes,
        max_physical_bytes_per_owner: inserted_bytes,
        ..CacheAdmissionConfig::default()
    };
    assert!(matches!(
        CacheIngestRepository::insert_chunk_with_policy(
            &pool,
            replacement.id,
            0,
            "aggregate-bytes-next",
            &entries(&["next"]),
            &charged_admission,
        )
        .await,
        Err(Error::CacheQuotaExceeded {
            code: "cache_global_physical_bytes_limit_exceeded",
            ..
        })
    ));

    assert_eq!(
        CacheEntryRepository::delete_cleanup_batch(&pool, generation.id, 10)
            .await
            .unwrap(),
        1
    );
    let released: (i64, i64) = sqlx::query_as(
        "SELECT deployment.physical_bytes, owner.physical_bytes \
         FROM cache_deployment_physical_byte_usage deployment \
         JOIN cache_owner_physical_byte_usage owner \
           ON owner.owner_type = 'system' AND owner.owner = 'system' \
         WHERE deployment.id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(released, (0, 0));
}

/// Promotion is blocked by retained-generation count, while aggregate bytes
/// are rejected earlier during staging admission.
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn retained_generation_and_aggregate_byte_quotas_are_enforced() {
    let pool = helpers::create_test_pool().await.unwrap();

    // Retained generation count.
    let count_namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("retained_count_{}", unique_test_id()),
            CacheNamespacePolicy {
                max_retained_generations: 2,
                ..CacheNamespacePolicy::default()
            },
        ),
    )
    .await
    .unwrap();
    let generation_one =
        publish_generation(&pool, count_namespace.id, "retc-1", &["a"], None).await;
    let generation_two = publish_generation(
        &pool,
        count_namespace.id,
        "retc-2",
        &["b"],
        Some(generation_one.id),
    )
    .await;
    let generation_three = seal_generation(&pool, count_namespace.id, "retc-3", &["c"]).await;
    assert!(matches!(
        CacheGenerationRepository::promote(
            &pool,
            count_namespace.id,
            generation_three.id,
            Some(generation_two.id),
            Utc::now() + Duration::minutes(10),
        )
        .await,
        Err(Error::Validation(_))
    ));

    // Retained bytes: publish one generation, then shrink the byte budget to
    // exactly one generation's size so the next promotion cannot fit.
    let byte_namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("retained_bytes_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let published =
        publish_generation(&pool, byte_namespace.id, "retb-1", &["a", "b", "c"], None).await;
    CacheNamespaceRepository::update_policy(
        &pool,
        byte_namespace.id,
        &CacheNamespacePolicy {
            max_retained_bytes: published.size_bytes,
            ..CacheNamespacePolicy::default()
        },
    )
    .await
    .unwrap();
    let next = create_generation(&pool, byte_namespace.id, "retb-2", 1, None).await;
    assert!(matches!(
        CacheIngestRepository::insert_chunk(
            &pool,
            next.id,
            0,
            "retb-2",
            &entries(&["a", "b", "c"]),
        )
        .await,
        Err(Error::Validation(_))
    ));
}

/// Database triggers reject invalid generation transitions and entry writes to
/// non-staging generations even when the repository layer is bypassed.
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn direct_db_state_transition_and_staging_trigger_are_rejected() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("trigger_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();

    // staging -> active and staging -> retired are not valid transitions.
    let staging = create_generation(&pool, namespace.id, "trigger-staging", 1, None).await;
    assert!(
        sqlx::query("UPDATE cache_generation SET state = 'active' WHERE id = $1")
            .bind(staging.id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE cache_generation SET state = 'retired' WHERE id = $1")
            .bind(staging.id)
            .execute(&pool)
            .await
            .is_err()
    );

    // Entries may only be inserted into staging generations.
    let ready = seal_generation(&pool, namespace.id, "trigger-ready", &["a"]).await;
    assert!(sqlx::query(
        "INSERT INTO cache_entry (generation, external_id, value, size_bytes) \
         VALUES ($1, $2, $3::jsonb, $4)"
    )
    .bind(ready.id)
    .bind("late")
    .bind(json!({"id": "late"}))
    .bind(10_i64)
    .execute(&pool)
    .await
    .is_err());
}

/// Tombstoning a namespace concurrently with an upload and with a seal never
/// deadlocks, because all three acquire the namespace lock before the
/// generation lock.
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn tombstone_races_upload_and_seal_without_deadlock() {
    let pool = helpers::create_test_pool().await.unwrap();

    // Tombstone concurrent with an in-flight chunk upload.
    let upload_namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("race_upload_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let uploading = create_generation(&pool, upload_namespace.id, "race-upload", 3, None).await;
    CacheIngestRepository::insert_chunk(&pool, uploading.id, 0, "c0", &entries(&["a"]))
        .await
        .unwrap();
    let upload_entries = entries(&["b"]);
    let (pool_a, pool_b) = (pool.clone(), pool.clone());
    let (upload, tombstone) = tokio::join!(
        CacheIngestRepository::insert_chunk(&pool_a, uploading.id, 1, "c1", &upload_entries),
        CacheNamespaceRepository::tombstone(&pool_b, upload_namespace.id),
    );
    assert!(tombstone.unwrap());
    if let Err(err) = &upload {
        assert!(!is_deadlock(err), "upload deadlocked with tombstone: {err}");
    }
    assert!(
        CacheNamespaceRepository::find_by_id(&pool, upload_namespace.id)
            .await
            .unwrap()
            .unwrap()
            .tombstoned_at
            .is_some()
    );

    // Tombstone concurrent with a seal.
    let seal_namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("race_seal_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let sealing = create_generation(&pool, seal_namespace.id, "race-seal", 1, Some(1)).await;
    CacheIngestRepository::insert_chunk(&pool, sealing.id, 0, "s0", &entries(&["x"]))
        .await
        .unwrap();
    let (pool_c, pool_d) = (pool.clone(), pool.clone());
    let (seal, tombstone_two) = tokio::join!(
        CacheGenerationRepository::seal(&pool_c, sealing.id),
        CacheNamespaceRepository::tombstone(&pool_d, seal_namespace.id),
    );
    assert!(tombstone_two.unwrap());
    if let Err(err) = &seal {
        assert!(!is_deadlock(err), "seal deadlocked with tombstone: {err}");
    }
    assert!(
        CacheNamespaceRepository::find_by_id(&pool, seal_namespace.id)
            .await
            .unwrap()
            .unwrap()
            .tombstoned_at
            .is_some()
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn cleanup_waits_for_a_reader_pinned_before_expiry() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("cleanup_reader_lock_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let first = publish_generation(&pool, namespace.id, "reader-lock-first", &["a"], None).await;
    let second = seal_generation(&pool, namespace.id, "reader-lock-second", &["b"]).await;
    let prior_readable_until: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT clock_timestamp() + INTERVAL '500 milliseconds'")
            .fetch_one(&pool)
            .await
            .unwrap();
    CacheGenerationRepository::promote(
        &pool,
        namespace.id,
        second.id,
        Some(first.id),
        prior_readable_until,
    )
    .await
    .unwrap();

    let mut reader_tx = pool.begin().await.unwrap();
    CacheGenerationRepository::find_by_id_for_share(&mut reader_tx, first.id)
        .await
        .unwrap()
        .expect("generation must exist before pin creation");
    let database_now: chrono::DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        database_now < prior_readable_until,
        "reader must acquire its pin before the generation expires"
    );
    // Use the database clock for both the deadline and wait to avoid a cross-clock boundary race.
    sqlx::query(
        "SELECT pg_sleep(\
             GREATEST(EXTRACT(EPOCH FROM ($1::timestamptz - clock_timestamp())), 0)::double precision \
             + 0.05\
         )",
    )
    .bind(prior_readable_until)
    .execute(&pool)
    .await
    .unwrap();

    let cleanup_pool = pool.clone();
    let mut cleanup = tokio::spawn(async move {
        CacheEntryRepository::delete_cleanup_batch(&cleanup_pool, first.id, 10).await
    });
    if let Ok(result) =
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut cleanup).await
    {
        panic!("cleanup must wait for the pinned reader's generation share lock, got {result:?}");
    }

    reader_tx.commit().await.unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), cleanup)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        1
    );
}

/// Both within-chunk and cross-chunk duplicate external ids surface the typed,
/// identifier-free ingestion error rather than a raw database error.
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn duplicate_external_ids_yield_typed_error() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("dupid_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let generation = create_generation(&pool, namespace.id, "dupid-v1", 3, None).await;

    // Duplicate within one chunk.
    assert!(matches!(
        CacheIngestRepository::insert_chunk(
            &pool,
            generation.id,
            0,
            "dupid-0",
            &entries(&["dup", "dup"]),
        )
        .await,
        Err(Error::CacheDuplicateExternalId)
    ));

    // Duplicate across chunks in the same generation.
    CacheIngestRepository::insert_chunk(&pool, generation.id, 0, "clean-0", &entries(&["k"]))
        .await
        .unwrap();
    assert!(matches!(
        CacheIngestRepository::insert_chunk(&pool, generation.id, 1, "cross-1", &entries(&["k"]),)
            .await,
        Err(Error::CacheDuplicateExternalId)
    ));
}

/// A readable snapshot with no records is authoritative: the scan returns an
/// empty page rather than `CacheSnapshotExpired`. Readability is decided by
/// generation state, not record count, so end-of-scan traversals never fail on
/// their final (empty) page.
#[tokio::test]
#[ignore = "integration test — requires database"]
async fn scan_pinned_returns_empty_page_for_zero_record_snapshot() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = CacheNamespaceRepository::create(
        &pool,
        namespace_input(
            format!("zero_record_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();

    // Seal and publish a generation that declares (and contains) zero records.
    let staged = match CacheGenerationRepository::create_or_get(
        &pool,
        &generation_input(namespace.id, "zero-record", 0, Some(0)),
    )
    .await
    .unwrap()
    {
        CreateCacheGenerationResult::Created(generation)
        | CreateCacheGenerationResult::Existing(generation) => generation,
    };
    let ready = CacheGenerationRepository::seal(&pool, staged.id)
        .await
        .unwrap();
    assert_eq!(ready.record_count, 0);
    let active = CacheGenerationRepository::promote(
        &pool,
        namespace.id,
        ready.id,
        None,
        Utc::now() + Duration::minutes(10),
    )
    .await
    .unwrap()
    .activated_generation;

    // Active zero-record snapshot: an authoritative empty page, not an error.
    assert!(
        CacheEntryRepository::scan_pinned(&pool, namespace.id, active.id, None, 10)
            .await
            .unwrap()
            .is_empty()
    );

    // The same holds once it is retired but still within its readable window.
    let successor = seal_generation(&pool, namespace.id, "zero-record-successor", &["a"]).await;
    CacheGenerationRepository::promote(
        &pool,
        namespace.id,
        successor.id,
        Some(active.id),
        Utc::now() + Duration::minutes(10),
    )
    .await
    .unwrap();
    assert!(
        CacheEntryRepository::scan_pinned(&pool, namespace.id, active.id, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

fn write_pack_file(root: &std::path::Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

async fn create_core_native_runtime(pool: &PgPool) {
    let core = PackFixture::new("core").create(pool).await.unwrap();
    RuntimeFixture::new(Some(core.id), Some(core.r#ref), "native")
        .create(pool)
        .await
        .unwrap();
}

fn write_pack_owner_components(root: &std::path::Path, pack_ref: &str) {
    write_pack_file(
        root,
        "actions/refresh.yaml",
        &format!(
            "ref: {pack_ref}.refresh\nlabel: Refresh\nrunner_type: native\n\
             entry_point: refresh\n"
        ),
    );
    write_pack_file(
        root,
        "sensors/watcher.yaml",
        &format!(
            "ref: {pack_ref}.watcher\nlabel: Watcher\nrunner_type: native\n\
             entry_point: watcher\n"
        ),
    );
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn late_pack_component_failure_rolls_back_metadata_and_components() {
    let pool = helpers::create_test_pool().await.unwrap();
    create_core_native_runtime(&pool).await;
    let pack = PackFixture::new_unique("atomic_pack_reload")
        .create(&pool)
        .await
        .unwrap();
    let pack_ref = pack.r#ref.clone();
    let action_ref = format!("{pack_ref}.refresh");
    let temp = TempDir::new().unwrap();
    write_pack_owner_components(temp.path(), &pack_ref);
    PackComponentLoader::new(&pool, pack.id, &pack_ref, &CacheAdmissionConfig::default())
        .load_all(temp.path())
        .await
        .unwrap();

    write_pack_file(
        temp.path(),
        "actions/refresh.yaml",
        &format!("ref: {action_ref}\nlabel: Changed\nrunner_type: native\nentry_point: refresh\n"),
    );
    let definition_ref = format!("{pack_ref}.late_failure");
    write_pack_file(
        temp.path(),
        "caches/late_failure.yaml",
        &format!(
            "ref: {definition_ref}\nnamespace: late_failure\nowner_type: pack\nowner_ref: {pack_ref}\n"
        ),
    );

    let admission = CacheAdmissionConfig {
        max_live_namespaces: 0,
        ..CacheAdmissionConfig::default()
    };
    let loader = PackComponentLoader::new(&pool, pack.id, &pack_ref, &admission);
    let mut tx = pool.begin().await.unwrap();
    PackRepository::update(
        &mut *tx,
        pack.id,
        UpdatePackInput {
            version: Some("99.0.0".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(loader
        .load_all_in_transaction(&mut tx, temp.path())
        .await
        .is_err());
    tx.rollback().await.unwrap();

    let preserved_pack = PackRepository::find_by_id(&pool, pack.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(preserved_pack.version, pack.version);
    let preserved_action = ActionRepository::find_by_ref(&pool, &action_ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(preserved_action.label, "Refresh");
    assert!(CacheNamespaceRepository::resolve_managed_definition(
        &pool,
        &pack_ref,
        &definition_ref,
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn pack_cache_loader_manages_owners_updates_removal_and_reinstall() {
    let pool = helpers::create_test_pool().await.unwrap();
    create_core_native_runtime(&pool).await;
    let pack = PackFixture::new_unique("cache_pack")
        .create(&pool)
        .await
        .unwrap();
    let pack_ref = pack.r#ref.clone();
    let action_ref = format!("{pack_ref}.refresh");
    let sensor_ref = format!("{pack_ref}.watcher");
    let pack_definition_ref = format!("{pack_ref}.catalog");
    let action_definition_ref = format!("{pack_ref}.action_catalog");
    let sensor_definition_ref = format!("{pack_ref}.sensor_catalog");
    let temp = TempDir::new().unwrap();

    write_pack_owner_components(temp.path(), &pack_ref);
    write_pack_file(
        temp.path(),
        "caches/catalog.yaml",
        &format!(
            "ref: {pack_definition_ref}\nnamespace: catalog\nowner_type: pack\n\
             owner_ref: {pack_ref}\nfreshness_target_seconds: 300\n"
        ),
    );
    write_pack_file(
        temp.path(),
        "caches/action_catalog.yaml",
        &format!(
            "ref: {action_definition_ref}\nnamespace: action_catalog\nowner_type: action\n\
             owner_ref: {action_ref}\n"
        ),
    );
    write_pack_file(
        temp.path(),
        "caches/sensor_catalog.yaml",
        &format!(
            "ref: {sensor_definition_ref}\nnamespace: sensor_catalog\nowner_type: sensor\n\
             owner_ref: {sensor_ref}\n"
        ),
    );

    let loader =
        PackComponentLoader::new(&pool, pack.id, &pack_ref, &CacheAdmissionConfig::default());
    let initial = loader.load_all(temp.path()).await.unwrap();
    assert_eq!(initial.caches_loaded, 3);

    let action = ActionRepository::find_by_ref(&pool, &action_ref)
        .await
        .unwrap()
        .unwrap();
    let sensor = SensorRepository::find_by_ref(&pool, &sensor_ref)
        .await
        .unwrap()
        .unwrap();
    let pack_namespace = CacheNamespaceRepository::resolve_managed_definition(
        &pool,
        &pack_ref,
        &pack_definition_ref,
    )
    .await
    .unwrap()
    .unwrap();
    let action_namespace = CacheNamespaceRepository::resolve_managed_definition(
        &pool,
        &pack_ref,
        &action_definition_ref,
    )
    .await
    .unwrap()
    .unwrap();
    let sensor_namespace = CacheNamespaceRepository::resolve_managed_definition(
        &pool,
        &pack_ref,
        &sensor_definition_ref,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(pack_namespace.owner_type, OwnerType::Pack);
    assert_eq!(pack_namespace.owner_pack, Some(pack.id));
    assert_eq!(action_namespace.owner_action, Some(action.id));
    assert_eq!(sensor_namespace.owner_sensor, Some(sensor.id));

    let active =
        publish_generation(&pool, pack_namespace.id, "pack-cache-v1", &["one"], None).await;
    write_pack_file(
        temp.path(),
        "caches/catalog.yaml",
        &format!(
            "ref: {pack_definition_ref}\nnamespace: catalog\nowner_type: pack\n\
             owner_ref: {pack_ref}\nfreshness_target_seconds: 45\n"
        ),
    );
    let updated = loader.load_all(temp.path()).await.unwrap();
    assert_eq!(updated.caches_updated, 1);
    let policy_updated = CacheNamespaceRepository::resolve_managed_definition(
        &pool,
        &pack_ref,
        &pack_definition_ref,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(policy_updated.id, pack_namespace.id);
    assert_eq!(policy_updated.active_generation, Some(active.id));
    assert_eq!(policy_updated.freshness_target_seconds, 45);

    let api_namespace = CacheNamespaceRepository::create_api(
        &pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::pack(pack.id, Some(pack_ref.clone())),
            namespace: "api_catalog".to_string(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap();

    fs::remove_file(temp.path().join("caches/catalog.yaml")).unwrap();
    loader.load_all(temp.path()).await.unwrap();
    let removed = CacheNamespaceRepository::find_by_id(&pool, pack_namespace.id)
        .await
        .unwrap()
        .unwrap();
    assert!(removed.tombstoned_at.is_some());
    assert_eq!(
        CacheNamespaceRepository::resolve(
            &pool,
            &CacheOwnerScope::pack(pack.id, Some(pack_ref.clone())),
            "api_catalog",
        )
        .await
        .unwrap()
        .unwrap()
        .id,
        api_namespace.id
    );

    write_pack_file(
        temp.path(),
        "caches/catalog.yaml",
        &format!(
            "ref: {pack_definition_ref}\nnamespace: catalog\nowner_type: pack\n\
             owner_ref: {pack_ref}\nfreshness_target_seconds: 45\n"
        ),
    );
    loader.load_all(temp.path()).await.unwrap();
    let reinstalled = CacheNamespaceRepository::resolve_managed_definition(
        &pool,
        &pack_ref,
        &pack_definition_ref,
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(reinstalled.id, pack_namespace.id);
    assert_eq!(
        CacheEntryRepository::delete_cleanup_batch(&pool, active.id, 100)
            .await
            .unwrap(),
        1
    );
    assert!(CacheGenerationRepository::delete_if_empty(&pool, active.id)
        .await
        .unwrap());
    assert!(
        CacheNamespaceRepository::delete_tombstoned_if_empty(&pool, pack_namespace.id)
            .await
            .unwrap()
    );
    assert_eq!(
        CacheNamespaceRepository::resolve_managed_definition(
            &pool,
            &pack_ref,
            &pack_definition_ref,
        )
        .await
        .unwrap()
        .unwrap()
        .id,
        reinstalled.id
    );

    let api_action_namespace = CacheNamespaceRepository::create_api(
        &pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::action(action.id, Some(action_ref.clone())),
            namespace: "api_action".to_string(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap();
    let api_sensor_namespace = CacheNamespaceRepository::create_api(
        &pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::sensor(sensor.id, Some(sensor_ref.clone())),
            namespace: "api_sensor".to_string(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap();

    fs::remove_file(temp.path().join("actions/refresh.yaml")).unwrap();
    fs::remove_file(temp.path().join("sensors/watcher.yaml")).unwrap();
    let owner_removed = loader.load_all(temp.path()).await.unwrap();
    assert!(owner_removed.caches_skipped >= 2);
    assert!(ActionRepository::find_by_ref(&pool, &action_ref)
        .await
        .unwrap()
        .is_none());
    assert!(SensorRepository::find_by_ref(&pool, &sensor_ref)
        .await
        .unwrap()
        .is_none());

    for (id, owner_type, expected_ref) in [
        (action_namespace.id, OwnerType::Action, action_ref.as_str()),
        (sensor_namespace.id, OwnerType::Sensor, sensor_ref.as_str()),
        (
            api_action_namespace.id,
            OwnerType::Action,
            action_ref.as_str(),
        ),
        (
            api_sensor_namespace.id,
            OwnerType::Sensor,
            sensor_ref.as_str(),
        ),
    ] {
        let namespace = CacheNamespaceRepository::find_by_id(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert!(namespace.tombstoned_at.is_some());
        match owner_type {
            OwnerType::Action => {
                assert!(namespace.owner_action.is_none());
                assert_eq!(namespace.owner_action_ref.as_deref(), Some(expected_ref));
            }
            OwnerType::Sensor => {
                assert!(namespace.owner_sensor.is_none());
                assert_eq!(namespace.owner_sensor_ref.as_deref(), Some(expected_ref));
            }
            _ => unreachable!(),
        }
    }
    assert!(
        CacheNamespaceRepository::find_by_id(&pool, api_namespace.id)
            .await
            .unwrap()
            .unwrap()
            .tombstoned_at
            .is_none()
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn stale_owner_cleanup_rolls_back_when_cache_tombstoning_fails() {
    let pool = helpers::create_test_pool().await.unwrap();
    create_core_native_runtime(&pool).await;
    let pack = PackFixture::new_unique("atomic_owner_cleanup")
        .create(&pool)
        .await
        .unwrap();
    let pack_ref = pack.r#ref.clone();
    let action_ref = format!("{pack_ref}.refresh");
    let sensor_ref = format!("{pack_ref}.watcher");
    let temp = TempDir::new().unwrap();
    write_pack_owner_components(temp.path(), &pack_ref);

    let loader =
        PackComponentLoader::new(&pool, pack.id, &pack_ref, &CacheAdmissionConfig::default());
    loader.load_all(temp.path()).await.unwrap();
    let action = ActionRepository::find_by_ref(&pool, &action_ref)
        .await
        .unwrap()
        .unwrap();
    let sensor = SensorRepository::find_by_ref(&pool, &sensor_ref)
        .await
        .unwrap()
        .unwrap();
    let action_namespace = CacheNamespaceRepository::create_api(
        &pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::action(action.id, Some(action_ref.clone())),
            namespace: "action_data".to_string(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap();
    let sensor_namespace = CacheNamespaceRepository::create_api(
        &pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::sensor(sensor.id, Some(sensor_ref.clone())),
            namespace: "sensor_data".to_string(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE FUNCTION reject_owner_cache_tombstone()
        RETURNS TRIGGER AS $$
        BEGIN
            IF OLD.tombstoned_at IS NULL
               AND NEW.tombstoned_at IS NOT NULL
               AND (OLD.owner_action IS NOT NULL OR OLD.owner_sensor IS NOT NULL) THEN
                RAISE EXCEPTION 'test cache tombstone failure';
            END IF;
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_owner_cache_tombstone_trigger \
         BEFORE UPDATE ON cache_namespace FOR EACH ROW \
         EXECUTE FUNCTION reject_owner_cache_tombstone()",
    )
    .execute(&pool)
    .await
    .unwrap();

    fs::remove_file(temp.path().join("actions/refresh.yaml")).unwrap();
    fs::remove_file(temp.path().join("sensors/watcher.yaml")).unwrap();
    let failed_cleanup = loader.load_all(temp.path()).await.unwrap();
    assert!(failed_cleanup
        .warnings
        .iter()
        .any(|warning| warning.contains("Failed to atomically clean up stale cache owners")));

    assert!(ActionRepository::find_by_ref(&pool, &action_ref)
        .await
        .unwrap()
        .is_some());
    assert!(SensorRepository::find_by_ref(&pool, &sensor_ref)
        .await
        .unwrap()
        .is_some());
    for namespace_id in [action_namespace.id, sensor_namespace.id] {
        let namespace = CacheNamespaceRepository::find_by_id(&pool, namespace_id)
            .await
            .unwrap()
            .unwrap();
        assert!(namespace.tombstoned_at.is_none());
    }

    sqlx::query("DROP TRIGGER reject_owner_cache_tombstone_trigger ON cache_namespace")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION reject_owner_cache_tombstone()")
        .execute(&pool)
        .await
        .unwrap();

    let successful_cleanup = loader.load_all(temp.path()).await.unwrap();
    assert!(!successful_cleanup
        .warnings
        .iter()
        .any(|warning| warning.contains("Failed to atomically clean up stale cache owners")));
    assert!(ActionRepository::find_by_ref(&pool, &action_ref)
        .await
        .unwrap()
        .is_none());
    assert!(SensorRepository::find_by_ref(&pool, &sensor_ref)
        .await
        .unwrap()
        .is_none());
    for namespace_id in [action_namespace.id, sensor_namespace.id] {
        let namespace = CacheNamespaceRepository::find_by_id(&pool, namespace_id)
            .await
            .unwrap()
            .unwrap();
        assert!(namespace.tombstoned_at.is_some());
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn managed_cache_definition_identity_is_immutable() {
    let pool = helpers::create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("immutable_cache")
        .create(&pool)
        .await
        .unwrap();
    let definition_ref = format!("{}.catalog", pack.r#ref);
    let initial = ManagedCacheNamespaceDefinition {
        definition_ref: definition_ref.clone(),
        owner: CacheOwnerScope::pack(pack.id, Some(pack.r#ref.clone())),
        namespace: "catalog".to_string(),
        policy: CacheNamespacePolicy::default(),
    };
    CacheNamespaceRepository::upsert_managed_definitions(
        &pool,
        pack.id,
        &pack.r#ref,
        std::slice::from_ref(&initial),
        &CacheAdmissionConfig::default(),
    )
    .await
    .unwrap();
    let original =
        CacheNamespaceRepository::resolve_managed_definition(&pool, &pack.r#ref, &definition_ref)
            .await
            .unwrap()
            .unwrap();

    let changed = ManagedCacheNamespaceDefinition {
        namespace: "renamed".to_string(),
        ..initial
    };
    assert!(CacheNamespaceRepository::upsert_managed_definitions(
        &pool,
        pack.id,
        &pack.r#ref,
        &[changed],
        &CacheAdmissionConfig::default(),
    )
    .await
    .is_err());
    let preserved =
        CacheNamespaceRepository::resolve_managed_definition(&pool, &pack.r#ref, &definition_ref)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(preserved.id, original.id);
    assert_eq!(preserved.namespace, "catalog");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn api_namespace_create_still_conflicts_while_tombstone_drains() {
    let pool = helpers::create_test_pool().await.unwrap();
    let namespace = format!("api_conflict_{}", unique_test_id());
    let input = namespace_input(namespace, CacheNamespacePolicy::default());
    let created = CacheNamespaceRepository::create_api(&pool, input.clone())
        .await
        .unwrap();
    CacheNamespaceRepository::tombstone(&pool, created.id)
        .await
        .unwrap();
    assert!(CacheNamespaceRepository::create_api(&pool, input)
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn direct_action_and_sensor_deletion_tombstones_owned_caches() {
    let pool = helpers::create_test_pool().await.unwrap();
    create_core_native_runtime(&pool).await;
    let pack = PackFixture::new_unique("direct_owner_delete")
        .create(&pool)
        .await
        .unwrap();
    let pack_ref = pack.r#ref.clone();
    let action_ref = format!("{pack_ref}.refresh");
    let sensor_ref = format!("{pack_ref}.watcher");
    let temp = TempDir::new().unwrap();
    write_pack_owner_components(temp.path(), &pack_ref);
    PackComponentLoader::new(&pool, pack.id, &pack_ref, &CacheAdmissionConfig::default())
        .load_all(temp.path())
        .await
        .unwrap();
    let action = ActionRepository::find_by_ref(&pool, &action_ref)
        .await
        .unwrap()
        .unwrap();
    let sensor = SensorRepository::find_by_ref(&pool, &sensor_ref)
        .await
        .unwrap()
        .unwrap();
    let action_namespace = CacheNamespaceRepository::create_api(
        &pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::action(action.id, Some(action_ref.clone())),
            namespace: "action_data".to_string(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap();
    let sensor_namespace = CacheNamespaceRepository::create_api(
        &pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::sensor(sensor.id, Some(sensor_ref.clone())),
            namespace: "sensor_data".to_string(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap();

    let mut action_tx = pool.begin().await.unwrap();
    assert_eq!(
        CacheNamespaceRepository::tombstone_for_action_deletion(&mut action_tx, action.id)
            .await
            .unwrap(),
        1
    );
    assert!(ActionRepository::delete(&mut *action_tx, action.id)
        .await
        .unwrap());
    action_tx.commit().await.unwrap();

    let mut sensor_tx = pool.begin().await.unwrap();
    assert_eq!(
        CacheNamespaceRepository::tombstone_for_sensor_deletion(&mut sensor_tx, sensor.id)
            .await
            .unwrap(),
        1
    );
    assert!(SensorRepository::delete(&mut *sensor_tx, sensor.id)
        .await
        .unwrap());
    sensor_tx.commit().await.unwrap();

    let action_after = CacheNamespaceRepository::find_by_id(&pool, action_namespace.id)
        .await
        .unwrap()
        .unwrap();
    assert!(action_after.tombstoned_at.is_some());
    assert!(action_after.owner_action.is_none());
    assert_eq!(
        action_after.owner_action_ref.as_deref(),
        Some(action_ref.as_str())
    );
    let sensor_after = CacheNamespaceRepository::find_by_id(&pool, sensor_namespace.id)
        .await
        .unwrap()
        .unwrap();
    assert!(sensor_after.tombstoned_at.is_some());
    assert!(sensor_after.owner_sensor.is_none());
    assert_eq!(
        sensor_after.owner_sensor_ref.as_deref(),
        Some(sensor_ref.as_str())
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn pack_deletion_tombstones_owned_caches_without_synchronous_drain() {
    let pool = helpers::create_test_pool().await.unwrap();
    create_core_native_runtime(&pool).await;
    let pack = PackFixture::new_unique("delete_cache_pack")
        .create(&pool)
        .await
        .unwrap();
    let pack_ref = pack.r#ref.clone();
    let action_ref = format!("{pack_ref}.refresh");
    let sensor_ref = format!("{pack_ref}.watcher");
    let temp = TempDir::new().unwrap();
    write_pack_owner_components(temp.path(), &pack_ref);
    let loader =
        PackComponentLoader::new(&pool, pack.id, &pack_ref, &CacheAdmissionConfig::default());
    loader.load_all(temp.path()).await.unwrap();
    let action = ActionRepository::find_by_ref(&pool, &action_ref)
        .await
        .unwrap()
        .unwrap();
    let sensor = SensorRepository::find_by_ref(&pool, &sensor_ref)
        .await
        .unwrap()
        .unwrap();

    let definition_ref = format!("{pack_ref}.managed");
    CacheNamespaceRepository::upsert_managed_definitions(
        &pool,
        pack.id,
        &pack_ref,
        &[ManagedCacheNamespaceDefinition {
            definition_ref: definition_ref.clone(),
            owner: CacheOwnerScope::pack(pack.id, Some(pack_ref.clone())),
            namespace: "managed".to_string(),
            policy: CacheNamespacePolicy::default(),
        }],
        &CacheAdmissionConfig::default(),
    )
    .await
    .unwrap();
    let managed =
        CacheNamespaceRepository::resolve_managed_definition(&pool, &pack_ref, &definition_ref)
            .await
            .unwrap()
            .unwrap();
    let active = publish_generation(&pool, managed.id, "managed-v1", &["one"], None).await;
    let api_action = CacheNamespaceRepository::create_api(
        &pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::action(action.id, Some(action_ref.clone())),
            namespace: "api_action".to_string(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap();
    let api_sensor = CacheNamespaceRepository::create_api(
        &pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::sensor(sensor.id, Some(sensor_ref.clone())),
            namespace: "api_sensor".to_string(),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        CacheNamespaceRepository::tombstone_for_pack_deletion(&mut tx, pack.id)
            .await
            .unwrap(),
        3
    );
    assert!(PackRepository::delete(&mut *tx, pack.id).await.unwrap());
    tx.commit().await.unwrap();

    assert!(PackRepository::find_by_ref(&pool, &pack_ref)
        .await
        .unwrap()
        .is_none());
    let managed_after = CacheNamespaceRepository::find_by_id(&pool, managed.id)
        .await
        .unwrap()
        .unwrap();
    assert!(managed_after.tombstoned_at.is_some());
    assert!(managed_after.owner_pack.is_none());
    assert!(managed_after.managing_pack.is_none());
    assert_eq!(
        managed_after.owner_pack_ref.as_deref(),
        Some(pack_ref.as_str())
    );
    assert_eq!(
        managed_after.managing_pack_ref.as_deref(),
        Some(pack_ref.as_str())
    );
    assert_eq!(
        CacheGenerationRepository::find_by_id(&pool, active.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        CacheGenerationState::Retired
    );
    for (id, owner_type) in [
        (api_action.id, OwnerType::Action),
        (api_sensor.id, OwnerType::Sensor),
    ] {
        let namespace = CacheNamespaceRepository::find_by_id(&pool, id)
            .await
            .unwrap()
            .unwrap();
        assert!(namespace.tombstoned_at.is_some());
        match owner_type {
            OwnerType::Action => assert!(namespace.owner_action.is_none()),
            OwnerType::Sensor => assert!(namespace.owner_sensor.is_none()),
            _ => unreachable!(),
        }
    }
}

fn is_deadlock(err: &Error) -> bool {
    matches!(
        err,
        Error::Database(sqlx::Error::Database(db)) if db.code().as_deref() == Some("40P01")
    )
}
