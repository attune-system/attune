//! Immutable final encrypted-log evidence. Call record only in the fenced
//! transaction that seals the exact stream and publishes its version Ready.
use super::worker_delivery::{DeliveryContext, DeliveryPermit};
use crate::{runtime_log_completion::FinalLogManifest, Error, Result};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

const COLUMNS: &str = "artifact_version,stream,execution,admission,attempt,worker,worker_instance,encoding,frame_count,plain_bytes,cipher_bytes,cipher_sha256,truncated,authenticated_manifest";

#[derive(Clone, FromRow)]
pub struct FinalLogRecord {
    pub artifact_version: i64,
    pub stream: i64,
    pub execution: i64,
    pub admission: i64,
    pub attempt: Uuid,
    pub worker: i64,
    pub worker_instance: Uuid,
    pub encoding: String,
    pub frame_count: i64,
    pub plain_bytes: i64,
    pub cipher_bytes: i64,
    pub cipher_sha256: String,
    pub truncated: bool,
    pub authenticated_manifest: String,
}

pub struct RuntimeLogCompletionRepository;
impl RuntimeLogCompletionRepository {
    pub async fn find(
        connection: &mut PgConnection,
        version: i64,
    ) -> Result<Option<FinalLogRecord>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM runtime_log_final_manifest WHERE artifact_version=$1"
        ))
        .bind(version)
        .fetch_optional(connection)
        .await?)
    }

    pub async fn record(
        connection: &mut PgConnection,
        context: &DeliveryContext,
        permit: &DeliveryPermit,
        manifest: &FinalLogManifest,
        authenticated: &str,
        truncated: bool,
    ) -> Result<()> {
        let denied =
            || Error::PermissionDenied("Encrypted log final manifest binding mismatch".into());
        let max_plain = permit.max_plaintext_bytes.ok_or_else(denied)?;
        let stream = permit.log_stream.ok_or_else(denied)?;
        if permit.context != context.id
            || context.deadline <= chrono::Utc::now()
            || context.execution != manifest.execution
            || permit.artifact_version != manifest.version
            || manifest.plaintext_bytes > max_plain as u64
            || manifest.ciphertext_bytes > permit.max_bytes as u64
            || manifest.frame_count
                > (max_plain as u64).div_ceil(crate::runtime_read_log::FRAME_BYTES as u64)
        {
            return Err(denied());
        }
        sqlx::query("INSERT INTO runtime_log_final_manifest(artifact_version,stream,execution,admission,attempt,worker,worker_instance,encoding,frame_count,plain_bytes,cipher_bytes,cipher_sha256,truncated,authenticated_manifest) \
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) ON CONFLICT(artifact_version) DO NOTHING")
            .bind(manifest.version).bind(stream).bind(context.execution).bind(context.admission).bind(context.attempt)
            .bind(context.worker).bind(context.worker_instance).bind(&manifest.encoding)
            .bind(i64::try_from(manifest.frame_count).map_err(|_| denied())?)
            .bind(i64::try_from(manifest.plaintext_bytes).map_err(|_| denied())?)
            .bind(i64::try_from(manifest.ciphertext_bytes).map_err(|_| denied())?)
            .bind(&manifest.ciphertext_sha256).bind(truncated).bind(authenticated).execute(&mut *connection).await?;
        let stored = Self::find(connection, manifest.version)
            .await?
            .ok_or_else(denied)?;
        if stored.stream != stream
            || stored.execution != context.execution
            || stored.admission != context.admission
            || stored.attempt != context.attempt
            || stored.worker != context.worker
            || stored.worker_instance != context.worker_instance
            || stored.encoding != manifest.encoding
            || stored.frame_count as u64 != manifest.frame_count
            || stored.plain_bytes as u64 != manifest.plaintext_bytes
            || stored.cipher_bytes as u64 != manifest.ciphertext_bytes
            || stored.cipher_sha256 != manifest.ciphertext_sha256
            || stored.truncated != truncated
            || stored.authenticated_manifest != authenticated
        {
            return Err(denied());
        }
        Ok(())
    }
}
