//! Durable live-cache mutation, retry, quota and snapshot boundary contracts.

mod helpers;

use attune_common::{
    config::CacheAdmissionConfig,
    models::{cache::CacheNamespace, CacheMode},
    repositories::{
        cache::{
            ApplyLiveCacheInput, ApplyLiveCacheResult, CacheEntryInput, CacheEntryRepository,
            CacheGenerationRepository, CacheLiveRepository, CacheNamespacePolicy,
            CacheNamespaceRepository, CacheOwnerScope, CreateCacheGenerationInput,
            CreateCacheNamespaceInput,
        },
        Create, FindById,
    },
    Error,
};
use serde_json::json;
use sqlx::PgPool;

async fn namespace(pool: &PgPool, policy: CacheNamespacePolicy) -> CacheNamespace {
    CacheNamespaceRepository::create(
        pool,
        CreateCacheNamespaceInput {
            mode: CacheMode::Live,
            owner: CacheOwnerScope::system(),
            namespace: format!("live_{}", helpers::unique_test_id()),
            policy,
        },
    )
    .await
    .unwrap()
}

fn entry(id: &str, value: i64) -> CacheEntryInput {
    CacheEntryInput {
        external_id: id.into(),
        value: json!({"value": value}),
        source_updated_at: None,
        source_checksum: None,
    }
}

async fn apply(
    pool: &PgPool,
    input: ApplyLiveCacheInput<'_>,
) -> attune_common::Result<ApplyLiveCacheResult> {
    let mut tx = pool.begin().await?;
    let result =
        CacheLiveRepository::apply_conn(&mut tx, input, &CacheAdmissionConfig::default()).await?;
    tx.commit().await?;
    Ok(result)
}

fn batch<'a>(
    namespace: i64,
    request: &'a str,
    revision: i64,
    entries: &'a [CacheEntryInput],
    deleted: &'a [String],
) -> ApplyLiveCacheInput<'a> {
    ApplyLiveCacheInput {
        namespace,
        request_id: request,
        request_checksum: request,
        expected_revision: revision,
        entries,
        deleted_external_ids: deleted,
        source_revision: Some(request),
    }
}

