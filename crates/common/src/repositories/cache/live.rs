//! Atomic live-cache mutations. Storage IDs stay internal to this interface.

use super::{
    deduplicate_lookup_ids, ensure_generation_admission, ensure_generation_quota,
    ensure_namespace_storage_quota, ensure_namespace_writable, ensure_physical_byte_admission,
    generation_entry_usage, lock_generation, lock_namespace, replace_entries_conn,
    validate_generation_storage, validate_mutation_input, validate_optional_text,
    validate_required_text, CacheEntryInput, CacheEntryRepository, CacheNamespaceRepository,
    CacheTransactionMode, MAX_CACHE_TEXT_BYTES,
};
use crate::{
    config::CacheAdmissionConfig,
    models::{cache::CacheEntry, CacheGenerationState, CacheMode, Id},
    repositories::FindById,
    Error, Result,
};

const MAX_LIVE_REQUEST_RECEIPTS: i64 = 10_000;

pub struct ApplyLiveCacheInput<'a> {
    pub namespace: Id,
    pub request_id: &'a str,
    pub request_checksum: &'a str,
    pub expected_revision: i64,
    pub entries: &'a [CacheEntryInput],
    pub deleted_external_ids: &'a [String],
    /// Omission retains the last upstream cursor.
    pub source_revision: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct ApplyLiveCacheResult {
    pub revision: i64,
    pub record_count: i64,
    pub size_bytes: i64,
    pub source_revision: Option<String>,
    pub replayed: bool,
}

pub struct CacheLiveRepository;

impl CacheLiveRepository {
    /// Share-locks the current collection for the transaction. No backing ID or
    /// historical revision selector is accepted by this read interface.
    pub async fn lookup_many_conn(
        conn: &mut sqlx::PgConnection,
        namespace_id: Id,
        external_ids: &[String],
    ) -> Result<Vec<CacheEntry>> {
        let ids = deduplicate_lookup_ids(external_ids)?;
        let namespace =
            CacheNamespaceRepository::find_by_id_for_share_conn(conn, namespace_id).await?;
        ensure_namespace_writable(&namespace)?;
        if namespace.mode != CacheMode::Live {
            return Err(Error::CacheModeMismatch(
                "live lookup requires a live cache namespace".into(),
            ));
        }
        let generation = namespace
            .active_generation
            .ok_or_else(|| Error::invalid_state("live cache is not populated"))?;
        CacheEntryRepository::find_generation_many_conn(conn, generation, &ids).await
    }

