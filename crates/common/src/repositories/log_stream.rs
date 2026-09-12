use crate::models::enums::LogStreamBackend;
use crate::models::log_stream::{LogSegment, LogStream, SEGMENT_COLUMNS, STREAM_COLUMNS};
use crate::{Error, Result};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};

pub struct LogStreamRepository;

impl LogStreamRepository {
    pub async fn create(
        pool: &PgPool,
        artifact_version: i64,
        max_unflushed_bytes: u64,
        max_unflushed_milliseconds: u64,
    ) -> Result<LogStream> {
        Self::create_with_backend(
            pool,
            artifact_version,
            LogStreamBackend::ObjectSegments,
            max_unflushed_bytes,
            max_unflushed_milliseconds,
        )
        .await
    }

    pub async fn create_with_backend(
        pool: &PgPool,
        artifact_version: i64,
        backend: LogStreamBackend,
        max_unflushed_bytes: u64,
        max_unflushed_milliseconds: u64,
    ) -> Result<LogStream> {
        if max_unflushed_bytes == 0 || max_unflushed_milliseconds == 0 {
            return Err(Error::validation(
                "log flush limits must be greater than zero",
            ));
        }
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(-$1)")
            .bind(artifact_version)
            .execute(&mut *tx)
            .await?;
        let query = format!(
            "INSERT INTO log_stream (artifact_version, backend, max_unflushed_bytes, max_unflushed_milliseconds) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (artifact_version) DO UPDATE \
             SET artifact_version = EXCLUDED.artifact_version RETURNING {STREAM_COLUMNS}"
        );
        let stream = sqlx::query_as(&query)
            .bind(artifact_version)
            .bind(backend)
            .bind(
                i64::try_from(max_unflushed_bytes)
                    .map_err(|_| Error::validation("log byte limit is too large"))?,
            )
            .bind(
                i64::try_from(max_unflushed_milliseconds)
                    .map_err(|_| Error::validation("log time limit is too large"))?,
            )
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(stream)
    }

    pub async fn find_by_artifact_version(
        pool: &PgPool,
        version_id: i64,
    ) -> Result<Option<LogStream>> {
        let query = format!("SELECT {STREAM_COLUMNS} FROM log_stream WHERE artifact_version = $1");
        sqlx::query_as(&query)
            .bind(version_id)
            .fetch_optional(pool)
            .await
            .map_err(Into::into)
    }

    pub async fn find_by_id(connection: &mut PgConnection, stream_id: i64) -> Result<LogStream> {
        let query = format!("SELECT {STREAM_COLUMNS} FROM log_stream WHERE id = $1");
        sqlx::query_as(&query)
            .bind(stream_id)
            .fetch_optional(connection)
            .await?
            .ok_or_else(|| Error::not_found("log_stream", "id", stream_id.to_string()))
    }

    pub async fn find_by_id_in_pool(pool: &PgPool, stream_id: i64) -> Result<LogStream> {
        let query = format!("SELECT {STREAM_COLUMNS} FROM log_stream WHERE id = $1");
        sqlx::query_as(&query)
            .bind(stream_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| Error::not_found("log_stream", "id", stream_id.to_string()))
    }

    pub async fn acquire_writer_lock(connection: &mut PgConnection, stream_id: i64) -> Result<()> {
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(stream_id)
            .execute(connection)
            .await?;
        Ok(())
    }

    pub async fn release_writer_lock(connection: &mut PgConnection, stream_id: i64) -> Result<()> {
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(stream_id)
            .execute(connection)
            .await?;
        Ok(())
    }

    pub async fn segments(pool: &PgPool, stream_id: i64) -> Result<Vec<LogSegment>> {
        let query = format!(
            "SELECT {SEGMENT_COLUMNS} FROM log_segment WHERE stream = $1 ORDER BY sequence"
        );
        sqlx::query_as(&query)
            .bind(stream_id)
            .fetch_all(pool)
            .await
            .map_err(Into::into)
    }

