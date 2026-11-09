use crate::models::enums::LogStreamBackend;
use crate::models::log_stream::{
    LogDeliveryState, LogSegment, LogStream, SEGMENT_COLUMNS, STREAM_COLUMNS,
};
use crate::{Error, Result};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};

/// Private projection: lifecycle fields never enter the public LogStream model.
#[derive(Debug, sqlx::FromRow)]
pub struct SelectedLogClose {
    revision: i64,
    phase: String,
    input: serde_json::Value,
    input_sha256: String,
}

impl SelectedLogClose {
    pub fn published(&self) -> bool {
        self.phase == "published"
    }
    pub fn revision(&self) -> i64 {
        self.revision
    }
    pub fn input(&self) -> &serde_json::Value {
        &self.input
    }
    pub fn input_sha256(&self) -> &str {
        &self.input_sha256
    }
}

pub struct LogStreamRepository;

impl LogStreamRepository {
    /// Transaction-local limits; the route owns a detached socket so a timed-out
    /// phase/rollback disposes the session instead of returning uncertain state.
    pub async fn configure_close_phase(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
        sqlx::query("SELECT set_config('statement_timeout','5s',true),set_config('lock_timeout','5s',true),set_config('jit','off',true)")
            .execute(&mut **tx).await?;
        Ok(())
    }

    pub async fn recover_expired_closes_bounded(
        pool: &PgPool,
        limit: i64,
        budget: &super::maintenance_admission::MaintenanceBudget,
        deadline: tokio::time::Instant,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> super::maintenance_admission::MaintenanceResult<()> {
        super::maintenance_admission::run(pool, budget, deadline, cancel,
            move |conn| Box::pin(async move {
                Ok(sqlx::query_as::<_, (i64,i64,chrono::DateTime<chrono::Utc>)>(
                    "SELECT id,close_revision,close_deadline FROM log_stream WHERE close_phase='closing' AND close_deadline<=clock_timestamp() ORDER BY close_deadline,id LIMIT $1")
                    .bind(limit.clamp(1,256)).fetch_all(conn).await?)
            }),
            |conn, candidates| Box::pin(async move {
                let mut changed=0;
                for (id, revision, deadline) in candidates {
                    changed += sqlx::query("UPDATE log_stream SET close_phase='abandoned',close_revision=close_revision+1 WHERE id=$1 AND close_phase='closing' AND close_revision=$2 AND close_deadline=$3 AND close_deadline<=clock_timestamp()")
                        .bind(id).bind(revision).bind(deadline).execute(&mut *conn).await?.rows_affected();
                }
                Ok(((),changed))
            })).await
    }

    /// Caller owns graph/source/artifact/version/stream locking and fresh producer
    /// authorization. Binding values are authenticated server projections, never
    /// client claims. This selects bytes only; it grants no publication authority.
    /// `expected_revision` fences abandonment, expiry takeover and lost replies.
    pub async fn select_close(
        tx: &mut Transaction<'_, Postgres>,
        stream: i64,
        expected_revision: i64,
        producer: &serde_json::Value,
        proof: &serde_json::Value,
        truncated: bool,
        deadline: chrono::DateTime<chrono::Utc>,
    ) -> Result<SelectedLogClose> {
        let row = sqlx::query_as(
            "WITH input AS (SELECT jsonb_build_object('snapshot',object_log_close_snapshot($1), \
                'producer',$3::jsonb,'proof',$4::jsonb,'truncated',$5::boolean) value) \
             UPDATE log_stream s SET close_phase='closing',close_revision=close_revision+1, \
               close_input=input.value, \
               close_input_sha256=encode(sha256(convert_to(input.value::text,'UTF8')),'hex'), \
               close_deadline=$6 FROM input WHERE s.id=$1 AND s.close_revision=$2 \
               AND (s.close_phase IS NULL OR s.close_phase='abandoned' \
                    OR (s.close_phase='closing' AND s.close_deadline<=clock_timestamp())) \
             RETURNING close_revision AS revision,close_phase AS phase,close_input AS input,close_input_sha256 AS input_sha256",
        ).bind(stream).bind(expected_revision).bind(producer).bind(proof).bind(truncated).bind(deadline)
        .fetch_optional(&mut **tx).await?;
        let row = match row {
            Some(row) => Some(row),
            None => sqlx::query_as(
                "SELECT close_revision AS revision,close_phase AS phase,close_input AS input,close_input_sha256 AS input_sha256 \
                 FROM log_stream WHERE id=$1 AND close_revision IN ($2,$2+1) AND close_phase IN ('closing','published') \
                 AND (close_phase='published' OR close_deadline>clock_timestamp()) AND close_input=jsonb_build_object( \
                   'snapshot',object_log_close_snapshot($1),'producer',$3::jsonb, \
                   'proof',$4::jsonb,'truncated',$5::boolean)",
            ).bind(stream).bind(expected_revision).bind(producer).bind(proof).bind(truncated)
            .fetch_optional(&mut **tx).await?,
        };
        row.ok_or_else(|| Error::invalid_state("close revision changed or input conflicts"))
    }

    pub async fn close_revision(tx: &mut Transaction<'_, Postgres>, stream: i64) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT close_revision FROM log_stream WHERE id=$1")
                .bind(stream)
                .fetch_one(&mut **tx)
                .await?,
        )
    }

