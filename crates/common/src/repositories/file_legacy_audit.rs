//! Explicit operator/native audit of otherwise unprovable historical PUTs.
//! Native adapters are trusted storage-controller code, NOT caller JSON or SDK
//! process output. Missing stop/drain/ownership/version capabilities stay manual.
use super::file_evidence_retention::FileEvidenceRetentionRepository as Evidence;
use crate::{delegation::DelegationAuthority, value_schema::digest, Error, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const PAGE: usize = 128;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LegacyAuditScope {
    /// Non-secret configured backend identity, including bucket/root and the
    /// GLOBAL legacy staging namespace. A tenant's final-key prefix is NOT it.
    pub backend: String,
    pub staging_prefix: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyAuditMember {
    pub operation: Uuid,
    pub version: i64,
    pub root_read: i64,
    pub object_key: Option<String>,
    pub provider_version: Option<String>,
    pub staging_path: Option<String>,
    pub execution_attempt: Option<Uuid>,
    pub worker_instance: Option<Uuid>,
    pub native_instance: Option<Uuid>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyAuditPlan {
    pub id: Uuid,
    pub scope: LegacyAuditScope,
    pub members: Vec<LegacyAuditMember>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LegacyItemKind {
    FinalObject,
    RemoteStaging,
    Multipart,
    LocalStaging,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    Owned,
    Unowned,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyAuditItem {
    pub kind: LegacyItemKind,
    pub key: String,
    /// Exact object version/generation, multipart upload ID or native inode
    /// identity. Never a user ref, current HEAD, credential or age cutoff.
    pub provider_identity: String,
    pub ownership: Ownership,
    pub ownership_proof: Value,
}
/// Trusted native output. A management adapter may deserialize it only AFTER
/// authenticating its controller attestation, never from an operator request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeWriterFence {
    pub scope: LegacyAuditScope,
    pub plan_digest: String,
    pub stopped_native_instances: Vec<Uuid>,
    pub stopped_worker_instances: Vec<Uuid>,
    pub stopped_execution_attempts: Vec<Uuid>,
    /// Verified controller receipts for non-restartable old writers, ALL
    /// accepted remote requests settled, and ownership of the enumerated stage
    /// namespace. A timestamp, process-exit claim or empty HEAD is insufficient.
    pub restart_fence: Value,
    pub provider_request_settlement: Value,
    pub namespace_ownership: Value,
}
#[derive(Serialize, Deserialize)]
pub struct LegacyAuditPage {
    pub items: Vec<LegacyAuditItem>,
    pub next_cursor: Option<String>,
    /// Native proof that ALL historical versions, delete markers, multipart
    /// sessions and exact local paths in the plan were enumerated under fence.
    /// None never means complete merely because a page happened to be empty.
    pub complete: Option<NativeCatalogueCompletion>,
}
#[derive(Serialize, Deserialize)]
pub struct NativeCatalogueCompletion {
    pub plan_digest: String,
    pub fence_digest: String,
    pub proof: Value,
}
#[derive(Serialize, Deserialize)]
pub struct ExactLegacyAbsence {
    pub item_digest: String,
    pub fence_digest: String,
    pub proof: Value,
}

/// Implement ONLY in an operator-owned native storage/process controller. The
/// adapter must verify signed/native stop, drain and ownership observations.
/// Ordinary BlobStore listing cannot establish all-version/multipart coverage.
#[async_trait]
pub trait NativeLegacyAuditor: Send + Sync {
    fn scope(&self) -> LegacyAuditScope;
    async fn stop_and_settle_writers(&self, plan: &LegacyAuditPlan) -> Result<NativeWriterFence>;
    /// Revalidate the persistent fence on restart. Never trust a JSON echo.
    async fn resume_fence(
        &self,
        plan: &LegacyAuditPlan,
        receipt: &Value,
    ) -> Result<NativeWriterFence>;
    async fn catalogue(
        &self,
        plan: &LegacyAuditPlan,
        fence: &NativeWriterFence,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<LegacyAuditPage>;
    async fn delete_and_confirm_exact(
        &self,
        plan: &LegacyAuditPlan,
        fence: &NativeWriterFence,
        item: &LegacyAuditItem,
    ) -> Result<ExactLegacyAbsence>;
}
fn invalid() -> Error {
    Error::PermissionDenied(
        "Legacy audit proof/scope is incomplete or does not match exact owned identities".into(),
    )
}
fn hashed(value: &impl Serialize) -> Result<String> {
    digest(&serde_json::to_value(value)?).map_err(|_| invalid())
}
fn bounded(value: &Value) -> Result<()> {
    if serde_json::to_vec(value)?.len() > 1024 * 1024 {
        Err(invalid())
    } else {
        Ok(())
    }
}
fn validate_fence(plan: &LegacyAuditPlan, fence: &NativeWriterFence) -> Result<()> {
    if fence.scope != plan.scope
        || fence.plan_digest != hashed(plan)?
        || fence.restart_fence.is_null()
        || fence.provider_request_settlement.is_null()
        || fence.namespace_ownership.is_null()
        || fence.stopped_native_instances.len() > 256
        || fence.stopped_worker_instances.len() > 256
        || fence.stopped_execution_attempts.len() > 256
    {
        return Err(invalid());
    }
    for m in &plan.members {
        if m.native_instance
            .is_some_and(|v| !fence.stopped_native_instances.contains(&v))
            || m.worker_instance
                .is_some_and(|v| !fence.stopped_worker_instances.contains(&v))
            || m.execution_attempt
                .is_some_and(|v| !fence.stopped_execution_attempts.contains(&v))
        {
            return Err(invalid());
        }
    }
    bounded(&serde_json::to_value(fence)?)
}
fn validate_item(plan: &LegacyAuditPlan, item: &LegacyAuditItem) -> Result<()> {
    if item.key.is_empty() || item.provider_identity.is_empty() || item.ownership_proof.is_null() {
        return Err(invalid());
    }
    crate::artifact_transport::ValidatedRelativePath::new(&item.key)?;
    let located = match item.kind {
        LegacyItemKind::FinalObject => plan
            .members
            .iter()
            .any(|m| m.object_key.as_deref() == Some(item.key.as_str())),
        LegacyItemKind::LocalStaging => plan
            .members
            .iter()
            .any(|m| m.staging_path.as_deref() == Some(item.key.as_str())),
        LegacyItemKind::RemoteStaging | LegacyItemKind::Multipart => item
            .key
            .strip_prefix(&plan.scope.staging_prefix)
            .is_some_and(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit())),
    };
    if !located {
        return Err(invalid());
    }
    bounded(&serde_json::to_value(item)?)
}

pub struct LegacyFileAuditRepository;

#[derive(Serialize, sqlx::FromRow)]
pub struct LegacyAuditStatus {
    pub id: Uuid,
    pub operator: i64,
    pub state: String,
    pub cursor: Option<String>,
    pub diagnostic: Option<String>,
    pub plan: Value,
    pub catalogued: i64,
    pub confirmed_absent: i64,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct LegacyAuditCandidate {
    pub version: i64,
    pub operation: Uuid,
    pub metadata_gone: bool,
    pub diagnostic: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct LegacyAuditReceipt {
    pub kind: String,
    pub identity: String,
    pub proof: Value,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct FileEvidenceAuditStatus {
    pub version: i64,
    pub evidence_state: String,
    pub diagnostic: Option<String>,
    pub metadata_gone: bool,
    pub object_done: bool,
    pub staging_done: bool,
    pub unresolved_operations: i64,
}

impl LegacyFileAuditRepository {
    pub async fn evidence_status(pool: &PgPool, version: i64) -> Result<FileEvidenceAuditStatus> {
        Ok(sqlx::query_as("SELECT o.version,o.evidence_state,o.diagnostic,o.metadata_gone,o.object_done,o.staging_done,(SELECT count(*) FROM file_evidence_put WHERE owner=o.version) AS unresolved_operations FROM file_evidence_owner o WHERE o.version=$1")
            .bind(version).fetch_one(pool).await?)
    }
    /// Safe operator choice when proof or a provider capability is unavailable.
    /// This records a decision, NOT a native fact, and releases no operation.
    pub async fn quarantine(
        pool: &PgPool,
        id: Uuid,
        claims: &crate::auth::jwt::Claims,
    ) -> Result<()> {
        let operator = Self::operator_id(claims)?;
        let mut tx = pool.begin().await?;
        Evidence::lock_reclamation(&mut tx).await?;
        if !DelegationAuthority::load_for_share(&mut tx, operator)
            .await?
            .manages_permissions()
        {
            return Err(invalid());
        }
        let state: String =
            sqlx::query_scalar("SELECT state FROM file_legacy_audit WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        if state == "completed" {
            return Err(invalid());
        }
        let decision = serde_json::json!({"operator":operator,"decision":"retain_quarantine","at":chrono::Utc::now(),"native_settlement_certified":false});
        Self::evidence(
            &mut tx,
            id,
            "operator_decision",
            &hashed(&decision)?,
            &decision,
        )
        .await?;
        sqlx::query("UPDATE file_legacy_audit SET state='manual_required',diagnostic='Operator retained quarantine. Supply verified native writer/provider management proof and request a new audit, or choose a separately authorized export/isolation migration. No operations or pins were released.' WHERE id=$1 AND state<>'manual_required'").bind(id).execute(&mut *tx).await?;
        Self::operator_id(claims)?;
        tx.commit().await?;
        Ok(())
    }
    /// Used by the existing authenticated maintenance API on every operation.
    pub async fn authorize_operator(
        pool: &PgPool,
        claims: &crate::auth::jwt::Claims,
    ) -> Result<()> {
        let operator = Self::operator_id(claims)?;
        let mut tx = pool.begin().await?;
        Evidence::lock_reclamation(&mut tx).await?;
        if !DelegationAuthority::load_for_share(&mut tx, operator)
            .await?
            .manages_permissions()
        {
            return Err(invalid());
        }
        Self::operator_id(claims)?;
        tx.commit().await?;
        Ok(())
    }

    fn operator_id(claims: &crate::auth::jwt::Claims) -> Result<i64> {
        use crate::auth::jwt::TokenType;
        if claims.token_type != TokenType::Access
            || claims.scope.is_some()
            || claims.exp <= chrono::Utc::now().timestamp()
            || claims
                .admitted_execution_credential()
                .map_err(|_| invalid())?
                .is_some()
        {
            return Err(invalid());
        }
        claims
            .sub
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(invalid)
    }

    pub async fn candidates(
        pool: &PgPool,
        after: i64,
        operation: Uuid,
    ) -> Result<Vec<LegacyAuditCandidate>> {
        Ok(sqlx::query_as("SELECT p.owner AS version,p.attempt AS operation,o.metadata_gone,d.diagnostic FROM file_evidence_put p JOIN file_evidence_owner o ON o.version=p.owner JOIN file_evidence_put_diagnostic d ON d.attempt=p.attempt WHERE p.protocol IN ('legacy_unknown','controlled_native_v1') AND (p.owner,p.attempt)>($1,$2) ORDER BY p.owner,p.attempt LIMIT 128")
            .bind(after).bind(operation).fetch_all(pool).await?)
    }

    pub async fn status(pool: &PgPool, id: Uuid) -> Result<LegacyAuditStatus> {
        Ok(sqlx::query_as("SELECT a.id,a.operator,a.state,a.cursor,a.diagnostic,a.plan,(SELECT count(*) FROM file_legacy_audit_item WHERE audit=a.id) AS catalogued,(SELECT count(*) FROM file_legacy_audit_evidence WHERE audit=a.id AND kind='absence') AS confirmed_absent FROM file_legacy_audit a WHERE a.id=$1")
            .bind(id).fetch_one(pool).await?)
    }

    pub async fn receipts(
        pool: &PgPool,
        id: Uuid,
        kind: &str,
        identity: &str,
    ) -> Result<Vec<LegacyAuditReceipt>> {
        if kind.len() > 64 || identity.len() > 256 {
            return Err(invalid());
        }
        Ok(sqlx::query_as("SELECT kind,identity,proof FROM file_legacy_audit_evidence WHERE audit=$1 AND (kind,identity)>($2,$3) ORDER BY kind,identity LIMIT 4")
            .bind(id).bind(kind).bind(identity).fetch_all(pool).await?)
    }

    /// Persist the verified, non-credential management attestation so audit
    /// readers can independently verify the controller's exact observation.
    pub async fn controller_observation(pool: &PgPool, id: Uuid, proof: &Value) -> Result<()> {
        let mut tx = pool.begin().await?;
        Evidence::lock_reclamation(&mut tx).await?;
        Self::evidence(&mut tx, id, "controller", &hashed(proof)?, proof).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Explicit operator call, not a process/SDK permission or automatic janitor.
    /// The authenticated maintenance host supplies verified operator claims and
    /// registered native adapter. No provider/stop receipt comes from the caller.
    pub async fn request(
        pool: &PgPool,
        operator_claims: &crate::auth::jwt::Claims,
        versions: &[i64],
        native: &dyn NativeLegacyAuditor,
    ) -> Result<Uuid> {
        let operator = Self::operator_id(operator_claims)?;
        if versions.is_empty() || versions.len() > 256 {
            return Err(invalid());
        }
        let scope = native.scope();
        if scope.backend.is_empty() || scope.staging_prefix != ".attune-upload/" {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        Evidence::lock_reclamation(&mut tx).await?;
        let operator_proof = DelegationAuthority::load_for_share(&mut tx, operator).await?;
        if !operator_proof.manages_permissions() {
            return Err(invalid());
        }
        let rows=sqlx::query("SELECT p.attempt,p.owner,o.root_read,o.object_key,o.provider_version,o.staging_path,COALESCE(p.execution_attempt,(o.reservation->>'attempt')::uuid) AS execution_attempt,p.worker_instance,p.native_instance FROM file_evidence_put p JOIN file_evidence_owner o ON o.version=p.owner WHERE p.owner=ANY($1) AND p.protocol IN ('legacy_unknown','controlled_native_v1') AND o.metadata_gone AND NOT EXISTS(SELECT 1 FROM file_legacy_audit_member m JOIN file_legacy_audit a ON a.id=m.audit WHERE m.operation=p.attempt AND a.state<>'manual_required') ORDER BY p.owner,p.attempt LIMIT 257 FOR UPDATE OF o,p")
            .bind(versions).fetch_all(&mut *tx).await?;
        if rows.is_empty() || rows.len() > 256 {
            return Err(invalid());
        }
        let members = rows
            .iter()
            .map(|r| {
                Ok(LegacyAuditMember {
                    operation: r.try_get("attempt")?,
                    version: r.try_get("owner")?,
                    root_read: r.try_get("root_read")?,
                    object_key: r.try_get("object_key")?,
                    provider_version: r.try_get("provider_version")?,
                    staging_path: r.try_get("staging_path")?,
                    execution_attempt: r.try_get("execution_attempt")?,
                    worker_instance: r.try_get("worker_instance")?,
                    native_instance: r.try_get("native_instance")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let id = Uuid::new_v4();
        let plan = LegacyAuditPlan { id, scope, members };
        let body = serde_json::to_value(&plan)?;
        bounded(&body)?;
        sqlx::query(
            "INSERT INTO file_legacy_audit(id,operator,plan,plan_digest) VALUES($1,$2,$3,$4)",
        )
        .bind(id)
        .bind(operator)
        .bind(body)
        .bind(hashed(&plan)?)
        .execute(&mut *tx)
        .await?;
        for member in &plan.members {
            let original:Option<(Value,Value)>=sqlx::query_as("SELECT n.member,g.scope FROM file_native_operation n JOIN file_native_generation g ON g.generation=n.generation WHERE n.operation=$1")
                .bind(member.operation).fetch_optional(&mut *tx).await?;
            if let Some((original, scope)) = original {
                if original != serde_json::to_value(member)?
                    || scope != serde_json::to_value(&plan.scope)?
                {
                    return Err(invalid());
                }
            }
            sqlx::query(
                "INSERT INTO file_legacy_audit_member(audit,operation,version) VALUES($1,$2,$3)",
            )
            .bind(id)
            .bind(member.operation)
            .bind(member.version)
            .execute(&mut *tx)
            .await?;
        }
        if operator_claims.exp <= chrono::Utc::now().timestamp() {
            return Err(invalid());
        }
        tx.commit().await?;
        Ok(id)
    }
    /// One bounded catalogue or cleanup page. Native I/O is outside source/target
    /// transactions; immutable fences and graph-first DB commits serialize it.
    pub async fn step(pool: &PgPool, id: Uuid, native: &dyn NativeLegacyAuditor) -> Result<String> {
        let (body, state, cursor): (Value, String, Option<String>) =
            sqlx::query_as("SELECT plan,state,cursor FROM file_legacy_audit WHERE id=$1")
                .bind(id)
                .fetch_one(pool)
                .await?;
        let plan: LegacyAuditPlan = serde_json::from_value(body)?;
        if state == "completed" || state == "manual_required" {
            return Ok(state);
        }
        if native.scope() != plan.scope {
            return Err(invalid());
        }
        let stored:Option<Value>=sqlx::query_scalar("SELECT proof FROM file_legacy_audit_evidence WHERE audit=$1 AND kind='writers' AND identity=$2").bind(id).bind(hashed(&plan)?).fetch_optional(pool).await?;
        let fence = match &stored {
            Some(proof) => native.resume_fence(&plan, proof).await,
            None => native.stop_and_settle_writers(&plan).await,
        };
        let fence = match fence {
            Ok(f) => f,
            Err(_) => {
                Self::manual(pool, id).await?;
                return Ok("manual_required".into());
            }
        };
        validate_fence(&plan, &fence)?;
        let receipt = serde_json::to_value(&fence)?;
        if stored.as_ref().is_some_and(|old| old != &receipt) {
            return Err(invalid());
        }
        let mut tx = pool.begin().await?;
        Evidence::lock_reclamation(&mut tx).await?;
        Self::evidence(&mut tx, id, "writers", &hashed(&plan)?, &receipt).await?;
        tx.commit().await?;
        if state == "requested" || state == "cataloguing" {
            let cursor_identity = hashed(&cursor)?;
            let visited:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM file_legacy_audit_evidence WHERE audit=$1 AND kind='cursor' AND identity=$2)").bind(id).bind(&cursor_identity).fetch_one(pool).await?;
            if visited {
                Self::manual(pool, id).await?;
                return Ok("manual_required".into());
            }
            let page = match native
                .catalogue(&plan, &fence, cursor.as_deref(), PAGE)
                .await
            {
                Ok(p) => p,
                Err(_) => {
                    Self::manual(pool, id).await?;
                    return Ok("manual_required".into());
                }
            };
            if page.items.len() > PAGE
                || page
                    .next_cursor
                    .as_ref()
                    .is_some_and(|v| v.len() > 16384 || Some(v) == cursor.as_ref())
                || (page.next_cursor.is_none() && page.complete.is_none())
                || (page.next_cursor.is_some() && page.complete.is_some())
            {
                return Err(invalid());
            }
            bounded(&serde_json::to_value(&page.items)?)?;
            let page_receipt = serde_json::json!({"cursor":&cursor,"next":&page.next_cursor,"items":page.items.iter().map(hashed).collect::<Result<Vec<_>>>()?,"terminal":page.complete.is_some()});
            let mut tx = pool.begin().await?;
            Evidence::lock_reclamation(&mut tx).await?;
            let current: (String, Option<String>) =
                sqlx::query_as("SELECT state,cursor FROM file_legacy_audit WHERE id=$1 FOR UPDATE")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?;
            if current.0 != state || current.1 != cursor {
                return Err(invalid());
            }
            Self::evidence(&mut tx, id, "cursor", &cursor_identity, &page_receipt).await?;
            for item in page.items {
                validate_item(&plan, &item)?;
                let body = serde_json::to_value(&item)?;
                sqlx::query("INSERT INTO file_legacy_audit_item(audit,identity,item,ownership) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(id).bind(hashed(&item)?).bind(body).bind(match item.ownership{Ownership::Owned=>"owned",Ownership::Unowned=>"unowned",Ownership::Unknown=>"unknown"}).execute(&mut *tx).await?;
            }
            let next = if let Some(end) = page.complete {
                if end.plan_digest != hashed(&plan)?
                    || end.fence_digest != hashed(&fence)?
                    || end.proof.is_null()
                {
                    return Err(invalid());
                }
                Self::evidence(
                    &mut tx,
                    id,
                    "catalogue",
                    &hashed(&plan)?,
                    &serde_json::to_value(end)?,
                )
                .await?;
                "cleaning"
            } else {
                "cataloguing"
            };
            sqlx::query("UPDATE file_legacy_audit SET state=$2,cursor=$3 WHERE id=$1")
                .bind(id)
                .bind(next)
                .bind(page.next_cursor)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(next.into());
        }
        let unknown:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM file_legacy_audit_item WHERE audit=$1 AND ownership='unknown')").bind(id).fetch_one(pool).await?;
        if unknown {
            Self::manual(pool, id).await?;
            return Ok("manual_required".into());
        }
        let items:Vec<(String,Value)>=sqlx::query_as("SELECT i.identity,i.item FROM file_legacy_audit_item i WHERE audit=$1 AND ownership='owned' AND NOT EXISTS(SELECT 1 FROM file_legacy_audit_evidence e WHERE e.audit=i.audit AND e.kind='absence' AND e.identity=i.identity) ORDER BY identity LIMIT 16").bind(id).fetch_all(pool).await?;
        for (identity, body) in &items {
            let item: LegacyAuditItem = serde_json::from_value(body.clone())?;
            validate_item(&plan, &item)?;
            let absent = native
                .delete_and_confirm_exact(&plan, &fence, &item)
                .await?;
            if absent.item_digest != *identity
                || absent.fence_digest != hashed(&fence)?
                || absent.proof.is_null()
            {
                return Err(invalid());
            }
            bounded(&absent.proof)?;
            let mut tx = pool.begin().await?;
            Evidence::lock_reclamation(&mut tx).await?;
            Self::evidence(&mut tx, id, "absence", identity, &absent.proof).await?;
            tx.commit().await?;
        }
        if !items.is_empty() {
            return Ok("cleaning".into());
        }
        let mut tx = pool.begin().await?;
        Evidence::lock_reclamation(&mut tx).await?;
        let ready:bool=sqlx::query_scalar("SELECT state='cleaning' AND EXISTS(SELECT 1 FROM file_legacy_audit_evidence WHERE audit=$1 AND kind='catalogue') AND NOT EXISTS(SELECT 1 FROM file_legacy_audit_item i WHERE audit=$1 AND (ownership='unknown' OR (ownership='owned' AND NOT EXISTS(SELECT 1 FROM file_legacy_audit_evidence e WHERE e.audit=i.audit AND e.kind='absence' AND e.identity=i.identity)))) FROM file_legacy_audit WHERE id=$1 FOR UPDATE")
            .bind(id).fetch_one(&mut *tx).await?;
        if !ready {
            return Err(invalid());
        }
        Self::evidence(&mut tx,id,"settled",&hashed(&plan)?,&serde_json::json!({"plan":hashed(&plan)?,"writers":hashed(&fence)?,"all_catalogued_owned_identities_confirmed_absent":true})).await?;
        sqlx::query("UPDATE file_legacy_audit SET state='completed' WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for m in &plan.members {
            sqlx::query("DELETE FROM file_evidence_put WHERE attempt=$1 AND owner=$2 AND protocol IN ('legacy_unknown','controlled_native_v1')").bind(m.operation).bind(m.version).execute(&mut *tx).await?;
            sqlx::query("UPDATE file_evidence_owner SET object_done=true,staging_done=true WHERE version=$1 AND metadata_gone AND NOT EXISTS(SELECT 1 FROM file_evidence_put WHERE owner=$1)").bind(m.version).execute(&mut *tx).await?;
            sqlx::query("SELECT file_evidence_release($1)")
                .bind(m.version)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok("completed".into())
    }
    async fn evidence(
        conn: &mut sqlx::PgConnection,
        id: Uuid,
        kind: &str,
        identity: &str,
        proof: &Value,
    ) -> Result<()> {
        bounded(proof)?;
        let prior:Option<Value>=sqlx::query_scalar("SELECT proof FROM file_legacy_audit_evidence WHERE audit=$1 AND kind=$2 AND identity=$3").bind(id).bind(kind).bind(identity).fetch_optional(&mut *conn).await?;
        if let Some(prior) = prior {
            if prior != *proof {
                return Err(invalid());
            }
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO file_legacy_audit_evidence(audit,kind,identity,proof) VALUES($1,$2,$3,$4)",
        )
        .bind(id)
        .bind(kind)
        .bind(identity)
        .bind(proof)
        .execute(conn)
        .await?;
        Ok(())
    }
    async fn manual(pool: &PgPool, id: Uuid) -> Result<()> {
        let mut tx = pool.begin().await?;
        Evidence::lock_reclamation(&mut tx).await?;
        sqlx::query("UPDATE file_legacy_audit SET state='manual_required',diagnostic='Native stopped-writer/request-settlement, all-version/multipart ownership or exact absence proof unavailable. Keep quarantined, supply a registered scope-capable native auditor, or choose a separately authorized export/isolation migration. Never reset rows or release by age/HEAD.' WHERE id=$1 AND state NOT IN ('completed','manual_required')").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