    pub async fn find_segment_by_sequence(
        pool: &PgPool,
        stream_id: i64,
        sequence: i64,
    ) -> Result<Option<LogSegment>> {
        let query = format!(
            "SELECT {SEGMENT_COLUMNS} FROM log_segment WHERE stream = $1 AND sequence = $2"
        );
        sqlx::query_as(&query)
            .bind(stream_id)
            .bind(sequence)
            .fetch_optional(pool)
            .await
            .map_err(Into::into)
    }

    pub async fn lock<'a>(tx: &mut Transaction<'a, Postgres>, stream_id: i64) -> Result<LogStream> {
        sqlx::query(
            "SELECT pg_advisory_xact_lock(-artifact_version) FROM log_stream WHERE id = $1",
        )
        .bind(stream_id)
        .execute(&mut **tx)
        .await?;
        let query = format!("SELECT {STREAM_COLUMNS} FROM log_stream WHERE id = $1 FOR UPDATE");
        sqlx::query_as(&query)
            .bind(stream_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| Error::not_found("log_stream", "id", stream_id.to_string()))
    }

    pub async fn find_segment<'a>(
        tx: &mut Transaction<'a, Postgres>,
        stream_id: i64,
        sequence: i64,
    ) -> Result<Option<LogSegment>> {
        let query = format!(
            "SELECT {SEGMENT_COLUMNS} FROM log_segment WHERE stream = $1 AND sequence = $2"
        );
        sqlx::query_as(&query)
            .bind(stream_id)
            .bind(sequence)
            .fetch_optional(&mut **tx)
            .await
            .map_err(Into::into)
    }

    pub async fn commit_segment<'a>(
        tx: &mut Transaction<'a, Postgres>,
        stream: &LogStream,
        sequence: i64,
        size_bytes: i64,
        sha256: &str,
        object_key: &str,
        provider_version: &str,
    ) -> Result<LogSegment> {
        if stream.sealed {
            return Err(Error::invalid_state("log stream is sealed"));
        }
        let body_is_pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM artifact_version WHERE id = $1 AND body_state = 'pending')",
        )
        .bind(stream.artifact_version)
        .fetch_one(&mut **tx)
        .await?;
        if !body_is_pending {
            return Err(Error::invalid_state("log artifact is not pending"));
        }
        if sequence != stream.next_sequence {
            return Err(Error::invalid_state(format!(
                "log segment sequence {sequence} is out of order; expected {}",
                stream.next_sequence
            )));
        }
        let query = format!(
            "INSERT INTO log_segment (stream, sequence, byte_start, byte_end, size_bytes, sha256, object_key, provider_version) \
             VALUES ($1, $2, $3, $3 + $4, $4, $5, $6, $7) RETURNING {SEGMENT_COLUMNS}"
        );
        let segment = sqlx::query_as(&query)
            .bind(stream.id)
            .bind(sequence)
            .bind(stream.total_bytes)
            .bind(size_bytes)
            .bind(sha256)
            .bind(object_key)
            .bind(provider_version)
            .fetch_one(&mut **tx)
            .await?;
        sqlx::query("UPDATE log_stream SET next_sequence = next_sequence + 1, total_bytes = total_bytes + $2 WHERE id = $1")
            .bind(stream.id).bind(size_bytes).execute(&mut **tx).await?;
        Ok(segment)
    }

    pub async fn seal<'a>(
        tx: &mut Transaction<'a, Postgres>,
        stream_id: i64,
        truncated: bool,
    ) -> Result<()> {
        sqlx::query("UPDATE log_stream SET sealed = TRUE, truncated = truncated OR $2, sealed_at = COALESCE(sealed_at, NOW()) WHERE id = $1")
            .bind(stream_id).bind(truncated).execute(&mut **tx).await?;
        Ok(())
    }

    pub async fn seal_shared_file<'a>(
        tx: &mut Transaction<'a, Postgres>,
        stream_id: i64,
        total_bytes: i64,
        truncated: bool,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE log_stream SET total_bytes = $2, sealed = TRUE, \
             truncated = truncated OR $3, sealed_at = COALESCE(sealed_at, NOW()) WHERE id = $1",
        )
        .bind(stream_id)
        .bind(total_bytes)
        .bind(truncated)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}
