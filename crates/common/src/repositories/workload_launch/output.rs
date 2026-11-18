//! Restart-safe setup output reservation and native-provider settlement.
//! A stopped engine resource is historical evidence, never upload authority.
use super::*;
use crate::blob_store::{FilePutSettlement, StoredObject};

const OUTPUT_COLUMNS:&str="id,launch,kind,size_bytes,sha256,object_key,token_hash,encrypted_token,state,provider_version,validation_receipt,object_id,cleanup_token,cleanup_required,cleanup_settled,expires_at,lease_generation";
#[derive(sqlx::FromRow)]
struct Output {
    id: i64,
    launch: i64,
    kind: String,
    size_bytes: i64,
    sha256: String,
    object_key: String,
    token_hash: String,
    encrypted_token: String,
    state: String,
    provider_version: Option<String>,
    validation_receipt: Option<i64>,
    object_id: Option<i64>,
    cleanup_token: Uuid,
    cleanup_required: bool,
    cleanup_settled: bool,
    expires_at: DateTime<Utc>,
    lease_generation: i64,
}
/// Not serializable: provider paths and validation budgets stay API-private.
pub struct OutputBodyPermit {
    pub key: String,
    pub size: u64,
    pub sha256: Sha256Digest,
    pub policy: crate::workload_archive::ArchivePolicy,
}
/// Native provider cleanup proof may be constructed only by the repository.
pub struct LaunchOutputCleanup {
    output: i64,
    launch: i64,
    key: String,
    token: Uuid,
}
impl LaunchOutputCleanup {
    pub fn key(&self) -> Result<crate::blob_store::ObjectKey> {
        crate::blob_store::ObjectKey::new(self.key.clone()).map_err(|_| denied())
    }
    pub fn token(&self) -> Uuid {
        self.token
    }
}
type OutputPageRow = (i64, i64, String, String, Uuid);
impl WorkloadLaunchRepository {
    async fn output(
        tx: &mut Transaction<'_, Postgres>,
        launch: i64,
        id: Option<i64>,
    ) -> Result<Option<Output>> {
        let output:Option<Output>=sqlx::query_as(&format!("SELECT {OUTPUT_COLUMNS} FROM workload_launch_output WHERE launch=$1 AND ($2::BIGINT IS NULL OR id=$2) AND kind='stage_archive' FOR UPDATE"))
            .bind(launch).bind(id).fetch_optional(&mut **tx).await?;
        if output
            .as_ref()
            .is_some_and(|output| output.launch != launch || output.kind != "stage_archive")
        {
            return Err(denied());
        }
        Ok(output)
    }
    fn publication(record: &Record) -> Result<crate::workload_archive::ArchivePolicy> {
        if record.manifest.purpose != LaunchPurpose::Setup
            || record.operation.lease_purpose != LaunchLeasePurpose::Publication
            || !record.operation.status.permits_stage_publication()
        {
            return Err(denied());
        }
        let stage = record.manifest.stage.as_ref().ok_or_else(denied)?;
        let archive_overhead = stage.quota_files.checked_mul(4096).ok_or_else(denied)?;
        let max_compressed_bytes = stage
            .quota_bytes
            .checked_add(archive_overhead)
            .ok_or_else(denied)?
            .min(1024 * 1024 * 1024);
        Ok(crate::workload_archive::ArchivePolicy {
            root: LaunchInputKind::Runtime,
            max_compressed_bytes,
            max_expanded_bytes: stage.quota_bytes,
            max_files: stage.quota_files,
        })
    }
    pub async fn reserve_output(
        pool: &PgPool,
        actor: ControllerActor,
        request: &ReserveLaunchOutput,
        lease: &LaunchToken,
        master: &str,
    ) -> Result<LaunchOutputGrant> {
        if request.kind != LaunchOutputKind::StageArchive || request.size_bytes == 0 {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        let key = LaunchRequest {
            launch_id: request.handle.launch_id,
            admitted_digest: request.handle.admitted_digest.clone(),
        };
        let mut record = Self::admitted_for_publication(&mut tx, actor, &key).await?;
        let now = Self::now(&mut tx).await?;
        Self::require_lease(&record, actor, &request.handle, lease, now)?;
        let policy = Self::publication(&record)?;
        if request.size_bytes > policy.max_compressed_bytes {
            return Err(denied());
        }
        if Self::output(&mut tx, record.id, None).await?.is_none() {
            let token = new_token()?;
            let object_key = format!("workload-stage/{}", Uuid::new_v4().simple());
            sqlx::query("INSERT INTO workload_launch_output(launch,kind,size_bytes,sha256,object_key,token_hash,encrypted_token,state,cleanup_token,expires_at,lease_generation) VALUES($1,'stage_archive',$2,$3,$4,$5,$6,'reserved',$7,$8,$9)")
                .bind(record.id).bind(i64::try_from(request.size_bytes).map_err(|_|denied())?).bind(request.sha256.as_str()).bind(object_key).bind(credential_hash(&token)).bind(crypto::encrypt(token.as_str(),master)?).bind(Uuid::new_v4()).bind(record.operation.lease_deadline).bind(record.operation.lease_generation).execute(&mut *tx).await?;
        }
        let mut output = Self::output(&mut tx, record.id, None)
            .await?
            .ok_or_else(denied)?;
        if output.lease_generation != record.operation.lease_generation {
            let token = new_token()?;
            output.token_hash = credential_hash(&token);
            output.encrypted_token = crypto::encrypt(token.as_str(), master)?;
            output.lease_generation = record.operation.lease_generation;
        }
        output.expires_at = record.operation.lease_deadline;
        sqlx::query("UPDATE workload_launch_output SET token_hash=$2,encrypted_token=$3,expires_at=$4,lease_generation=$5 WHERE id=$1")
            .bind(output.id).bind(&output.token_hash).bind(&output.encrypted_token).bind(output.expires_at).bind(output.lease_generation).execute(&mut *tx).await?;
        if output.size_bytes as u64 != request.size_bytes
            || output.sha256 != request.sha256.as_str()
            || output.state == "rejected"
            || output.cleanup_required
        {
            return Err(denied());
        }
        let token = LaunchToken::new(
            crypto::decrypt(&output.encrypted_token, master).map_err(|_| denied())?,
        )?;
        if record.operation.status.publication == LaunchPublicationState::Pending {
            record.operation.status.publication = LaunchPublicationState::Uploading;
        }
        let expires_at = record
            .effect_deadline
            .map_or(record.operation.lease_deadline, |source| {
                source.min(record.operation.lease_deadline)
            });
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(LaunchOutputGrant {
            output_id: output.id,
            token,
            expires_at,
        })
    }
    pub async fn output_body_permit(
        pool: &PgPool,
        actor: ControllerActor,
        request: &LaunchRequest,
        id: i64,
        lease: &LaunchToken,
        grant: &LaunchToken,
    ) -> Result<OutputBodyPermit> {
        let mut tx = pool.begin().await?;
        let record = Self::admitted_for_publication(&mut tx, actor, request).await?;
        Self::require_lease(
            &record,
            actor,
            &record.operation.handle(),
            lease,
            Self::now(&mut tx).await?,
        )?;
        let policy = Self::publication(&record)?;
        let output = Self::output(&mut tx, record.id, Some(id))
            .await?
            .ok_or_else(denied)?;
        if output.token_hash != credential_hash(grant)
            || output.state == "rejected"
            || output.cleanup_required
            || output.expires_at <= Self::now(&mut tx).await?
            || output.lease_generation != record.operation.lease_generation
            || output.size_bytes as u64 > policy.max_compressed_bytes
        {
            return Err(denied());
        }
        let result = OutputBodyPermit {
            key: output.object_key,
            size: output.size_bytes as u64,
            sha256: Sha256Digest::parse(output.sha256)?,
            policy,
        };
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(result)
    }
    fn result(output: &Output) -> Result<LaunchStageResult> {
        if output.state != "published" {
            return Err(denied());
        }
        Ok(LaunchStageResult {
            output_id: output.id,
            object_id: output.object_id.ok_or_else(denied)?,
            object_version: output.provider_version.clone().ok_or_else(denied)?,
            size_bytes: output.size_bytes as u64,
            sha256: Sha256Digest::parse(output.sha256.clone())?,
            validation_receipt: output.validation_receipt.ok_or_else(denied)?,
        })
    }
    /// Publication repeats source/epoch/attempt checks after native provider I/O.
    /// The callback carries a native StoredObject, never untrusted JSON evidence.
    pub async fn publish_output(
        pool: &PgPool,
        actor: ControllerActor,
        request: &LaunchRequest,
        id: i64,
        lease: &LaunchToken,
        grant: &LaunchToken,
        object: &StoredObject,
    ) -> Result<LaunchStageResult> {
        let mut tx = pool.begin().await?;
        let mut record = Self::admitted_for_publication(&mut tx, actor, request).await?;
        Self::require_lease(
            &record,
            actor,
            &record.operation.handle(),
            lease,
            Self::now(&mut tx).await?,
        )?;
        let _policy = Self::publication(&record)?;
        let mut output = Self::output(&mut tx, record.id, Some(id))
            .await?
            .ok_or_else(denied)?;
        if output.token_hash != credential_hash(grant)
            || output.state == "rejected"
            || output.cleanup_required
            || output.expires_at <= Self::now(&mut tx).await?
            || output.lease_generation != record.operation.lease_generation
            || output.object_key != object.key.as_str()
            || output.size_bytes as u64 != object.size
            || output.sha256
                != object
                    .sha256
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
        {
            return Err(denied());
        }
        if output.state == "published" {
            if output.provider_version.as_deref() != Some(object.provider_version.as_stored()) {
                return Err(denied());
            }
            let result = Self::result(&output)?;
            Self::actor(&mut tx, actor).await?;
            tx.commit().await?;
            return Ok(result);
        }
        let (source, binding): (Json<LaunchTarget>, Json<RuntimeStageBinding>) = sqlx::query_as(
            "SELECT source_target,stage_binding FROM workload_launch_prerequisite WHERE id=$1",
        )
        .bind(match record.manifest.target {
            LaunchTarget::Setup { admission, .. } => admission,
            _ => return Err(denied()),
        })
        .fetch_one(&mut *tx)
        .await?;
        let stage = record.manifest.stage.as_ref().ok_or_else(denied)?;
        let object_id:i64=sqlx::query_scalar("INSERT INTO workload_launch_object(source_target,kind,object_key,provider_version,size_bytes,sha256,max_expanded_bytes,max_files,built_image_sha256,build_launch,stage_binding,validation_receipt) VALUES($1,'runtime',$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING id")
            .bind(source).bind(&output.object_key).bind(object.provider_version.as_stored()).bind(output.size_bytes).bind(&output.sha256)
            .bind(i64::try_from(stage.quota_bytes).map_err(|_|denied())?).bind(i64::try_from(stage.quota_files).map_err(|_|denied())?).bind(record.manifest.image.sha256.as_str()).bind(record.id).bind(binding).bind(output.id).fetch_one(&mut *tx).await?;
        sqlx::query("UPDATE workload_launch_output SET state='published',provider_version=$2,object_id=$3,validation_receipt=id WHERE id=$1 AND state IN ('reserved','uploaded')")
            .bind(output.id).bind(object.provider_version.as_stored()).bind(object_id).execute(&mut *tx).await?;
        record.operation.status.publication = LaunchPublicationState::Published;
        output.state = "published".into();
        output.provider_version = Some(object.provider_version.as_stored().into());
        output.object_id = Some(object_id);
        output.validation_receipt = Some(output.id);
        let result = Self::result(&output)?;
        Self::save(&mut tx, &record).await?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(result)
    }
    /// A historical receipt is an acknowledgement, not current upload, execution
    /// or input-disclosure authority. It cannot recreate a retained-away body.
    pub async fn stage_result(
        pool: &PgPool,
        actor: ControllerActor,
        request: &LaunchRequest,
        dispatch: &LaunchToken,
    ) -> Result<LaunchStageResult> {
        let mut tx = pool.begin().await?;
        Self::actor(&mut tx, actor).await?;
        let record = Self::record(&mut tx, request, true).await?;
        if record.engine != actor.engine
            || record.dispatch_hash != credential_hash(dispatch)
            || record.operation.status.publication != LaunchPublicationState::Published
        {
            return Err(denied());
        }
        let result = Self::result(
            &Self::output(&mut tx, record.id, None)
                .await?
                .ok_or_else(denied)?,
        )?;
        Self::actor(&mut tx, actor).await?;
        tx.commit().await?;
        Ok(result)
    }
}

impl WorkloadLaunchRepository {
    async fn output_page(pool: &PgPool, cleanup: bool, limit: i64) -> Result<Vec<OutputPageRow>> {
        let mut tx = pool.begin().await?;
        let kind = if cleanup { 2i64 } else { 1i64 };
        let (after, mut high): (i64, i64) = sqlx::query_as(
            "SELECT after_id,high_water FROM workload_launch_maintenance WHERE id=$1 FOR UPDATE",
        )
        .bind(kind)
        .fetch_one(&mut *tx)
        .await?;
        let predicate = if cleanup {
            "o.cleanup_required AND NOT o.cleanup_settled"
        } else {
            "NOT o.cleanup_required"
        };
        if high == 0 {
            high = sqlx::query_scalar(&format!(
                "SELECT COALESCE(MAX(o.id),0) FROM workload_launch_output o WHERE {predicate}"
            ))
            .fetch_one(&mut *tx)
            .await?;
        }
        let rows:Vec<OutputPageRow>=sqlx::query_as(&format!("SELECT o.id,o.launch,l.admitted_digest,o.object_key,o.cleanup_token FROM workload_launch_output o JOIN workload_launch l ON l.id=o.launch WHERE o.id>$1 AND o.id<=$2 AND {predicate} ORDER BY o.id LIMIT $3"))
            .bind(after).bind(high).bind(limit.clamp(1,64)).fetch_all(&mut *tx).await?;
        let (next, high) = match rows.last() {
            Some((id, ..)) if *id < high => (*id, high),
            _ => (0, 0),
        };
        sqlx::query("UPDATE workload_launch_maintenance SET after_id=$2,high_water=$3 WHERE id=$1")
            .bind(kind)
            .bind(next)
            .bind(high)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(rows)
    }
    /// Bounded supervisor continuation, independent of the original HTTP future.
    /// Revoked/expired source epochs close publication before native fencing.
    pub async fn reconcile_outputs(pool: &PgPool, limit: i64) -> Result<u64> {
        let requests = Self::output_page(pool, false, limit).await?;
        let mut changed = 0;
        for (_, id, digest, _, _) in requests {
            let request = LaunchRequest {
                launch_id: id,
                admitted_digest: Sha256Digest::parse(digest)?,
            };
            let mut tx = pool.begin().await?;
            let snapshot = Self::record(&mut tx, &request, false).await?;
            let valid = match Self::epochs(&mut tx, &snapshot).await {
                Ok((epochs, deadline)) => {
                    epochs == snapshot.source_epochs.0
                        && !deadline.is_some_and(|deadline| deadline <= Utc::now())
                }
                Err(Error::PermissionDenied(_)) => false,
                Err(error) => return Err(error),
            };
            let mut record = Self::record(&mut tx, &request, true).await?;
            if valid && record.operation.status.publication != LaunchPublicationState::Rejected {
                tx.commit().await?;
                continue;
            }
            if record.operation.publication_pending() {
                record
                    .operation
                    .stop_at(LaunchStopReason::SourceRevoked, Self::now(&mut tx).await?);
            }
            let output = Self::output(&mut tx, id, None).await?.ok_or_else(denied)?;
            sqlx::query("UPDATE workload_launch_output SET cleanup_required=true,state=CASE WHEN state='published' THEN state ELSE 'rejected' END WHERE id=$1")
                .bind(output.id).execute(&mut *tx).await?;
            if let Some(object) = output.object_id {
                sqlx::query("UPDATE workload_launch_object SET retired_at=COALESCE(retired_at,clock_timestamp()) WHERE id=$1")
                    .bind(object).execute(&mut *tx).await?;
            }
            Self::save(&mut tx, &record).await?;
            tx.commit().await?;
            changed += 1;
        }
        Ok(changed)
    }
    pub async fn output_cleanup_candidates(
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<LaunchOutputCleanup>> {
        let rows = Self::output_page(pool, true, limit).await?;
        Ok(rows
            .into_iter()
            .map(|(output, launch, _, key, token)| LaunchOutputCleanup {
                output,
                launch,
                key,
                token,
            })
            .collect())
    }
    pub async fn acknowledge_output_cleanup(
        pool: &PgPool,
        plan: &LaunchOutputCleanup,
        settlement: &FilePutSettlement,
    ) -> Result<()> {
        if settlement.key().as_str() != plan.key || settlement.token() != plan.token {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        let output = Self::output(&mut tx, plan.launch, Some(plan.output))
            .await?
            .ok_or_else(denied)?;
        if output.object_key != plan.key || output.cleanup_token != plan.token {
            return Err(denied());
        }
        if !output.cleanup_settled {
            let changed=sqlx::query("UPDATE workload_launch_output SET cleanup_settled=true,cleanup_provider_version=$2 WHERE id=$1 AND cleanup_required AND NOT cleanup_settled")
                .bind(plan.output).bind(settlement.provider().as_stored()).execute(&mut *tx).await?.rows_affected();
            if changed != 1 {
                return Err(denied());
            }
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn output_maintenance_pending(pool: &PgPool) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workload_launch_maintenance WHERE after_id<high_water) OR EXISTS(SELECT 1 FROM workload_launch_output WHERE cleanup_required AND NOT cleanup_settled)").fetch_one(pool).await?)
    }
    pub async fn output_reconcile_pending(pool: &PgPool) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT after_id<high_water FROM workload_launch_maintenance WHERE id=1",
        )
        .fetch_one(pool)
        .await?)
    }
}
