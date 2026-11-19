//! Internal, transaction-bound admission of an already delivered source value.
//! A receipt ID or equality of Key/value bytes is never a transfer capability.
use super::*;
use crate::repositories::{
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::{
        InheritedInputRepository as Inputs, InputSite, InputSitePurpose, ReceiptContext,
    },
    recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
    runtime_authority::{
        ApprovalEffectFence, RuntimeAuthorityRepository as Authority, RuntimeCredentialBinding,
        ValidatedRuntimeAuthority,
    },
};
use crate::{
    rbac::Action,
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{PreparedSecretValue, ENTITY_QUEUE_ITEM_PAYLOAD},
};

const PURPOSE: &str = "queue_contributor_transfer";

/// Issued only after the current source credential and its exact delivered
/// field have been validated for this destination. No serde/raw-ID constructor.
pub struct QueueInputTransfer {
    proof: TransferProof,
    transaction: String,
    context: ReceiptContext,
    fence: ApprovalEffectFence,
    source: LiveTransferSource,
}

pub(super) struct LiveTransferSource {
    credential: RuntimeCredentialBinding,
    digest: String,
}

/// A retained transfer relationship, not a read grant. The canonical receipt
/// traversal must consume its exact original site and all ancestor fences.
pub(in crate::repositories) struct QueueReceiptParent {
    receipt: Id,
    site: InputSite,
    field_path: String,
    fence: ApprovalEffectFence,
}
impl QueueReceiptParent {
    pub(in crate::repositories) fn receipt(&self) -> Id {
        self.receipt
    }
    pub(in crate::repositories) fn site(&self) -> &InputSite {
        &self.site
    }
    pub(in crate::repositories) fn field_path(&self) -> &str {
        &self.field_path
    }
    pub(in crate::repositories) fn source_fence(&self) -> &ApprovalEffectFence {
        &self.fence
    }
    pub(in crate::repositories) fn origin(&self) -> SecretOrigin {
        SecretOrigin::Entity {
            entity_type: self.site.kind.origin_kind(self.site.purpose),
            entity_id: self.receipt,
            path: self.site.path.clone(),
        }
    }
}
impl LiveTransferSource {
    pub(super) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        let current = Authority::validate_execution_credential(conn, &self.credential).await?;
        if hash(&serde_json::to_value(current.authority())?)? != self.digest {
            return Err(denied());
        }
        current.ensure_fresh_on(conn).await
    }
    fn copy(&self) -> Self {
        Self {
            credential: RuntimeCredentialBinding {
                execution_id: self.credential.execution_id,
                admission_id: self.credential.admission_id,
                sponsor_identity_id: self.credential.sponsor_identity_id,
                action_ref: self.credential.action_ref.clone(),
                attempt: self.credential.attempt,
            },
            digest: self.digest.clone(),
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TransferProof {
    purpose: String,
    queue: Id,
    item: Id,
    source_execution: Id,
    source_admission: Id,
    source_attempt: uuid::Uuid,
    source_digest: String,
    contributor_digests: BTreeMap<String, String>,
    receipt: Id,
    site: InputSite,
    origin: SecretOrigin,
    field: PreparedSecretValue,
}

impl QueueConfigurationInputRepository {
    /// Internal producer seam only. The submitted protected field must already
    /// contain the exact original delivered receipt's provenance. The caller
    /// retains this transaction through acceptance and checks the returned
    /// input's combined effect fence immediately before committing publication.
    pub async fn prepare_receipt_transfer(
        conn: &mut PgConnection,
        source: &ValidatedRuntimeAuthority,
        queue: &WorkQueue,
        item: &WorkQueueItem,
        field_path: &str,
        origin: &SecretOrigin,
        master: &str,
    ) -> Result<QueueInputTransfer> {
        super::super::schema_type::SchemaTypeRepository::lock_selection(conn).await?;
        let current =
            Authority::validate_execution_credential(conn, &source.credential_binding()).await?;
        if current.authority() != source.authority() || queue.is_adhoc {
            return Err(denied());
        }
        let credential = current.credential_binding();
        let (receipt, site) = Inputs::receipt_source_site(conn, origin).await?;
        let destination: Id = sqlx::query_scalar(
            "SELECT destination_admission FROM runtime_input_receipt WHERE id=$1 FOR SHARE",
        )
        .bind(receipt)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(denied)?;
        if !super::super::immutable_input_origin::is_input_receipt(origin)
            || site.execution != credential.execution_id
            || site.purpose != InputSitePurpose::Value
            || destination != credential.admission_id
        {
            return Err(denied());
        }
        let context =
            Inputs::queue_delivered_source(conn, credential.execution_id, &site, origin, master)
                .await?;
        // Finish every source/ancestor fence before destination protection.
        let target = RuntimeTargetRepository::resolve(
            conn,
            RuntimeTarget::QueueItems(queue.id),
            credential.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        if !current.allows_new_effect(&target, Action::Create) {
            return Err(denied());
        }
        let actual =
            super::super::work_queue::WorkQueueItemRepository::find_by_id_for_update(conn, item.id)
                .await?
                .ok_or_else(denied)?;
        if actual.queue != queue.id
            || actual.queue_ref != queue.r#ref
            || queue.is_adhoc
            || actual.requested_by_execution != Some(current.credential_binding().execution_id)
        {
            return Err(denied());
        }
        let field = fields(conn, actual.id)
            .await?
            .into_iter()
            .find(|field| field.json_path == field_path)
            .ok_or_else(denied)?;
        exact_field(&field, origin, context.accepted_value(), master)?;
        let source_manifests = manifests(conn, "execution", credential.execution_id).await?;
        let destination_manifests = manifests(conn, "queue_item", item.id).await?;
        if source_manifests != destination_manifests || source_manifests.is_empty() {
            return Err(denied());
        }
        if Contributors::queue_authorities(conn, credential.execution_id)
            .await?
            .is_some_and(|roots| roots.is_empty())
        {
            return Err(denied());
        }
        let contributor_digests = source_manifests
            .into_iter()
            .map(|(name, _, digest)| (name, digest))
            .collect();
        let fence = current.effect_fence().intersect(context.accepted_fence());
        context.ensure_fresh_on(conn).await?;
        fence.ensure_fresh_on(conn).await?;
        let source_digest = hash(&serde_json::to_value(current.authority())?)?;
        Ok(QueueInputTransfer {
            proof: TransferProof {
                purpose: PURPOSE.into(),
                queue: queue.id,
                item: item.id,
                source_execution: credential.execution_id,
                source_admission: credential.admission_id,
                source_attempt: credential.attempt,
                source_digest: source_digest.clone(),
                contributor_digests,
                receipt,
                site,
                origin: origin.clone(),
                field,
            },
            transaction: transaction(conn).await?,
            context,
            fence,
            source: LiveTransferSource {
                credential,
                digest: source_digest,
            },
        })
    }
}

impl QueueInputTransfer {
    pub(super) async fn accepted(
        &self,
        conn: &mut PgConnection,
        binding: &Binding,
        master: &str,
    ) -> Result<TransferProof> {
        if transaction(conn).await? != self.transaction
            || binding.item != self.proof.item
            || binding.queue != self.proof.queue
            || binding.contributor_digests != self.proof.contributor_digests
        {
            return Err(denied());
        }
        self.source.ensure_fresh_on(conn).await?;
        self.context.ensure_fresh_on(conn).await?;
        self.fence.ensure_fresh_on(conn).await?;
        self.proof.validate(conn, binding, master).await?;
        Ok(self.proof.clone())
    }
    pub(super) fn fence(&self) -> &ApprovalEffectFence {
        &self.fence
    }
    pub(super) fn live_source(&self) -> LiveTransferSource {
        self.source.copy()
    }
}

impl TransferProof {
    pub(super) fn validate_shape(&self, master: &str) -> Result<()> {
        if self.purpose != PURPOSE
            || self.queue <= 0
            || self.item <= 0
            || self.source_execution <= 0
            || self.source_admission <= 0
            || self.site.execution != self.source_execution
            || self.site.purpose != InputSitePurpose::Value
            || !matches!(&self.origin, SecretOrigin::Entity {entity_type,entity_id,path}
                if *entity_id==self.receipt && *entity_id>0 && entity_type==&self.site.kind.origin_kind(self.site.purpose) && path==&self.site.path)
        {
            return Err(denied());
        }
        let value = crypto::decrypt_json(&self.field.encrypted_value, master)?;
        exact_field(&self.field, &self.origin, &value, master)
    }
    pub(super) fn receipt(&self) -> Id {
        self.receipt
    }
    pub(super) fn path(&self) -> &str {
        &self.field.json_path
    }
    pub(super) fn field(&self) -> &PreparedSecretValue {
        &self.field
    }
    pub(super) async fn validate(
        &self,
        conn: &mut PgConnection,
        binding: &Binding,
        master: &str,
    ) -> Result<QueueReceiptParent> {
        self.validate_shape(master)?;
        if self.purpose != PURPOSE
            || self.item != binding.item
            || self.queue != binding.queue
            || self.contributor_digests != binding.contributor_digests
            || binding.source_execution != Some(self.source_execution)
            || self.receipt <= 0
            || self.site.execution != self.source_execution
            || self.site.purpose != InputSitePurpose::Value
            || !super::super::immutable_input_origin::is_input_receipt(&self.origin)
        {
            return Err(denied());
        }
        let (source, fence) =
            Authority::retained_admission_with_fence(conn, self.source_admission).await?;
        if source.owner_kind != "execution"
            || source.owner_id != self.source_execution
            || source.authority_digest != self.source_digest
        {
            return Err(denied());
        }
        // History belongs to the immutable admission and survives execution
        // cleanup. Verify issuance's actual attempt without requiring it to
        // remain the current/running attempt after durable acceptance.
        let attempt:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_authority_execution_attempt WHERE execution=$1 AND admission=$2 AND attempt=$3)")
            .bind(self.source_execution).bind(self.source_admission).bind(self.source_attempt).fetch_one(&mut *conn).await?;
        if !attempt {
            return Err(denied());
        }
        let (receipt, site) = Inputs::receipt_source_site(conn, &self.origin).await?;
        let (execution,kind,purpose,path,destination,encrypted,key_hash,published):(Id,String,String,String,Id,Value,String,bool)=sqlx::query_as(
            "SELECT r.execution,r.entity_kind,r.destination_purpose,r.destination_path,r.destination_admission,r.encrypted_value,r.encryption_key_hash,NOT EXISTS(SELECT 1 FROM runtime_input_receipt_staged s WHERE s.receipt=r.id AND s.publication_transaction IS NULL) FROM runtime_input_receipt r WHERE r.id=$1 FOR SHARE OF r",
        )
        .bind(receipt)
        .fetch_one(&mut *conn)
        .await?;
        if receipt != self.receipt
            || site != self.site
            || destination != self.source_admission
            || execution != self.source_execution
            || kind != site.kind.as_str()
            || purpose != "value"
            || path != site.path
            || !published
            || key_hash != crypto::hash_encryption_key(master)
        {
            return Err(denied());
        }
        // Acceptance's live attempt is retained as evidence, not demanded after
        // durable acceptance. Completion is distinct from source revocation.
        exact_field(
            &self.field,
            &self.origin,
            &crypto::decrypt_json(&encrypted, master)?,
            master,
        )?;
        fence.ensure_fresh_on(conn).await?;
        Ok(QueueReceiptParent {
            receipt,
            site,
            field_path: self.field.json_path.clone(),
            fence,
        })
    }
}

pub(super) async fn fields(conn: &mut PgConnection, item: Id) -> Result<Vec<PreparedSecretValue>> {
    let fields =
        Secrets::find_stored_by_entity(&mut *conn, ENTITY_QUEUE_ITEM_PAYLOAD, item).await?;
    if fields.len() > 256 {
        return Err(denied());
    }
    fields
        .into_iter()
        .map(|field| {
            Ok(PreparedSecretValue {
                json_path: field.json_path,
                encrypted_value: field.encrypted_value,
                encryption_key_hash: field.encryption_key_hash.ok_or_else(denied)?,
                source_kind: field.source_kind,
                source_ref: field.source_ref,
            })
        })
        .collect()
}
fn exact_field(
    field: &PreparedSecretValue,
    origin: &SecretOrigin,
    value: &Value,
    master: &str,
) -> Result<()> {
    let provenance: SecretProvenance =
        serde_json::from_str(field.source_ref.as_deref().ok_or_else(denied)?)?;
    if field.source_kind != "provenance"
        || field.encryption_key_hash != crypto::hash_encryption_key(master)
        || provenance.origins.len() > 256
        || provenance
            .origins
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != provenance.origins.len()
        || !provenance.origins.contains(origin)
        || provenance.origins.iter().any(|other| {
            super::super::immutable_input_origin::is_input_receipt(other) && other != origin
        })
        || crypto::decrypt_json(&field.encrypted_value, master)? != *value
    {
        return Err(denied());
    }
    Ok(())
}
async fn transaction(conn: &mut PgConnection) -> Result<String> {
    Ok(sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
        .fetch_one(conn)
        .await?)
}
async fn manifests(
    conn: &mut PgConnection,
    kind: &str,
    owner: Id,
) -> Result<Vec<(String, Value, String)>> {
    let sql=match kind {
        "execution"=>"SELECT contributor,authority,authority_digest FROM (SELECT ''::TEXT AS contributor,authority,authority_digest FROM recorded_execution_input_authority WHERE owner_kind='execution' AND owner_id=$1 UNION ALL SELECT item::TEXT || ':' || contributor,authority,authority_digest FROM recorded_queue_execution_input WHERE execution=$1) roots ORDER BY contributor LIMIT 257",
        "queue_item"=>"SELECT contributor,authority,authority_digest FROM (SELECT ''::TEXT AS contributor,authority,authority_digest FROM recorded_execution_input_authority WHERE owner_kind='queue_item' AND owner_id=$1 UNION ALL SELECT contributor,authority,authority_digest FROM recorded_queue_item_input WHERE item=$1) roots ORDER BY contributor LIMIT 257",
        _=>return Err(denied()),
    };
    let rows: Vec<(String, Value, String)> =
        sqlx::query_as(sql).bind(owner).fetch_all(conn).await?;
    if rows.len() > 256 {
        return Err(denied());
    }
    let mut names = std::collections::BTreeSet::new();
    for (name, body, digest) in &rows {
        if !names.insert(name) {
            return Err(denied());
        }
        Contributors::from_manifest(body, digest)?;
    }
    Ok(rows)
}
