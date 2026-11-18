//! File allocation shares the operation fence with durable Run close. The
//! caller's owning transaction retains source -> epoch -> operation protection
//! through exact version allocation and binding; no snapshot by execution ID.
use super::*;
use crate::{
    artifact_transport::ValidatedRelativePath,
    workload_artifact::{validate_inventory, ArtifactLimits, PreparedArtifactFile},
};

pub struct ArtifactAllocation<'tx, 'connection> {
    publication: i64,
    launch: i64,
    target: LaunchTarget,
    deadline: DateTime<Utc>,
    transaction: &'tx mut Transaction<'connection, Postgres>,
}
impl ArtifactAllocation<'_, '_> {
    pub fn connection(&mut self) -> &mut sqlx::PgConnection {
        self.transaction
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if self.deadline <= Utc::now() {
            return Err(denied());
        }
        Ok(())
    }
}

impl WorkloadLaunchRepository {
    /// Trusted token consumers resolve `target` from their authenticated exact
    /// attempt/host binding, never from an artifact request. Invoke before any
    /// target artifact lock. Use this SAME transaction for bind_artifact_file.
    pub async fn authorize_artifact_allocation<'tx, 'connection>(
        tx: &'tx mut Transaction<'connection, Postgres>,
        target: &LaunchTarget,
        owner: i64,
    ) -> Result<ArtifactAllocation<'tx, 'connection>> {
        let purpose = match target {
            LaunchTarget::Execution { .. } => "action",
            LaunchTarget::Sensor { .. } => "sensor",
            LaunchTarget::CandidateTest { .. } => "candidate_test",
            _ => return Err(denied()),
        };
        let request: Option<(i64, String)> = sqlx::query_as(
            "SELECT id,admitted_digest FROM workload_launch WHERE manifest->>'purpose'=$1 AND manifest->'target'=$2",
        )
        .bind(purpose)
        .bind(Json(target))
        .fetch_optional(&mut **tx)
        .await?;
        let (launch_id, digest) = request.ok_or_else(denied)?;
        let request = LaunchRequest {
            launch_id,
            admitted_digest: Sha256Digest::parse(digest)?,
        };
        let snapshot = Self::record(tx, &request, false).await?;
        if snapshot.owner != owner {
            return Err(denied());
        }
        let (epochs, source_deadline) = Self::epochs_for(tx, &snapshot, false).await?;
        let record = Self::record(tx, &request, true).await?;
        let now = Self::now(tx).await?;
        if !Self::same_epochs(&epochs, &record.source_epochs, false)
            || record.operation.status.state != LaunchState::Created
            || record.operation.status.runtime_state != LaunchRuntimeState::Running
            || record.operation.lease_deadline <= now
            || source_deadline.is_some_and(|deadline| deadline <= now)
        {
            return Err(denied());
        }
        let publication: i64 = sqlx::query_scalar(
            "SELECT id FROM workload_artifact_publication WHERE launch=$1 AND state='open' FOR SHARE",
        )
        .bind(record.id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)?;
        Ok(ArtifactAllocation {
            publication,
            launch: record.id,
            target: target.clone(),
            deadline: source_deadline.map_or(record.operation.lease_deadline, |deadline| {
                deadline.min(record.operation.lease_deadline)
            }),
            transaction: tx,
        })
    }

