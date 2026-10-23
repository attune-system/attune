//! Narrow file-body hooks into C's retained origin graph. Never infer provenance
//! from the request's metadata or from the artifact's mutable latest version.
use crate::middleware::{ApiError, ApiResult};
use attune_common::{
    repositories::{
        artifact::ArtifactVersionRepository,
        file_origin::{FileOrigin, FileOriginRepository},
        runtime_read::{
            RuntimeReadOrigin, RuntimeReadOwner, RuntimeReadRepository, RuntimeReadTarget,
        },
        runtime_target::{RuntimeTarget, RuntimeTargetRepository},
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
};
use sqlx::PgConnection;

fn denied() -> ApiError {
    ApiError::Forbidden("File source closure is unavailable or changed during delivery".into())
}

pub(crate) struct PreparedFileOrigins {
    rows: Vec<attune_common::repositories::execution_secret_value::SecretProvenanceRow>,
    budget: crate::recorded_disclosure::OriginBudget,
}
impl PreparedFileOrigins {
    pub(crate) fn ensure_fresh(&self) -> ApiResult<()> {
        self.budget.ensure_receipts_fresh()
    }
    pub(crate) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> ApiResult<()> {
        self.budget.ensure_receipts_fresh_on(conn).await
    }
}
fn same_rows(
    a: &[attune_common::repositories::execution_secret_value::SecretProvenanceRow],
    b: &[attune_common::repositories::execution_secret_value::SecretProvenanceRow],
) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.entity_type == b.entity_type
                && a.entity_id == b.entity_id
                && a.json_path == b.json_path
                && a.source_kind == b.source_kind
                && a.source_ref == b.source_ref
        })
}

/// Entry before any consumer capture or file target/version lock. A changed
/// frontier aborts this transaction; it is never prepared behind target locks.
pub(crate) async fn prepare_reservation(
    conn: &mut PgConnection,
    authority: &attune_common::repositories::runtime_authority::ValidatedRuntimeAuthority,
    key: Option<&str>,
) -> ApiResult<PreparedFileOrigins> {
    let execution = authority.credential_binding().execution_id;
    let rows = crate::recorded_disclosure::execution_origins(conn, execution).await?;
    let mut budget = crate::recorded_disclosure::OriginBudget::with_encryption_key(key);
    budget.prepare_receipts(conn, &rows).await?;
    RuntimeReadRepository::lock_open(conn, authority.credential_binding().admission_id).await?;
    let captured = crate::recorded_disclosure::execution_origins(conn, execution).await?;
    if !same_rows(&rows, &captured) {
        return Err(denied());
    }
    crate::recorded_disclosure::authorize_origins_bounded(conn, authority, &rows, &mut budget)
        .await?;
    Ok(PreparedFileOrigins { rows, budget })
}

pub(crate) async fn prepare_existing(
    conn: &mut PgConnection,
    authority: Option<&attune_common::repositories::runtime_authority::ValidatedRuntimeAuthority>,
    version: i64,
    key: Option<&str>,
) -> ApiResult<Option<crate::recorded_disclosure::OriginBudget>> {
    let Some(retained) = FileOriginRepository::find(conn, version).await? else {
        if let Some(authority) = authority {
            RuntimeReadRepository::lock_open(conn, authority.credential_binding().admission_id)
                .await?;
        }
        return Ok(None);
    };
    let budget = if let Some(authority) = authority {
        // Publication is a write to this Pending destination, not a read of it.
        // Authenticate its frozen ORIGINAL source rows, preserving their sites
        // and acquired-read references. C still authorizes every actual source;
        // ordinary readers still reject a Pending artifact body. Target write,
        // exact source/frontier equality and final freshness are checked below
        // by recorded_files + require, never replaced by this preparation.
        let rows = FileOriginRepository::retained_sources(conn, &retained).await?;
        crate::recorded_disclosure::authorize_origins_with_key(conn, authority, &rows, key).await?
    } else {
        // Worker completion revalidates typed source fences, but obtains no
        // process API authority and may inspect its CLOSED capture context.
        let mut budget = crate::recorded_disclosure::OriginBudget::with_encryption_key(key);
        let rows = [row(RuntimeReadRepository::reference(retained.origin_read))?];
        budget.prepare_receipts(conn, &rows).await?;
        FileOriginRepository::lock_capture(conn, retained.admission).await?;
        budget
    };
    Ok(Some(budget))
}

/// Recheck an existing capture, not a new read receipt. Called after blob/body
/// waits, including EOF and HEAD, with source -> capture -> target ordering.
pub(crate) async fn authorize_delivery(
    state: &crate::state::AppState,
    user: &crate::auth::middleware::AuthenticatedUser,
    captured: &SecretOrigin,
    version: &attune_common::models::ArtifactVersion,
    body: &attune_common::repositories::file_origin::FileBody,
) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;
    let authority = user
        .validate_recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(denied)?;
    let budget = crate::recorded_disclosure::authorize_origins_with_key(
        &mut tx,
        &authority,
        &[row(captured.clone())?],
        state.config.security.encryption_key.as_deref(),
    )
    .await?;
    FileOriginRepository::require_ready_body_on(&mut tx, version, body).await?;
    authority.ensure_fresh_on(&mut tx).await?;
    budget.ensure_receipts_fresh_on(&mut tx).await?;
    if user.claims.exp <= chrono::Utc::now().timestamp() {
        return Err(denied());
    }
    tx.commit().await?;
    authority.ensure_fresh()?;
    if user.claims.exp <= chrono::Utc::now().timestamp() {
        return Err(denied());
    }
    // Pure check on the SAME opaque A contexts prepared before capture/targets.
    // No application await may follow before this byte/HEAD/EOF handoff.
    budget.ensure_receipts_fresh()?;
    Ok(())
}

