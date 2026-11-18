//! Durable CP ownership of public input bodies before native-provider PUT.
//! Publication is a fresh source/epoch transaction; cancellation retains the
//! operation and create-only provider fence so delayed writes cannot revive it.
use super::*;
use crate::{
    blob_store::{FilePutSettlement, ObjectKey, StoredObject},
    workload_pack_input::{PreparedPackInput, ReleaseInputPin},
};
const INPUT_COLUMNS:&str="id,admission,source_target,source_epochs,release_sha256,object_key,size_bytes,sha256,expanded_bytes,entries,state,provider_version,object_id,cleanup_token,cleanup_required,cleanup_settled";
#[derive(sqlx::FromRow)]
struct InputRecord {
    id: i64,
    admission: i64,
    source_target: Json<LaunchTarget>,
    source_epochs: Json<Vec<SourceEpoch>>,
    release_sha256: String,
    object_key: String,
    size_bytes: i64,
    sha256: String,
    expanded_bytes: i64,
    entries: i64,
    state: String,
    provider_version: Option<String>,
    object_id: Option<i64>,
    cleanup_token: Uuid,
    cleanup_required: bool,
    cleanup_settled: bool,
}
/// Server-private provider ownership; not serializable or accepted over HTTP.
pub struct PackInputReservation {
    id: i64,
    admission: i64,
    key: String,
    size: u64,
    sha256: Sha256Digest,
}
impl PackInputReservation {
    pub fn key(&self) -> Result<ObjectKey> {
        ObjectKey::new(self.key.clone()).map_err(|_| denied())
    }
    pub fn size_bytes(&self) -> u64 {
        self.size
    }
    pub fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }
}
pub struct PublicInputCleanup {
    id: i64,
    key: String,
    token: Uuid,
}
impl PublicInputCleanup {
    pub fn key(&self) -> Result<ObjectKey> {
        ObjectKey::new(self.key.clone()).map_err(|_| denied())
    }
    pub fn token(&self) -> Uuid {
        self.token
    }
}
impl WorkloadLaunchRepository {
    async fn input_record(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<InputRecord> {
        sqlx::query_as(&format!(
            "SELECT {INPUT_COLUMNS} FROM workload_input_materialization WHERE id=$1 FOR UPDATE"
        ))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(denied)
    }
    async fn input_root(
        tx: &mut Transaction<'_, Postgres>,
        id: i64,
    ) -> Result<(ActionAdmission, Vec<SourceEpoch>, ReleaseInputPin)> {
        let snapshot:ActionAdmission=sqlx::query_as("SELECT id,source_target,image,source_epochs,artifact_limits,expires_at,revoked_at FROM workload_launch_prerequisite WHERE id=$1 AND purpose='action'")
            .bind(id).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
        let owners: Vec<i64> = snapshot
            .source_epochs
            .iter()
            .filter(|epoch| epoch.kind == "owner")
            .map(|epoch| epoch.subject)
            .collect();
        let [owner] = owners.as_slice() else {
            return Err(denied());
        };
        let mut proof = Self::source(tx, &snapshot.source_target, *owner).await?;
        let execution = match snapshot.source_target.0 {
            LaunchTarget::Execution { execution, .. } => execution,
            _ => return Err(denied()),
        };
        let (pack_ref,digest,size):(String,String,i64)=sqlx::query_as(
            "SELECT r.pack_ref,r.digest,r.archive_size FROM execution e JOIN pack_release r ON r.id=e.pack_release \
             WHERE e.id=$1 AND r.digest=e.pack_release_digest AND (e.executable_snapshot#>>'{release,id}')::bigint=r.id \
             AND e.executable_snapshot#>>'{release,digest}'=r.digest FOR SHARE OF r",
        ).bind(execution).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
        let engine:Option<i64>=sqlx::query_scalar("SELECT i.engine FROM workload_execution_image i JOIN workload_engine e ON e.id=i.engine WHERE i.id=$1 AND i.revoked_at IS NULL AND NOT e.disabled AND i.platform=e.platform FOR SHARE OF i,e")
            .bind(snapshot.image).fetch_optional(&mut **tx).await?;
        let root:ActionAdmission=sqlx::query_as("SELECT id,source_target,image,source_epochs,artifact_limits,expires_at,revoked_at FROM workload_launch_prerequisite WHERE id=$1 AND purpose='action' FOR SHARE")
            .bind(id).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
        if root.source_target.0 != snapshot.source_target.0
            || root.revoked_at.is_some()
            || root.expires_at <= Self::now(tx).await?
        {
            return Err(denied());
        }
        proof.sources.extend([
            ("image", root.image),
            ("engine", engine.ok_or_else(denied)?),
            ("admission", root.id),
        ]);
        let epochs = workload_source::snapshot(tx, proof.sources).await?;
        if root
            .source_epochs
            .iter()
            .any(|expected| !epochs.contains(expected))
        {
            return Err(denied());
        }
        Ok((
            root,
            epochs,
            ReleaseInputPin {
                pack_ref,
                size_bytes: u64::try_from(size).map_err(|_| denied())?,
                sha256: Sha256Digest::parse(digest)?,
            },
        ))
    }
    /// Reserve before native PUT. Only the bounded verified converter can
    /// construct the prepared body; no host supplies a path, digest or payload.
    pub async fn reserve_pack_input(
        pool: &PgPool,
        admission: i64,
        body: &PreparedPackInput,
    ) -> Result<PackInputReservation> {
        let mut tx = pool.begin().await?;
        let (root, epochs, release) = Self::input_root(&mut tx, admission).await?;
        if release.pack_ref != body.source_pin.pack_ref
            || release.size_bytes != body.source_pin.size_bytes
            || release.sha256 != body.source_pin.sha256
        {
            return Err(denied());
        }
        let size = i64::try_from(body.size_bytes).map_err(|_| denied())?;
        let expanded = i64::try_from(body.expanded_bytes).map_err(|_| denied())?;
        let entries = i64::try_from(body.entries).map_err(|_| denied())?;
        sqlx::query("INSERT INTO workload_input_materialization(admission,kind,source_target,source_epochs,release_sha256,object_key,size_bytes,sha256,expanded_bytes,entries) VALUES($1,'pack',$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(admission,kind) DO NOTHING")
            .bind(root.id).bind(&root.source_target).bind(Json(&epochs)).bind(release.sha256.as_str()).bind(format!("workload-input/{}",Uuid::new_v4().simple()))
            .bind(size).bind(body.sha256.as_str()).bind(expanded).bind(entries).execute(&mut *tx).await?;
        let row:InputRecord=sqlx::query_as(&format!("SELECT {INPUT_COLUMNS} FROM workload_input_materialization WHERE admission=$1 AND kind='pack' FOR UPDATE"))
            .bind(admission).fetch_one(&mut *tx).await?;
        if row.source_epochs.0 != epochs
            || row.source_target.0 != root.source_target.0
            || row.release_sha256 != release.sha256.as_str()
            || row.size_bytes != size
            || row.sha256 != body.sha256.as_str()
            || row.expanded_bytes != expanded
            || row.entries != entries
            || row.state == "cancelled"
            || row.cleanup_required
            || root.expires_at <= Self::now(&mut tx).await?
        {
            return Err(denied());
        }
        let reservation = PackInputReservation {
            id: row.id,
            admission: root.id,
            key: row.object_key,
            size: body.size_bytes,
            sha256: body.sha256.clone(),
        };
        tx.commit().await?;
        Ok(reservation)
    }
    fn input_pin(row: &InputRecord) -> Result<PinnedLaunchInput> {
        if row.state != "published" || row.cleanup_required {
            return Err(denied());
        }
        let id = row.object_id.ok_or_else(denied)?;
        Ok(PinnedLaunchInput {
            pin_id: id,
            kind: LaunchInputKind::Pack,
            object_id: id,
            object_version: row.provider_version.clone().ok_or_else(denied)?,
            size_bytes: u64::try_from(row.size_bytes).map_err(|_| denied())?,
            sha256: Sha256Digest::parse(&row.sha256)?,
            max_expanded_bytes: u64::try_from(row.expanded_bytes).map_err(|_| denied())?,
            max_files: u64::try_from(row.entries).map_err(|_| denied())?,
        })
    }
    pub async fn prepared_pack_input(
        pool: &PgPool,
        admission: i64,
    ) -> Result<Option<PinnedLaunchInput>> {
        let mut tx = pool.begin().await?;
        let (root, epochs, release) = Self::input_root(&mut tx, admission).await?;
        let row:Option<InputRecord>=sqlx::query_as(&format!("SELECT {INPUT_COLUMNS} FROM workload_input_materialization WHERE admission=$1 AND kind='pack' FOR UPDATE"))
            .bind(admission).fetch_optional(&mut *tx).await?;
        let pin = match row {
            None => None,
            Some(row) => {
                if row.source_target.0 != root.source_target.0
                    || row.source_epochs.0 != epochs
                    || row.release_sha256 != release.sha256.as_str()
                    || row.cleanup_required
                    || row.state == "cancelled"
                {
                    return Err(denied());
                }
                if row.state == "published" {
                    Some(Self::input_pin(&row)?)
                } else {
                    None
                }
            }
        };
        tx.commit().await?;
        Ok(pin)
    }
    /// A successful provider response is not publication authority. Sources and
    /// permanent epochs are checked again before inserting immutable input pins.
    pub async fn publish_pack_input(
        pool: &PgPool,
        permit: &PackInputReservation,
        object: &StoredObject,
    ) -> Result<PinnedLaunchInput> {
        let mut tx = pool.begin().await?;
        let (root, epochs, release) = Self::input_root(&mut tx, permit.admission).await?;
        let mut row = Self::input_record(&mut tx, permit.id).await?;
        let hash: String = object
            .sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if row.admission != root.id
            || row.source_epochs.0 != epochs
            || row.source_target.0 != root.source_target.0
            || row.release_sha256 != release.sha256.as_str()
            || row.cleanup_required
            || row.state == "cancelled"
            || row.object_key != permit.key
            || row.object_key != object.key.as_str()
            || row.size_bytes as u64 != object.size
            || row.size_bytes as u64 != permit.size
            || row.sha256 != hash
            || row.sha256 != permit.sha256.as_str()
        {
            return Err(denied());
        }
        if row.state == "published" {
            if row.provider_version.as_deref() != Some(object.provider_version.as_stored()) {
                return Err(denied());
            }
        } else {
            let id:i64=sqlx::query_scalar("INSERT INTO workload_launch_object(source_target,kind,object_key,provider_version,size_bytes,sha256,max_expanded_bytes,max_files) VALUES($1,'pack',$2,$3,$4,$5,$6,$7) RETURNING id")
                .bind(&row.source_target).bind(&row.object_key).bind(object.provider_version.as_stored()).bind(row.size_bytes).bind(&row.sha256).bind(row.expanded_bytes).bind(row.entries).fetch_one(&mut *tx).await?;
            sqlx::query("UPDATE workload_input_materialization SET state='published',object_id=$2,provider_version=$3 WHERE id=$1 AND state='reserved'")
                .bind(row.id).bind(id).bind(object.provider_version.as_stored()).execute(&mut *tx).await?;
            row.state = "published".into();
            row.object_id = Some(id);
            row.provider_version = Some(object.provider_version.as_stored().into());
        }
        if root.expires_at <= Self::now(&mut tx).await? {
            return Err(denied());
        }
        let pin = Self::input_pin(&row)?;
        tx.commit().await?;
        Ok(pin)
    }
    async fn input_page(
        pool: &PgPool,
        cleanup: bool,
        limit: i64,
    ) -> Result<Vec<(i64, i64, String, Uuid)>> {
        let mut tx = pool.begin().await?;
        let lane = if cleanup { 2i64 } else { 1i64 };
        let (after, mut high): (i64, i64) = sqlx::query_as(
            "SELECT after_id,high_water FROM workload_input_maintenance WHERE id=$1 FOR UPDATE",
        )
        .bind(lane)
        .fetch_one(&mut *tx)
        .await?;
        let predicate = if cleanup {
            "cleanup_required AND NOT cleanup_settled"
        } else {
            "NOT cleanup_required"
        };
        if high == 0 {
            high = sqlx::query_scalar(&format!(
                "SELECT COALESCE(MAX(id),0) FROM workload_input_materialization WHERE {predicate}"
            ))
            .fetch_one(&mut *tx)
            .await?;
        }
        let rows:Vec<(i64,i64,String,Uuid)>=sqlx::query_as(&format!("SELECT id,admission,object_key,cleanup_token FROM workload_input_materialization WHERE id>$1 AND id<=$2 AND {predicate} ORDER BY id LIMIT $3"))
            .bind(after).bind(high).bind(limit.clamp(1,64)).fetch_all(&mut *tx).await?;
        let (next, high) = match rows.last() {
            Some((id, ..)) if *id < high => (*id, high),
            _ => (0, 0),
        };
        sqlx::query("UPDATE workload_input_maintenance SET after_id=$2,high_water=$3 WHERE id=$1")
            .bind(lane)
            .bind(next)
            .bind(high)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(rows)
    }
    /// Bounded restart continuation. Cancel metadata before provider cleanup;
    /// old idempotency keys survive and cannot publish after authority restores.
    pub async fn reconcile_inputs(pool: &PgPool, limit: i64) -> Result<u64> {
        let rows = Self::input_page(pool, false, limit).await?;
        let mut changed = 0;
        for (id, admission, ..) in rows {
            let mut tx = pool.begin().await?;
            let fresh = match Self::input_root(&mut tx, admission).await {
                Ok((root, epochs, release)) => Some((root, epochs, release)),
                Err(Error::PermissionDenied(_)) => None,
                Err(error) => return Err(error),
            };
            let row = Self::input_record(&mut tx, id).await?;
            let valid = fresh.as_ref().is_some_and(|(root, epochs, release)| {
                row.source_target.0 == root.source_target.0
                    && row.source_epochs.0 == *epochs
                    && row.release_sha256 == release.sha256.as_str()
            });
            if valid && row.state != "cancelled" {
                tx.commit().await?;
                continue;
            }
            sqlx::query("UPDATE workload_input_materialization SET state='cancelled',cleanup_required=true WHERE id=$1")
                .bind(id).execute(&mut *tx).await?;
            if let Some(object) = row.object_id {
                sqlx::query("UPDATE workload_launch_object SET retired_at=COALESCE(retired_at,clock_timestamp()) WHERE id=$1")
                    .bind(object).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            changed += 1;
        }
        Ok(changed)
    }
    pub async fn input_cleanup_candidates(
        pool: &PgPool,
        limit: i64,
    ) -> Result<Vec<PublicInputCleanup>> {
        Ok(Self::input_page(pool, true, limit)
            .await?
            .into_iter()
            .map(|(id, _, key, token)| PublicInputCleanup { id, key, token })
            .collect())
    }
    pub async fn acknowledge_input_cleanup(
        pool: &PgPool,
        plan: &PublicInputCleanup,
        settlement: &FilePutSettlement,
    ) -> Result<()> {
        if settlement.key().as_str() != plan.key || settlement.token() != plan.token {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        let row = Self::input_record(&mut tx, plan.id).await?;
        if row.object_key != plan.key
            || row.cleanup_token != plan.token
            || !row.cleanup_required
            || row.state != "cancelled"
        {
            return Err(denied());
        }
        if !row.cleanup_settled {
            sqlx::query("UPDATE workload_input_materialization SET cleanup_settled=true,cleanup_provider_version=$2 WHERE id=$1")
                .bind(row.id).bind(settlement.provider().as_stored()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn input_reconcile_pending(pool: &PgPool) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT after_id<high_water FROM workload_input_maintenance WHERE id=1",
        )
        .fetch_one(pool)
        .await?)
    }
    pub async fn input_maintenance_pending(pool: &PgPool) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workload_input_maintenance WHERE after_id<high_water) OR EXISTS(SELECT 1 FROM workload_input_materialization WHERE cleanup_required AND NOT cleanup_settled)").fetch_one(pool).await?)
    }
}