    /// Caller owns the transaction. Revision, records, quota accounting, source
    /// cursor and retry receipt commit together, or all roll back.
    pub async fn apply_conn(
        conn: &mut sqlx::PgConnection,
        input: ApplyLiveCacheInput<'_>,
        admission: &CacheAdmissionConfig,
    ) -> Result<ApplyLiveCacheResult> {
        validate_required_text(
            input.request_id,
            MAX_CACHE_TEXT_BYTES,
            "live cache request ID",
        )?;
        validate_optional_text(
            input.source_revision,
            MAX_CACHE_TEXT_BYTES,
            "cache source revision",
        )?;
        if input.expected_revision < 0 {
            return Err(Error::validation("expected_revision must be nonnegative"));
        }
        let ids = validate_mutation_input(
            0,
            input.request_checksum,
            input.entries,
            input.deleted_external_ids,
        )?;
        CacheEntryRepository::protect_transaction(conn, CacheTransactionMode::Write).await?;
        let observed = CacheNamespaceRepository::find_by_id(&mut *conn, input.namespace)
            .await?
            .ok_or_else(|| {
                Error::not_found("cache_namespace", "id", input.namespace.to_string())
            })?;
        // Admission prevents mutation/reclamation while parent protection is
        // upgraded, before namespace or generation row locks are acquired.
        let mode = if observed.active_generation.is_none() {
            CacheTransactionMode::Attach
        } else {
            CacheTransactionMode::Ingest
        };
        CacheEntryRepository::protect_transaction(conn, mode).await?;
        let namespace = lock_namespace(conn, input.namespace)
            .await?
            .ok_or_else(|| {
                Error::not_found("cache_namespace", "id", input.namespace.to_string())
            })?;
        ensure_namespace_writable(&namespace)?;
        if namespace.mode != CacheMode::Live {
            return Err(Error::CacheModeMismatch(
                "apply requires a live cache namespace".into(),
            ));
        }
        let receipt: Option<(String, i64, i64, i64, Option<String>)> = sqlx::query_as(
            "SELECT request_checksum, revision, record_count, size_bytes, source_revision
             FROM cache_live_request WHERE namespace = $1 AND request_id = $2
               AND created > clock_timestamp() - interval '24 hours'",
        )
        .bind(namespace.id)
        .bind(input.request_id)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some((checksum, revision, record_count, size_bytes, source_revision)) = receipt {
            if checksum != input.request_checksum {
                return Err(Error::invalid_state(
                    "live cache request ID already accepted with different content",
                ));
            }
            return Ok(ApplyLiveCacheResult {
                revision,
                record_count,
                size_bytes,
                source_revision,
                replayed: true,
            });
        }
        if input.expected_revision != namespace.live_revision {
            return Err(Error::invalid_state(
                "live cache revision changed before apply",
            ));
        }
        let generation_id = match namespace.active_generation {
            Some(id) => {
                validate_generation_storage(conn, id).await?;
                let generation = lock_generation(conn, id)
                    .await?
                    .ok_or_else(|| Error::invalid_state("live backing generation is missing"))?;
                if generation.namespace != namespace.id
                    || generation.state != CacheGenerationState::Active
                {
                    return Err(Error::invalid_state(
                        "live backing generation is not active",
                    ));
                }
                id
            }
            None => {
                ensure_generation_admission(conn, &namespace, None, admission).await?;
                let partitions: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM pg_inherits WHERE inhparent = 'cache_entry'::regclass",
                )
                .fetch_one(&mut *conn)
                .await?;
                if partitions >= admission.max_entry_partitions {
                    return Err(Error::cache_quota_exceeded(
                        "cache_entry_partition_limit_exceeded",
                        "deployment cache entry partition limit exceeded",
                    ));
                }
                let id: Id = sqlx::query_scalar(
                    "INSERT INTO cache_generation(namespace, client_refresh_id, expected_chunk_count, state, activated)
                     VALUES ($1, 'live', 0, 'active', clock_timestamp()) RETURNING id",
                ).bind(namespace.id).fetch_one(&mut *conn).await?;
                sqlx::query("SELECT create_cache_generation_partition($1)")
                    .bind(id)
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("UPDATE cache_namespace SET active_generation = $2 WHERE id = $1")
                    .bind(namespace.id)
                    .bind(id)
                    .execute(&mut *conn)
                    .await?;
                id
            }
        };
        replace_entries_conn(conn, generation_id, input.entries, &ids).await?;
        let (record_count, size_bytes) = generation_entry_usage(conn, generation_id).await?;
        ensure_generation_quota(&namespace, record_count, size_bytes)?;
        ensure_namespace_storage_quota(&namespace, size_bytes)?;
        ensure_physical_byte_admission(conn, &namespace, admission).await?;
        let source_revision: Option<String> = sqlx::query_scalar(
            "UPDATE cache_generation SET record_count = $2, size_bytes = $3,
                 source_revision = COALESCE($4, source_revision) WHERE id = $1 RETURNING source_revision",
        ).bind(generation_id).bind(record_count).bind(size_bytes).bind(input.source_revision)
            .fetch_one(&mut *conn).await?;
        let revision: i64 = sqlx::query_scalar(
            "UPDATE cache_namespace SET live_revision = live_revision + 1, live_updated_at = clock_timestamp()
             WHERE id = $1 RETURNING live_revision",
        ).bind(namespace.id).fetch_one(&mut *conn).await?;
        sqlx::query("DELETE FROM cache_live_request WHERE namespace = $1 AND created <= clock_timestamp() - interval '24 hours'")
            .bind(namespace.id).execute(&mut *conn).await?;
        // Keep the newest receipts. Required expected_revision rejects stale
        // retries even after their receipt has expired or been evicted.
        sqlx::query(
            "DELETE FROM cache_live_request WHERE namespace = $1 AND request_id IN (
                 SELECT request_id FROM cache_live_request WHERE namespace = $1
                 ORDER BY revision DESC OFFSET $2)",
        )
        .bind(namespace.id)
        .bind(MAX_LIVE_REQUEST_RECEIPTS - 1)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO cache_live_request(namespace, request_id, request_checksum, revision, record_count, size_bytes, source_revision)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        ).bind(namespace.id).bind(input.request_id).bind(input.request_checksum).bind(revision)
            .bind(record_count).bind(size_bytes).bind(&source_revision).execute(&mut *conn).await?;
        Ok(ApplyLiveCacheResult {
            revision,
            record_count,
            size_bytes,
            source_revision,
            replayed: false,
        })
    }
}
