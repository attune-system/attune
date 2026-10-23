//! Isolated, bounded candidate writes. No production repository or notification is called.

use crate::{
    auth::{generate_integration_token, hash_integration_token},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub struct CandidateEffectRepository;

/// Worker-only return value. Never log or serialize the runtime secret into test results.
pub struct CandidateEffectLease {
    /// Opaque attempt/lease ID, distinct from the context table's BIGINT primary ID.
    pub id: Uuid,
    pub token: String,
}

/// Authenticated candidate proof, never user Claims. Fields and construction are
/// private; deliberately neither Debug nor Serialize. Consumption revalidates it.
pub struct AuthenticatedCandidateContext {
    lease: Uuid,
    token: String,
}

#[derive(Clone, Copy)]
pub struct CandidateArchiveBinding<'a> {
    pub key: &'a str,
    pub provider_version: &'a str,
    pub digest: &'a str,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateOperation {
    PutKey { target: String, value: Value },
    GetKey { target: String },
    PutTrigger { target: String },
    EmitEvent { target: String, payload: Value },
    ListEvents,
}

fn denied() -> Error {
    Error::PermissionDenied("Candidate context, archive, lease or target is unavailable".into())
}

impl CandidateEffectRepository {
    /// Guard-time authentication, without reserving an operation or retaining a
    /// permit across requests. Final effect admission repeats every live fence.
    pub async fn authenticate(
        pool: &PgPool,
        lease: Uuid,
        token: &str,
    ) -> Result<AuthenticatedCandidateContext> {
        let valid: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pack_install p JOIN pack_candidate_context c ON c.install_id = p.id WHERE c.lease = $1 AND c.runtime_token_hash = $2 AND NOT c.revoked AND c.expires_at > clock_timestamp() AND c.operations < 1024 AND p.status = 'running' AND p.assigned_worker_id = c.worker_id AND p.candidate_object_key = c.archive_key AND p.candidate_provider_version = c.archive_version AND p.candidate_digest = c.archive_digest AND p.candidate_access_token_hash = c.archive_token_hash AND p.started_at > clock_timestamp() - INTERVAL '7 hours')")
            .bind(lease).bind(hash_integration_token(token)).fetch_one(pool).await?;
        if !valid {
            return Err(denied());
        }
        Ok(AuthenticatedCandidateContext {
            lease,
            token: token.to_owned(),
        })
    }

    pub async fn apply_authenticated(
        pool: &PgPool,
        context: &AuthenticatedCandidateContext,
        operation: CandidateOperation,
    ) -> Result<Value> {
        Self::apply(pool, context.lease, &context.token, operation).await
    }

    /// Activation hook for an opt-in candidate test attempt. Call in the ordinary
    /// deployment transaction before changing projections. A successful result
    /// from an older worker without an isolated context is not test evidence.
    /// Explicitly skipped tests do not call this hook.
    pub async fn require_closed_context(
        connection: &mut sqlx::PgConnection,
        install: i64,
        archive: &CandidateArchiveBinding<'_>,
    ) -> Result<()> {
        // finish_running clears the download credential. Its former hash remains
        // immutable in the closed context; activation must not restore it.
        let lease: Option<Uuid> = sqlx::query_scalar("SELECT c.lease FROM pack_install p JOIN pack_candidate_context c ON c.install_id = p.id WHERE p.id = $1 AND p.status = 'activating' AND c.revoked AND p.assigned_worker_id = c.worker_id AND p.candidate_object_key = c.archive_key AND p.candidate_provider_version = c.archive_version AND p.candidate_digest = c.archive_digest AND p.candidate_access_token_hash IS NULL AND c.archive_key = $2 AND c.archive_version = $3 AND c.archive_digest = $4 FOR UPDATE OF p")
            .bind(install).bind(archive.key).bind(archive.provider_version).bind(archive.digest)
            .fetch_optional(connection).await?;
        lease.ok_or_else(denied)?;
        Ok(())
    }

    /// Bounded maintenance for abandoned/expired workers. Revoked tombstones stay
    /// until their install is deleted, so cleanup never permits reminting a lease.
    pub async fn purge_expired(pool: &PgPool, limit: i64) -> Result<usize> {
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT c.lease FROM pack_candidate_context c JOIN pack_install p ON p.id = c.install_id WHERE NOT c.revoked AND (c.expires_at <= clock_timestamp() OR p.status <> 'running' OR p.started_at <= clock_timestamp() - INTERVAL '7 hours') ORDER BY c.expires_at LIMIT $1")
            .bind(limit.clamp(1, 100)).fetch_all(pool).await?;
        for id in &ids {
            Self::close(pool, *id).await?;
        }
        Ok(ids.len())
    }
    /// Called only after the worker has verified/extracted the pinned archive and read opt-in.
    /// A context cannot be reminted, including after explicit revocation.
    pub async fn open(
        pool: &PgPool,
        install: i64,
        worker: i64,
        archive_token: &str,
        archive: &CandidateArchiveBinding<'_>,
    ) -> Result<CandidateEffectLease> {
        Self::purge_expired(pool, 16).await?;
        let token = generate_integration_token()?;
        let id = Uuid::new_v4();
        let mut tx = pool.begin().await?;
        // Lock install before context everywhere, including finish's database trigger.
        let row = sqlx::query("SELECT candidate_object_key, candidate_provider_version, candidate_digest, candidate_access_token_hash FROM pack_install WHERE id = $1 AND assigned_worker_id = $2 AND status = 'running' AND started_at > clock_timestamp() - INTERVAL '7 hours' FOR UPDATE")
            .bind(install).bind(worker).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
        let key: Option<String> = row.try_get("candidate_object_key")?;
        let version: Option<String> = row.try_get("candidate_provider_version")?;
        let stored_digest: Option<String> = row.try_get("candidate_digest")?;
        let archive_hash: Option<String> = row.try_get("candidate_access_token_hash")?;
        let archive_hash = archive_hash.ok_or_else(denied)?;
        if archive_hash != hash_integration_token(archive_token)
            || stored_digest.as_deref() != Some(archive.digest)
            || key.as_deref() != Some(archive.key)
            || version.as_deref() != Some(archive.provider_version)
        {
            return Err(denied());
        }
        let active: bool = sqlx::query_scalar("INSERT INTO pack_candidate_context (lease, install_id, worker_id, archive_key, archive_version, archive_digest, archive_token_hash, runtime_token_hash, expires_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,LEAST(clock_timestamp() + INTERVAL '6 hours', (SELECT started_at + INTERVAL '7 hours' FROM pack_install WHERE id = $2))) RETURNING expires_at > clock_timestamp()")
            .bind(id).bind(install).bind(worker).bind(archive.key).bind(archive.provider_version).bind(archive.digest).bind(archive_hash).bind(token.hash)
            .fetch_one(&mut *tx).await?;
        if !active {
            return Err(denied());
        }
        tx.commit().await?;
        Ok(CandidateEffectLease {
            id,
            token: token.secret,
        })
    }

    pub async fn close(pool: &PgPool, id: Uuid) -> Result<()> {
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT id FROM pack_install WHERE id = (SELECT install_id FROM pack_candidate_context WHERE lease = $1) FOR UPDATE")
            .bind(id).execute(&mut *tx).await?;
        sqlx::query("UPDATE pack_candidate_context SET revoked = TRUE WHERE lease = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM pack_candidate_resource WHERE context = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM pack_candidate_event WHERE context = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Authentication and effect admission share a transaction with the write. Credentials
    /// have no meaning to JWT consumers or to any production target resolver.
    pub async fn apply(
        pool: &PgPool,
        id: Uuid,
        token: &str,
        operation: CandidateOperation,
    ) -> Result<Value> {
        let token_hash = hash_integration_token(token);
        let mut tx = pool.begin().await?;
        let pack: Option<String> = sqlx::query_scalar("SELECT p.pack_ref FROM pack_install p JOIN pack_candidate_context c ON c.install_id = p.id WHERE c.lease = $1 AND c.runtime_token_hash = $2 AND NOT c.revoked AND c.expires_at > clock_timestamp() AND p.status = 'running' AND p.assigned_worker_id = c.worker_id AND p.candidate_object_key = c.archive_key AND p.candidate_provider_version = c.archive_version AND p.candidate_digest = c.archive_digest AND p.candidate_access_token_hash = c.archive_token_hash AND p.started_at > clock_timestamp() - INTERVAL '7 hours' FOR UPDATE OF p")
            .bind(id).bind(&token_hash).fetch_optional(&mut *tx).await?;
        let pack = pack.ok_or_else(denied)?;
        let admitted: Option<Uuid> = sqlx::query_scalar("UPDATE pack_candidate_context SET operations = operations + 1 WHERE lease = $1 AND runtime_token_hash = $2 AND NOT revoked AND expires_at > clock_timestamp() AND operations < 1024 RETURNING lease")
            .bind(id).bind(&token_hash).fetch_optional(&mut *tx).await?;
        admitted.ok_or_else(denied)?;
        let target = match &operation {
            CandidateOperation::PutKey { target, .. }
            | CandidateOperation::GetKey { target }
            | CandidateOperation::PutTrigger { target }
            | CandidateOperation::EmitEvent { target, .. } => Some(target),
            CandidateOperation::ListEvents => None,
        };
        if target.is_some_and(|target| {
            !target.starts_with(&format!("{pack}."))
                || target.len() > 255
                || target.len() <= pack.len() + 1
        }) {
            return Err(denied());
        }
        let result = match operation {
            CandidateOperation::PutKey { target, value } => Self::put(&mut tx, id, "key", &target, value).await?,
            CandidateOperation::PutTrigger { target } => Self::put(&mut tx, id, "trigger", &target, Value::Null).await?,
            CandidateOperation::GetKey { target } => sqlx::query_scalar("SELECT value FROM pack_candidate_resource WHERE context = $1 AND kind = 'key' AND ref = $2").bind(id).bind(target).fetch_optional(&mut *tx).await?.ok_or_else(denied)?,
            CandidateOperation::EmitEvent { target, payload } => {
                if serde_json::to_vec(&payload)?.len() > 65536 { return Err(Error::validation("Candidate payload exceeds 64 KiB")); }
                let event: Option<i64> = sqlx::query_scalar("INSERT INTO pack_candidate_event (context, trigger_ref, payload) SELECT $1,$2,$3 WHERE EXISTS (SELECT 1 FROM pack_candidate_resource WHERE context = $1 AND kind = 'trigger' AND ref = $2) AND (SELECT count(*) FROM pack_candidate_event WHERE context = $1) < 256 RETURNING id")
                    .bind(id).bind(target).bind(payload).fetch_optional(&mut *tx).await?;
                serde_json::json!({"id": event.ok_or_else(denied)?, "candidate_context": id})
            }
            CandidateOperation::ListEvents => {
                let events: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id', id, 'trigger', trigger_ref, 'payload', payload) FROM pack_candidate_event WHERE context = $1 ORDER BY id LIMIT 256").bind(id).fetch_all(&mut *tx).await?;
                Value::Array(events)
            }
        };
        tx.commit().await?;
        Ok(result)
    }

    async fn put(
        connection: &mut sqlx::PgConnection,
        id: Uuid,
        kind: &str,
        target: &str,
        value: Value,
    ) -> Result<Value> {
        if serde_json::to_vec(&value)?.len() > 65536 {
            return Err(Error::validation("Candidate value exceeds 64 KiB"));
        }
        let inserted: Option<Value> = sqlx::query_scalar("INSERT INTO pack_candidate_resource (context, kind, ref, value) SELECT $1,$2,$3,$4 WHERE (SELECT count(*) FROM pack_candidate_resource WHERE context = $1) < 128 OR EXISTS (SELECT 1 FROM pack_candidate_resource WHERE context = $1 AND kind = $2 AND ref = $3) ON CONFLICT (context, kind, ref) DO UPDATE SET value = EXCLUDED.value RETURNING value")
            .bind(id).bind(kind).bind(target).bind(value).fetch_optional(connection).await?;
        inserted.ok_or_else(denied)
    }
}