    /// Exact immutable inventory member, committed atomically with allocation.
    /// This method is not an upload grant and does not classify artifact bytes.
    /// Existing output policy/origin admission must precede this call.
    pub async fn bind_artifact_file(
        allocation: &mut ArtifactAllocation<'_, '_>,
        version: i64,
        max_bytes: u64,
    ) -> Result<()> {
        allocation.ensure_fresh()?;
        let deadline = allocation.deadline;
        let tx = &mut *allocation.transaction;
        if deadline <= Self::now(tx).await? {
            return Err(denied());
        }
        let max_bytes = i64::try_from(max_bytes).map_err(|_| denied())?;
        if version <= 0 || max_bytes <= 0 {
            return Err(denied());
        }
        // The borrowed transaction retains admission even across savepoints;
        // it cannot be committed/rolled back while this proof exists. No source
        // locks are acquired after the artifact version lock.
        let open: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workload_launch l JOIN workload_artifact_publication p ON p.launch=l.id \
             WHERE l.id=$1 AND p.id=$2 AND p.state='open' AND l.manifest->'target'=$3 \
             AND l.operation#>>'{status,state}'='created' AND l.operation#>>'{status,runtime_state}'='running' \
             AND (l.operation->>'lease_deadline')::timestamptz>clock_timestamp())",
        )
        .bind(allocation.launch)
        .bind(allocation.publication)
        .bind(Json(&allocation.target))
        .fetch_one(&mut **tx)
        .await?;
        if !open {
            return Err(denied());
        }
        let prepared: Option<(Option<String>, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT file_path,execution,body_state::text FROM artifact_version WHERE id=$1 AND provider_version IS NULL AND sha256 IS NULL AND content IS NULL AND content_json IS NULL FOR SHARE",
        )
        .bind(version)
        .fetch_optional(&mut **tx)
        .await?;
        let (path, execution, body_state) = prepared.ok_or_else(denied)?;
        let path = path.ok_or_else(denied)?;
        ValidatedRelativePath::new(&path).map_err(|_| denied())?;
        if body_state.as_deref() != Some("pending")
            || matches!(&allocation.target, LaunchTarget::Execution { execution: own, .. } if execution != Some(*own))
        {
            return Err(denied());
        }
        let (total_bytes, entries): (i64,i64) = sqlx::query_as(
            "SELECT max_total_bytes,max_entries FROM workload_artifact_publication WHERE id=$1 AND state='open'",
        ).bind(allocation.publication).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
        let rows: Vec<(i64,String,i64)> = sqlx::query_as(
            "SELECT artifact_version,relative_path,max_bytes FROM workload_artifact_file WHERE publication=$1 AND artifact_version<>$2 ORDER BY artifact_version",
        ).bind(allocation.publication).bind(version).fetch_all(&mut **tx).await?;
        let mut inventory = Vec::with_capacity(rows.len() + 1);
        let mut remaining = u64::try_from(total_bytes).map_err(|_| denied())?;
        for (version_id, relative_path, max_bytes) in rows {
            let max_bytes = u64::try_from(max_bytes).map_err(|_| denied())?;
            remaining = remaining.checked_sub(max_bytes).ok_or_else(denied)?;
            inventory.push(PreparedArtifactFile {
                version_id,
                relative_path,
                max_bytes,
            });
        }
        let max_bytes = max_bytes.min(i64::try_from(remaining).map_err(|_| denied())?);
        inventory.push(PreparedArtifactFile {
            version_id: version,
            relative_path: path.clone(),
            max_bytes: u64::try_from(max_bytes).map_err(|_| denied())?,
        });
        validate_inventory(
            &inventory,
            ArtifactLimits {
                max_total_bytes: u64::try_from(total_bytes).map_err(|_| denied())?,
                max_entries: u64::try_from(entries).map_err(|_| denied())?,
            },
        )?;
        sqlx::query(
            "INSERT INTO workload_artifact_file(publication,artifact_version,relative_path,max_bytes) \
             VALUES($1,$2,$3,$4) ON CONFLICT(artifact_version) DO NOTHING",
        )
        .bind(allocation.publication)
        .bind(version)
        .bind(&path)
        .bind(max_bytes)
        .execute(&mut **tx)
        .await?;
        let exact: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workload_artifact_file WHERE publication=$1 AND artifact_version=$2 AND relative_path=$3 AND max_bytes=$4 AND state='prepared')",
        )
        .bind(allocation.publication)
        .bind(version)
        .bind(path)
        .bind(max_bytes)
        .fetch_one(&mut **tx)
        .await?;
        if !exact {
            return Err(denied());
        }
        if deadline <= Utc::now() || deadline <= Self::now(tx).await? {
            return Err(denied());
        }
        Ok(())
    }
}