#[tokio::test]
async fn live_replaces_and_deletes_in_one_durable_partition_with_exact_accounting() {
    let pool = helpers::create_test_pool().await.unwrap();
    let ns = namespace(&pool, Default::default()).await;
    let first = apply(
        &pool,
        batch(ns.id, "first", 0, &[entry("a", 1), entry("b", 2)], &[]),
    )
    .await
    .unwrap();
    assert_eq!((first.revision, first.record_count), (1, 2));
    let initial = CacheNamespaceRepository::find_by_id(&*pool, ns.id)
        .await
        .unwrap()
        .unwrap();
    let backing = initial.active_generation.unwrap();
    let second = apply(
        &pool,
        batch(
            ns.id,
            "second",
            1,
            &[entry("a", 10), entry("c", 3)],
            &["b".into(), "missing".into()],
        ),
    )
    .await
    .unwrap();
    assert_eq!((second.revision, second.record_count), (2, 2));
    // A separate connection sees the committed records and upstream cursor.
    let current = CacheNamespaceRepository::find_by_id(&*pool, ns.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.active_generation, Some(backing));
    assert_eq!(current.live_revision, 2);
    assert!(current.live_updated_at.unwrap() >= initial.live_updated_at.unwrap());
    assert_eq!(
        CacheEntryRepository::find_active(&pool, ns.id, "a")
            .await
            .unwrap()
            .unwrap()
            .value,
        json!({"value": 10})
    );
    assert!(CacheEntryRepository::find_active(&pool, ns.id, "b")
        .await
        .unwrap()
        .is_none());
    let totals: (i64, i64) = sqlx::query_as("SELECT COUNT(*)::BIGINT, COALESCE(SUM(size_bytes), 0)::BIGINT FROM cache_entry WHERE generation = $1")
        .bind(backing).fetch_one(&*pool).await.unwrap();
    assert_eq!(totals, (second.record_count, second.size_bytes));
    let usage: (i64, i64) = sqlx::query_as("SELECT record_count, physical_bytes FROM cache_generation_entry_usage WHERE generation = $1")
        .bind(backing).fetch_one(&*pool).await.unwrap();
    assert_eq!(totals, usage);
    let deployment_bytes: i64 = sqlx::query_scalar(
        "SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id = 1",
    )
    .fetch_one(&*pool)
    .await
    .unwrap();
    let owner_bytes: i64 = sqlx::query_scalar("SELECT physical_bytes FROM cache_owner_physical_byte_usage WHERE owner_type = 'system' AND owner = 'system'")
        .fetch_one(&*pool).await.unwrap();
    assert_eq!(
        (deployment_bytes, owner_bytes),
        (second.size_bytes, second.size_bytes)
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cache_generation WHERE namespace = $1")
            .bind(ns.id)
            .fetch_one(&*pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let generation = CacheGenerationRepository::find_by_id(&*pool, backing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(generation.source_revision.as_deref(), Some("second"));
    let deleted_ids = ["a".into(), "c".into()];
    let mut deletion = batch(ns.id, "delete", 2, &[], &deleted_ids);
    deletion.source_revision = None;
    let deleted = apply(&pool, deletion).await.unwrap();
    assert_eq!((deleted.record_count, deleted.size_bytes), (0, 0));
    assert_eq!(deleted.source_revision.as_deref(), Some("second"));
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn live_retries_replay_original_result_and_expired_receipts_cannot_reapply() {
    let pool = helpers::create_test_pool().await.unwrap();
    let ns = namespace(&pool, Default::default()).await;
    let first = apply(&pool, batch(ns.id, "one", 0, &[entry("a", 1)], &[]))
        .await
        .unwrap();
    apply(&pool, batch(ns.id, "two", 1, &[entry("a", 2)], &[]))
        .await
        .unwrap();
    let replay = apply(&pool, batch(ns.id, "one", 0, &[entry("a", 1)], &[]))
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.revision, first.revision);
    let mut changed = batch(ns.id, "one", 0, &[], &[]);
    changed.request_checksum = "different";
    assert!(apply(&pool, changed).await.is_err());
    sqlx::query("UPDATE cache_live_request SET created = clock_timestamp() - interval '25 hours' WHERE namespace = $1")
        .bind(ns.id).execute(&*pool).await.unwrap();
    assert!(apply(&pool, batch(ns.id, "one", 0, &[entry("a", 1)], &[]))
        .await
        .is_err());
    assert_eq!(
        CacheEntryRepository::find_active(&pool, ns.id, "a")
            .await
            .unwrap()
            .unwrap()
            .value,
        json!({"value": 2})
    );
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn live_overquota_and_invalid_batches_roll_back_records_cursor_and_revision() {
    let pool = helpers::create_test_pool().await.unwrap();
    let ns = namespace(
        &pool,
        CacheNamespacePolicy {
            max_records_per_generation: 1,
            ..Default::default()
        },
    )
    .await;
    let initial = apply(&pool, batch(ns.id, "initial", 0, &[entry("a", 1)], &[]))
        .await
        .unwrap();
    assert!(apply(
        &pool,
        batch(ns.id, "too-many", 1, &[entry("a", 8), entry("b", 2)], &[])
    )
    .await
    .is_err());
    assert!(matches!(
        apply(
            &pool,
            batch(ns.id, "overlap", 1, &[entry("a", 8)], &["a".into()])
        )
        .await,
        Err(Error::CacheDuplicateExternalId)
    ));
    assert!(matches!(
        apply(
            &pool,
            batch(ns.id, "duplicate", 1, &[entry("a", 8), entry("a", 9)], &[])
        )
        .await,
        Err(Error::CacheDuplicateExternalId)
    ));
    let current = CacheNamespaceRepository::find_by_id(&*pool, ns.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.live_revision, initial.revision);
    assert_eq!(
        CacheEntryRepository::find_active(&pool, ns.id, "a")
            .await
            .unwrap()
            .unwrap()
            .value,
        json!({"value": 1})
    );
    let generation =
        CacheGenerationRepository::find_by_id(&*pool, current.active_generation.unwrap())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(generation.source_revision.as_deref(), Some("initial"));
    assert_eq!(generation.size_bytes, initial.size_bytes);
    // Delete-before-insert permits replacement at the record quota.
    assert_eq!(
        apply(
            &pool,
            batch(ns.id, "replace", 1, &[entry("b", 2)], &["a".into()])
        )
        .await
        .unwrap()
        .record_count,
        1
    );
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn live_rejects_snapshot_lifecycle_scans_and_mode_conversion() {
    let pool = helpers::create_test_pool().await.unwrap();
    let ns = namespace(&pool, Default::default()).await;
    let input = CreateCacheGenerationInput {
        namespace: ns.id,
        client_refresh_id: "snapshot".into(),
        expected_active_generation: None,
        expected_chunk_count: 0,
        expected_count: None,
        expected_bytes: None,
        checksum_algorithm: None,
        checksum: None,
        source_revision: None,
        created_by: None,
        created_by_execution: None,
    };
    assert!(matches!(
        CacheGenerationRepository::create_or_get(&pool, &input).await,
        Err(Error::CacheModeMismatch(_))
    ));
    apply(&pool, batch(ns.id, "one", 0, &[entry("a", 1)], &[]))
        .await
        .unwrap();
    let current = CacheNamespaceRepository::find_by_id(&*pool, ns.id)
        .await
        .unwrap()
        .unwrap();
    let backing = current.active_generation.unwrap();
    assert!(matches!(
        CacheEntryRepository::find_pinned(&pool, ns.id, backing, "a").await,
        Err(Error::CacheModeMismatch(_))
    ));
    assert!(matches!(
        CacheEntryRepository::find_pinned_many(&pool, ns.id, backing, &["a".into()]).await,
        Err(Error::CacheModeMismatch(_))
    ));
    assert!(matches!(
        CacheEntryRepository::scan_pinned_page(&pool, ns.id, backing, None, 10).await,
        Err(Error::CacheModeMismatch(_))
    ));
    assert!(
        sqlx::query("UPDATE cache_namespace SET mode = 'snapshot' WHERE id = $1")
            .bind(ns.id)
            .execute(&*pool)
            .await
            .is_err()
    );
    let snapshot = CacheNamespaceRepository::create(
        &*pool,
        CreateCacheNamespaceInput {
            mode: CacheMode::Snapshot,
            owner: CacheOwnerScope::system(),
            namespace: "snapshot".into(),
            policy: Default::default(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        apply(&pool, batch(snapshot.id, "wrong-mode", 0, &[], &[])).await,
        Err(Error::CacheModeMismatch(_))
    ));
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn live_concurrent_writers_with_same_revision_have_one_winner() {
    let pool = helpers::create_test_pool().await.unwrap();
    let ns = namespace(&pool, Default::default()).await;
    let a = [entry("a", 1)];
    let b = [entry("b", 2)];
    let (one, two) = tokio::join!(
        apply(&pool, batch(ns.id, "one", 0, &a, &[])),
        apply(&pool, batch(ns.id, "two", 0, &b, &[]))
    );
    assert_ne!(one.is_ok(), two.is_ok());
    let current = CacheNamespaceRepository::find_by_id(&*pool, ns.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.live_revision, 1);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cache_generation WHERE namespace = $1")
            .bind(ns.id)
            .fetch_one(&*pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn live_receipt_eviction_stays_bounded_and_rejects_stale_requests() {
    let pool = helpers::create_test_pool().await.unwrap();
    let ns = namespace(&pool, Default::default()).await;
    apply(&pool, batch(ns.id, "initial", 0, &[], &[]))
        .await
        .unwrap();
    // Seed only this fixture's receipt metadata, avoiding 10000 real mutations.
    sqlx::query("INSERT INTO cache_live_request(namespace, request_id, request_checksum, revision, record_count, size_bytes)
        SELECT $1, 'request-' || revision, 'checksum-' || revision, revision, 0, 0
        FROM generate_series(2, 10000) revision")
        .bind(ns.id).execute(&*pool).await.unwrap();
    sqlx::query("UPDATE cache_namespace SET live_revision = 10000 WHERE id = $1")
        .bind(ns.id)
        .execute(&*pool)
        .await
        .unwrap();
    assert_eq!(
        apply(&pool, batch(ns.id, "newest", 10000, &[], &[]))
            .await
            .unwrap()
            .revision,
        10001
    );
    let retained: (i64, i64) = sqlx::query_as("SELECT COUNT(*)::BIGINT, MIN(revision)::BIGINT FROM cache_live_request WHERE namespace = $1")
        .bind(ns.id).fetch_one(&*pool).await.unwrap();
    assert_eq!(retained, (10000, 2));
    assert!(apply(&pool, batch(ns.id, "initial", 0, &[], &[]))
        .await
        .is_err());
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn managed_live_namespace_reload_preserves_contents_and_rejects_mode_changes() {
    use attune_common::repositories::cache::ManagedCacheNamespaceDefinition;
    let pool = helpers::create_test_pool().await.unwrap();
    let pack = helpers::PackFixture::new_unique("live_managed")
        .create(&pool)
        .await
        .unwrap();
    let mut definition = ManagedCacheNamespaceDefinition {
        mode: CacheMode::Live,
        definition_ref: format!("{}.users", pack.r#ref),
        owner: CacheOwnerScope::pack(pack.id, Some(pack.r#ref.clone())),
        namespace: "users".into(),
        policy: Default::default(),
    };
    let admission = CacheAdmissionConfig::default();
    CacheNamespaceRepository::upsert_managed_definitions(
        &pool,
        pack.id,
        &pack.r#ref,
        std::slice::from_ref(&definition),
        &admission,
    )
    .await
    .unwrap();
    let ns = CacheNamespaceRepository::resolve_managed_definition(
        &*pool,
        &pack.r#ref,
        &definition.definition_ref,
    )
    .await
    .unwrap()
    .unwrap();
    apply(&pool, batch(ns.id, "one", 0, &[entry("a", 1)], &[]))
        .await
        .unwrap();
    definition.policy.freshness_target_seconds = 120;
    CacheNamespaceRepository::upsert_managed_definitions(
        &pool,
        pack.id,
        &pack.r#ref,
        std::slice::from_ref(&definition),
        &admission,
    )
    .await
    .unwrap();
    definition.mode = CacheMode::Snapshot;
    assert!(CacheNamespaceRepository::upsert_managed_definitions(
        &pool,
        pack.id,
        &pack.r#ref,
        &[definition],
        &admission
    )
    .await
    .is_err());
    let current = CacheNamespaceRepository::find_by_id(&*pool, ns.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.mode, CacheMode::Live);
    assert_eq!(current.live_revision, 1);
    assert!(CacheEntryRepository::find_active(&pool, ns.id, "a")
        .await
        .unwrap()
        .is_some());
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn live_initial_quota_failure_leaves_no_generation_partition_or_retry_receipt() {
    let pool = helpers::create_test_pool().await.unwrap();
    let ns = namespace(&pool, Default::default()).await;
    let changes = [entry("a", 1)];
    let mut tx = pool.begin().await.unwrap();
    let error = CacheLiveRepository::apply_conn(
        &mut tx,
        batch(ns.id, "quota", 0, &changes, &[]),
        &CacheAdmissionConfig {
            max_physical_bytes: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, Error::CacheQuotaExceeded { .. }));
    tx.rollback().await.unwrap();
    let current = CacheNamespaceRepository::find_by_id(&*pool, ns.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.live_revision, 0);
    assert!(current.active_generation.is_none());
    for query in [
        "SELECT COUNT(*) FROM cache_generation WHERE namespace = $1",
        "SELECT COUNT(*) FROM cache_live_request WHERE namespace = $1",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(query)
                .bind(ns.id)
                .fetch_one(&*pool)
                .await
                .unwrap(),
            0
        );
    }
    let partitions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_inherits WHERE inhparent = 'cache_entry'::regclass",
    )
    .fetch_one(&*pool)
    .await
    .unwrap();
    assert_eq!(partitions, 0);
    assert_eq!(
        apply(&pool, batch(ns.id, "quota", 0, &changes, &[]))
            .await
            .unwrap()
            .revision,
        1
    );
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn live_tombstone_blocks_access_then_partition_cleanup_releases_usage_and_receipts() {
    use attune_common::{
        config::CacheRetentionConfig, repositories::cache::CacheGenerationCleanupOutcome,
    };
    let pool = helpers::create_test_pool().await.unwrap();
    let ns = namespace(&pool, Default::default()).await;
    apply(&pool, batch(ns.id, "initial", 0, &[entry("a", 1)], &[]))
        .await
        .unwrap();
    let backing = CacheNamespaceRepository::find_by_id(&*pool, ns.id)
        .await
        .unwrap()
        .unwrap()
        .active_generation
        .unwrap();
    CacheNamespaceRepository::tombstone(&pool, ns.id)
        .await
        .unwrap();
    assert!(apply(&pool, batch(ns.id, "late", 1, &[], &[]))
        .await
        .is_err());
    assert!(CacheEntryRepository::find_active(&pool, ns.id, "a")
        .await
        .unwrap()
        .is_none());
    // Move only this test-owned tombstoned generation past its traversal window.
    sqlx::query("UPDATE cache_generation SET retired = NOW() - interval '2 days', readable_until = NOW() - interval '2 days' WHERE id = $1")
        .bind(backing).execute(&*pool).await.unwrap();
    assert!(matches!(
        CacheGenerationRepository::drop_if_cleanup_eligible(
            &pool,
            backing,
            &CacheRetentionConfig::default()
        )
        .await
        .unwrap(),
        CacheGenerationCleanupOutcome::Dropped { .. }
    ));
    assert_eq!(
        CacheNamespaceRepository::delete_empty_tombstoned_batch(&pool, 10)
            .await
            .unwrap(),
        1
    );
    let usage: i64 = sqlx::query_scalar(
        "SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id = 1",
    )
    .fetch_one(&*pool)
    .await
    .unwrap();
    let receipts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cache_live_request WHERE namespace = $1")
            .bind(ns.id)
            .fetch_one(&*pool)
            .await
            .unwrap();
    assert_eq!((usage, receipts), (0, 0));
    pool.cleanup().await.unwrap();
}
