//! Shared existing assertions: same bodies under owner and split-principal fixtures.
use super::helpers::unique_test_id;
use attune_common::{
    config::{CacheAdmissionConfig, CacheRetentionConfig},
    models::CacheRefreshConcurrency,
    repositories::{
        cache::{
            CacheEntryInput, CacheEntryRepository, CacheGenerationCleanupOutcome,
            CacheGenerationRepository, CacheIngestRepository, CacheNamespacePolicy,
            CacheNamespaceRepository, CacheOwnerScope, CreateCacheGenerationInput,
            CreateCacheGenerationResult, CreateCacheNamespaceInput,
        },
        Create, FindById,
    },
};
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
fn namespace_input(namespace: String, policy: CacheNamespacePolicy) -> CreateCacheNamespaceInput {
    CreateCacheNamespaceInput {
        mode: Default::default(),
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
        created_by_execution: None,
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

async fn reclaim_generation(pool: &PgPool, generation: i64) -> attune_common::Result<u64> {
    let config = CacheRetentionConfig {
        min_traversal_window_seconds: 0,
        ..CacheRetentionConfig::default()
    };
    match CacheGenerationRepository::drop_if_cleanup_eligible(pool, generation, &config).await? {
        CacheGenerationCleanupOutcome::Dropped { records, .. } => Ok(records),
        CacheGenerationCleanupOutcome::Absent | CacheGenerationCleanupOutcome::Ineligible => Ok(0),
        outcome => panic!("unexpected cleanup deferral: {outcome:?}"),
    }
}

pub(super) async fn promotion_is_optimistic_and_pinned_reads_do_not_mix_generations(
    pool: &PgPool,
    _maintenance: &PgPool,
) {
    let namespace = CacheNamespaceRepository::create(
        pool,
        namespace_input(
            format!("promote_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let first = publish_generation(pool, namespace.id, "first", &["a", "b"], None).await;
    let second_ready = seal_generation(pool, namespace.id, "second", &["c", "d"]).await;

    CacheGenerationRepository::promote(
        pool,
        namespace.id,
        second_ready.id,
        Some(first.id),
        Utc::now() + Duration::minutes(10),
    )
    .await
    .unwrap();
    let pinned_first =
        CacheEntryRepository::scan_pinned(pool, namespace.id, first.id, Some("a"), 10)
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
        CacheEntryRepository::find_active(pool, namespace.id, "c")
            .await
            .unwrap()
            .unwrap()
            .external_id,
        "c"
    );

    let third = seal_generation(pool, namespace.id, "third", &["e"]).await;
    let fourth = seal_generation(pool, namespace.id, "fourth", &["f"]).await;
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

pub(super) async fn refresh_reuse_race_preserves_one_producer_and_upload_contract(
    pool: &PgPool,
    _maintenance: &PgPool,
) {
    let namespace = CacheNamespaceRepository::create(
        pool,
        namespace_input(
            format!("reuse_{}", unique_test_id()),
            CacheNamespacePolicy {
                refresh_concurrency: CacheRefreshConcurrency::Reuse,
                max_staging_generations: 1,
                ..CacheNamespacePolicy::default()
            },
        ),
    )
    .await
    .unwrap();
    let mut left = generation_input(namespace.id, "producer-left", 1, Some(1));
    left.created_by_execution = Some(101);
    left.source_revision = Some("left-revision".into());
    let mut right = generation_input(namespace.id, "producer-right", 2, Some(2));
    right.created_by_execution = Some(202);
    right.source_revision = Some("right-revision".into());
    let (left_result, right_result) = tokio::join!(
        CacheGenerationRepository::create_or_get(pool, &left),
        CacheGenerationRepository::create_or_get(pool, &right),
    );
    let (created, existing, original) = match (left_result.unwrap(), right_result.unwrap()) {
        (
            CreateCacheGenerationResult::Created(created),
            CreateCacheGenerationResult::Existing(existing),
        ) => (created, existing, left),
        (
            CreateCacheGenerationResult::Existing(existing),
            CreateCacheGenerationResult::Created(created),
        ) => (created, existing, right),
        results => panic!("reuse must converge on one producer: {results:?}"),
    };
    assert_eq!(
        serde_json::to_value(&created).unwrap(),
        serde_json::to_value(&existing).unwrap()
    );
    assert_eq!(created.created_by_execution, original.created_by_execution);
    assert_eq!(created.client_refresh_id, original.client_refresh_id);
    assert_eq!(created.source_revision, original.source_revision);
    assert_eq!(created.expected_chunk_count, original.expected_chunk_count);
    assert_eq!(created.expected_count, original.expected_count);
    assert_eq!(
        CacheGenerationRepository::list_for_namespace(pool, namespace.id, 10)
            .await
            .unwrap()
            .len(),
        1
    );

    // Even a now-exhausted aggregate budget must not reject a reuse or change
    // the existing upload contract to the waiter's different expectations.
    let admission = CacheAdmissionConfig {
        max_unpublished_generations_per_owner: 1,
        ..CacheAdmissionConfig::default()
    };
    let waiter = generation_input(namespace.id, "waiter", 99, Some(99));
    let CreateCacheGenerationResult::Existing(reused) =
        CacheGenerationRepository::create_or_get_with_policy(pool, &waiter, &admission)
            .await
            .unwrap()
    else {
        panic!("reuse must not consume admission capacity")
    };
    assert_eq!(
        serde_json::to_value(created).unwrap(),
        serde_json::to_value(reused).unwrap()
    );
}

pub(super) async fn partition_cap_allows_retry_and_releases_capacity_only_after_drop(
    pool: &PgPool,
    maintenance: &PgPool,
) {
    let namespace = CacheNamespaceRepository::create(
        pool,
        namespace_input(
            format!("partition_cap_{}", unique_test_id()),
            CacheNamespacePolicy::default(),
        ),
    )
    .await
    .unwrap();
    let admission = CacheAdmissionConfig {
        max_entry_partitions: 1,
        ..Default::default()
    };
    let input = generation_input(namespace.id, "first", 0, Some(0));
    let CreateCacheGenerationResult::Created(first) =
        CacheGenerationRepository::create_or_get_with_policy(pool, &input, &admission)
            .await
            .unwrap()
    else {
        panic!("expected a new generation")
    };
    assert!(matches!(
        CacheGenerationRepository::create_or_get_with_policy(pool, &input, &admission)
            .await
            .unwrap(),
        CreateCacheGenerationResult::Existing(_)
    ));
    let next = generation_input(namespace.id, "next", 0, Some(0));
    let error = CacheGenerationRepository::create_or_get_with_policy(pool, &next, &admission)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("partition limit"), "{error}");
    CacheGenerationRepository::fail(pool, first.id, "fixture")
        .await
        .unwrap();
    assert!(
        CacheGenerationRepository::create_or_get_with_policy(pool, &next, &admission)
            .await
            .is_err()
    );
    assert_eq!(reclaim_generation(maintenance, first.id).await.unwrap(), 0);
    let next = CacheGenerationRepository::create_or_get_with_policy(pool, &next, &admission)
        .await
        .unwrap();
    assert!(matches!(next, CreateCacheGenerationResult::Created(_)));
}