    /// Called only with privately verified bytes by the API's selected-close owner.
    pub async fn publish_close(
        tx: &mut Transaction<'_, Postgres>,
        stream: i64,
        selected: &SelectedLogClose,
    ) -> Result<()> {
        let changed = sqlx::query("UPDATE log_stream SET close_phase='published',sealed=true,sealed_at=clock_timestamp(),truncated=(close_input->>'truncated')::boolean WHERE id=$1 AND close_revision=$2 AND close_input=$3 AND close_input_sha256=$4 AND close_phase='closing' AND close_deadline>clock_timestamp()")
            .bind(stream).bind(selected.revision()).bind(selected.input()).bind(selected.input_sha256())
            .execute(&mut **tx).await?.rows_affected();
        if changed != 1 {
            return Err(Error::invalid_state("Selected close expired or changed"));
        }
        Ok(())
    }

    /// Release only the verification pin; identity and admission closure survive.
    pub async fn abandon_close(
        tx: &mut Transaction<'_, Postgres>,
        stream: i64,
        expected_revision: i64,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE log_stream SET close_phase='abandoned',close_revision=close_revision+1 \
             WHERE id=$1 AND close_phase='closing' AND close_revision=$2",
        )
        .bind(stream)
        .bind(expected_revision)
        .execute(&mut **tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Check every admission and completed-grant retry under the stream lock.
    /// Closure preserves exact accepted retries but never authorizes another PUT.
    pub async fn check_close_append(
        tx: &mut Transaction<'_, Postgres>,
        stream: i64,
        sequence: i64,
        size: i64,
        digest: &str,
        object_key: &str,
        provider: Option<&str>,
    ) -> Result<()> {
        let allowed: bool = sqlx::query_scalar(
            "SELECT close_phase IS NULL OR EXISTS(SELECT 1 FROM log_segment g \
             JOIN artifact_version v ON v.id=s.artifact_version \
             WHERE g.stream=s.id AND g.sequence=$2 AND g.size_bytes=$3 AND g.sha256=$4 \
               AND g.object_key=$5 AND ($6::text IS NULL OR g.provider_version=$6) \
               AND v.body_state IN ('pending','ready')) FROM log_stream s WHERE s.id=$1",
        )
        .bind(stream)
        .bind(sequence)
        .bind(size)
        .bind(digest)
        .bind(object_key)
        .bind(provider)
        .fetch_one(&mut **tx)
        .await?;
        if !allowed {
            return Err(Error::invalid_state(
                "log close forbids changed or new segment",
            ));
        }
        Ok(())
    }

    pub async fn segments_with_conn(
        connection: &mut PgConnection,
        stream: i64,
    ) -> Result<Vec<LogSegment>> {
        sqlx::query_as(&format!(
            "SELECT {SEGMENT_COLUMNS} FROM log_segment WHERE stream=$1 ORDER BY sequence"
        ))
        .bind(stream)
        .fetch_all(connection)
        .await
        .map_err(Into::into)
    }
    pub async fn find_by_artifact_version_with_conn(
        connection: &mut PgConnection,
        version: i64,
    ) -> Result<Option<LogStream>> {
        sqlx::query_as(&format!(
            "SELECT {STREAM_COLUMNS} FROM log_stream WHERE artifact_version=$1"
        ))
        .bind(version)
        .fetch_optional(connection)
        .await
        .map_err(Into::into)
    }
    pub async fn create_with_backend_with_conn(
        connection: &mut PgConnection,
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
        sqlx::query_as(&format!("INSERT INTO log_stream(artifact_version,backend,max_unflushed_bytes,max_unflushed_milliseconds) \
            VALUES($1,$2,$3,$4) RETURNING {STREAM_COLUMNS}"))
            .bind(artifact_version).bind(backend)
            .bind(i64::try_from(max_unflushed_bytes).map_err(|_| Error::validation("log byte limit too large"))?)
            .bind(i64::try_from(max_unflushed_milliseconds).map_err(|_| Error::validation("log time limit too large"))?)
            .fetch_one(connection).await.map_err(Into::into)
    }
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
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('log_stream'), hashtext($1::text))")
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

    pub async fn segments_in_byte_range(
        pool: &PgPool,
        stream_id: i64,
        byte_start: i64,
        byte_end: i64,
    ) -> Result<Vec<LogSegment>> {
        if byte_start < 0 || byte_end <= byte_start {
            return Err(Error::validation("invalid log segment byte range"));
        }
        let query = format!(
            "SELECT {SEGMENT_COLUMNS} FROM log_segment \
             WHERE stream = $1 AND byte_end > $2 AND byte_start < $3 ORDER BY byte_start"
        );
        sqlx::query_as(&query)
            .bind(stream_id)
            .bind(byte_start)
            .bind(byte_end)
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

    pub async fn delivery_state(
        pool: &PgPool,
        artifact_version: i64,
        sequence: i64,
    ) -> Result<LogDeliveryState> {
        sqlx::query_as(
            "SELECT stream.id AS stream_id, stream.backend, stream.next_sequence, stream.sealed, \
                    segment.size_bytes AS segment_size, segment.sha256 AS segment_sha256 \
             FROM log_stream stream \
             LEFT JOIN log_segment segment \
               ON segment.stream = stream.id AND segment.sequence = $2 \
             WHERE stream.artifact_version = $1",
        )
        .bind(artifact_version)
        .bind(sequence)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| {
            Error::not_found(
                "log_stream",
                "artifact_version",
                artifact_version.to_string(),
            )
        })
    }

    pub async fn lock<'a>(tx: &mut Transaction<'a, Postgres>, stream_id: i64) -> Result<LogStream> {
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtext('log_stream'), hashtext(artifact_version::text)) \
             FROM log_stream WHERE id = $1",
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
