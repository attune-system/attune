//! Shared existing live-cache assertions, run through actual API database logins.
use super::helpers;
use attune_common::{
    config::CacheAdmissionConfig,
    models::{cache::CacheNamespace, CacheMode},
    repositories::{
        cache::{
            ApplyLiveCacheInput, ApplyLiveCacheResult, CacheEntryInput, CacheEntryRepository,
            CacheGenerationRepository, CacheLiveRepository, CacheNamespacePolicy,
            CacheNamespaceRepository, CacheOwnerScope, CreateCacheNamespaceInput,
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

pub(super) async fn live_replaces_and_deletes_in_one_durable_partition_with_exact_accounting(
    pool: &PgPool,
) {
    let ns = namespace(pool, Default::default()).await;
    let first = apply(
        pool,
        batch(ns.id, "first", 0, &[entry("a", 1), entry("b", 2)], &[]),
    )
    .await
    .unwrap();
    assert_eq!((first.revision, first.record_count), (1, 2));
    let initial = CacheNamespaceRepository::find_by_id(pool, ns.id)
        .await
        .unwrap()
        .unwrap();
    let backing = initial.active_generation.unwrap();
    let second = apply(
        pool,
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
    let current = CacheNamespaceRepository::find_by_id(pool, ns.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.active_generation, Some(backing));
    assert_eq!(current.live_revision, 2);
    assert!(current.live_updated_at.unwrap() >= initial.live_updated_at.unwrap());
    assert_eq!(
        CacheEntryRepository::find_active(pool, ns.id, "a")
            .await
            .unwrap()
            .unwrap()
            .value,
        json!({"value": 10})
    );
    assert!(CacheEntryRepository::find_active(pool, ns.id, "b")
        .await
        .unwrap()
        .is_none());
    let totals: (i64, i64) = sqlx::query_as("SELECT COUNT(*)::BIGINT, COALESCE(SUM(size_bytes), 0)::BIGINT FROM cache_entry WHERE generation = $1")
        .bind(backing).fetch_one(pool).await.unwrap();
    assert_eq!(totals, (second.record_count, second.size_bytes));
    let usage: (i64, i64) = sqlx::query_as("SELECT record_count, physical_bytes FROM cache_generation_entry_usage WHERE generation = $1")
        .bind(backing).fetch_one(pool).await.unwrap();
    assert_eq!(totals, usage);
    let deployment_bytes: i64 = sqlx::query_scalar(
        "SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id = 1",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let owner_bytes: i64 = sqlx::query_scalar("SELECT physical_bytes FROM cache_owner_physical_byte_usage WHERE owner_type = 'system' AND owner = 'system'")
        .fetch_one(pool).await.unwrap();
    assert_eq!(
        (deployment_bytes, owner_bytes),
        (second.size_bytes, second.size_bytes)
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cache_generation WHERE namespace = $1")
            .bind(ns.id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let generation = CacheGenerationRepository::find_by_id(pool, backing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(generation.source_revision.as_deref(), Some("second"));
    let deleted_ids = ["a".into(), "c".into()];
    let mut deletion = batch(ns.id, "delete", 2, &[], &deleted_ids);
    deletion.source_revision = None;
    let deleted = apply(pool, deletion).await.unwrap();
    assert_eq!((deleted.record_count, deleted.size_bytes), (0, 0));
    assert_eq!(deleted.source_revision.as_deref(), Some("second"));
}

pub(super) async fn live_initial_quota_failure_leaves_no_generation_partition_or_retry_receipt(
    pool: &PgPool,
) {
    let ns = namespace(pool, Default::default()).await;
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
    let current = CacheNamespaceRepository::find_by_id(pool, ns.id)
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
                .fetch_one(pool)
                .await
                .unwrap(),
            0
        );
    }
    let partitions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_inherits WHERE inhparent = 'cache_entry'::regclass",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(partitions, 0);
    assert_eq!(
        apply(pool, batch(ns.id, "quota", 0, &changes, &[]))
            .await
            .unwrap()
            .revision,
        1
    );
}