async fn target(
    conn: &mut PgConnection,
    version: i64,
    sponsor: i64,
) -> ApiResult<RuntimeReadTarget> {
    let version = ArtifactVersionRepository::find_by_id(&mut *conn, version)
        .await?
        .ok_or_else(denied)?;
    let owner =
        RuntimeTargetRepository::resolve(conn, RuntimeTarget::Artifact(version.artifact), sponsor)
            .await?
            .ok_or_else(denied)?;
    Ok(RuntimeReadTarget::ArtifactVersion {
        artifact: version.artifact,
        version: version.id,
        execution: version.execution,
        owner: RuntimeReadOwner::of(&owner),
        encrypted_manifest: None,
    })
}

fn row(
    origin: SecretOrigin,
) -> ApiResult<attune_common::repositories::execution_secret_value::SecretProvenanceRow> {
    Ok(
        attune_common::repositories::execution_secret_value::SecretProvenanceRow {
            entity_type: "artifact_file_body".into(),
            entity_id: 0,
            json_path: String::new(),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(&SecretProvenance {
                    origins: vec![origin],
                    templates: vec![],
                })
                .map_err(|_| denied())?,
            ),
        },
    )
}

pub(crate) async fn reserve(
    conn: &mut PgConnection,
    authority: &attune_common::repositories::runtime_authority::ValidatedRuntimeAuthority,
    version: i64,
    prepared: &PreparedFileOrigins,
) -> ApiResult<()> {
    let binding = authority.credential_binding();
    RuntimeReadRepository::lock_open(conn, binding.admission_id).await?;
    let rows = crate::recorded_disclosure::execution_origins(conn, binding.execution_id).await?;
    if !same_rows(&rows, &prepared.rows) {
        return Err(denied());
    }
    let origin = RuntimeReadOrigin {
        target: target(conn, version, binding.sponsor_identity_id).await?,
        provenance: crate::recorded_disclosure::snapshot(&rows)?,
        parents: prepared.budget.retained_parents(),
    };
    // Unbound legacy/template sources are rejected by the common evaluator.
    // Capture actual delivered inputs and acquired reads, not current template values.
    FileOriginRepository::reserve(
        conn,
        version,
        binding.admission_id,
        binding.execution_id,
        binding.attempt,
        binding.sponsor_identity_id,
        &origin,
        &rows,
    )
    .await?;
    prepared.ensure_fresh_on(conn).await?;
    Ok(())
}

/// Called under the source/target mutation fence in BOTH publication transactions.
/// A later read or changed source set denies; an old upload is never relabeled.
pub(crate) async fn require(
    conn: &mut PgConnection,
    source: &crate::recorded_files::FileMutationProof,
    version: i64,
) -> ApiResult<Option<FileOrigin>> {
    let retained = FileOriginRepository::find(conn, version).await?;
    let Some(retained) = retained else {
        if source.process.is_some() || source.worker.is_some() {
            let permit=attune_common::repositories::worker_delivery::WorkerDeliveryRepository::for_version(conn,version).await?.ok_or_else(denied)?;
            if permit.log_stream.is_none() {
                return Err(denied());
            }
        }
        return Ok(None);
    };
    let (admission, execution, attempt, sponsor) = if let Some((context, _, _)) = &source.worker {
        (
            context.admission,
            context.execution,
            context.attempt,
            retained.sponsor,
        )
    } else {
        let authority = source.process.as_ref().ok_or_else(denied)?;
        let b = authority.credential_binding();
        (
            b.admission_id,
            b.execution_id,
            b.attempt,
            b.sponsor_identity_id,
        )
    };
    if (admission, execution, attempt) != (retained.admission, retained.execution, retained.attempt)
    {
        return Err(denied());
    }
    let original = RuntimeReadRepository::find(&mut *conn, retained.origin_read).await?;
    let ready = ArtifactVersionRepository::find_by_id(&mut *conn, version)
        .await?
        .ok_or_else(denied)?
        .body_state
        == Some(attune_common::models::ArtifactBodyState::Ready);
    if ready {
        // An exact Ready retry cannot incorporate later reads: publication
        // verifies its plaintext digest and never changes its body or proof.
        if target(conn, version, sponsor).await? != original.target {
            return Err(denied());
        }
    } else {
        FileOriginRepository::lock_capture(conn, admission).await?;
        let rows = crate::recorded_disclosure::execution_origins(conn, execution).await?;
        let fresh = RuntimeReadOrigin {
            target: target(conn, version, sponsor).await?,
            provenance: crate::recorded_disclosure::snapshot(&rows)?,
            parents: original.parents,
        };
        FileOriginRepository::require_sources(conn, &retained, &fresh, &rows).await?;
    }
    let budget = source.file_origin_budget.as_ref().ok_or_else(denied)?;
    budget.ensure_receipts_fresh_on(conn).await?;
    Ok(Some(retained))
}

pub(crate) async fn reference(
    conn: &mut PgConnection,
    version: i64,
) -> ApiResult<Option<SecretProvenance>> {
    Ok(FileOriginRepository::find(conn, version)
        .await?
        .map(|proof| SecretProvenance {
            origins: vec![RuntimeReadRepository::reference(proof.origin_read)],
            templates: vec![],
        }))
}
