//! Explicit managed-Queue branch of the canonical accepted batch proof.
use super::*;
use crate::repositories::{
    execution_secret_value::ExecutionSecretValueRepository,
    queue_configuration_input::QueueConfigurationInputRepository as Fixed,
    recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
    work_queue::WorkQueueItemRepository,
};
use crate::{
    models::WorkQueueItem,
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{PreparedSecretValue, StoredSecretValue},
    value_schema::ValueSchemaBinding,
};
use serde_json::json;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attestation {
    purpose: String,
    item: Id,
    original_admission: Id,
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
    encryption_key_hash: String,
}
const PURPOSE: &str = "attune.queue.accepted-input";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueuePayload {
    attestation: Attestation,
    pub(super) fixed_input: Id,
    pub(super) ordinal: usize,
    queue: Id,
    queue_ref: String,
    pack_ref: Option<String>,
    schema: ValueSchemaBinding,
    payload: Value,
    fields: Vec<PreparedSecretValue>,
    parent_receipts: Vec<Id>,
    context: Value,
    dispatch: Option<Id>,
}
impl QueuePayload {
    pub(super) async fn transfer_parents(
        &self,
        conn: &mut PgConnection,
        master: &str,
    ) -> Result<Vec<crate::repositories::queue_configuration_input::QueueReceiptParent>> {
        let (ids, parents) =
            Fixed::transfer_contexts(conn, self.fixed_input, &self.fields, master).await?;
        if ids != self.parent_receipts {
            return Err(denied());
        }
        Ok(parents)
    }
    pub(super) fn field_count(&self) -> usize {
        self.fields.len() + 1
    }
    pub(super) fn restore(&self, master: &str) -> Result<Value> {
        self.schema
            .validate(crate::value_schema::SchemaContext::FlatDocument)
            .map_err(|_| denied())?;
        for field in &self.fields {
            let provenance: SecretProvenance =
                serde_json::from_str(field.source_ref.as_deref().ok_or_else(denied)?)?;
            if field.source_kind != "provenance"
                || provenance.origins.is_empty()
                || provenance.origins.iter().any(|origin| match origin {
                    SecretOrigin::Key(_) => false,
                    SecretOrigin::Local { source_kind, .. } => {
                        source_kind != "parameter_schema" && source_kind != "queue_item"
                    }
                    other => !crate::repositories::immutable_input_origin::is_input_receipt(other),
                })
            {
                return Err(denied());
            }
        }
        let fields = self
            .fields
            .iter()
            .map(|field| StoredSecretValue {
                json_path: field.json_path.clone(),
                encrypted_value: field.encrypted_value.clone(),
                encryption_key_hash: Some(field.encryption_key_hash.clone()),
                source_kind: field.source_kind.clone(),
                source_ref: field.source_ref.clone(),
            })
            .collect::<Vec<_>>();
        let payload =
            crate::secret_values::restore_secret_values(self.payload.clone(), &fields, master)?;
        crate::value_schema::CompiledContract::new(&self.schema.contract)
            .map_err(|_| denied())?
            .validate(&payload)
            .map_err(|_| denied())?;
        Ok(payload)
    }
}

pub(super) async fn capture(
    conn: &mut PgConnection,
    destination: Id,
    queue: &WorkQueue,
    items: &[WorkQueueItem],
    dispatch: Option<Id>,
    master: &str,
) -> Result<ApprovalEffectFence> {
    super::super::schema_type::SchemaTypeRepository::lock_selection(conn).await?;
    let record:RuntimeAdmissionRecord = sqlx::query_as("SELECT id,owner_kind,owner_id,policy_revision,authority,authority_digest,revoked_at FROM runtime_authority_admission WHERE id=$1")
        .bind(destination).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
    let ids = canonical_ids(&items.iter().map(|item| item.id).collect::<Vec<_>>())?;
    if queue.is_adhoc
        || record.owner_kind != "execution"
        || record.revoked_at.is_some()
        || !matches!(&record.authority.binding,InvocationBinding::Queue {queue_ref,..} if queue_ref==&queue.r#ref)
        || !matches!(&record.authority.attribution,RequestAttribution::QueueBatch {queue_ref,items} if queue_ref==&queue.r#ref && items.keys().copied().collect::<Vec<_>>()==ids)
        || hash(&serde_json::to_value(&record.authority.0)?)? != record.authority_digest
    {
        return Err(denied());
    }
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM runtime_accepted_batch_manifest WHERE admission=$1)",
    )
    .bind(destination)
    .fetch_one(&mut *conn)
    .await?
    {
        return RuntimeAuthorityRepository::retained_admission_fence(conn, destination).await;
    }
    let original:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_authority_admission a JOIN execution e ON a.owner_kind='execution' AND e.id=a.owner_id JOIN execution_authority_selection s ON s.execution=e.id AND s.admission=a.id WHERE a.id=$1 AND s.mode='recorded' AND NOT s.explicit AND e.initial_input_transaction=pg_current_xact_id() AND s.selection_transaction=pg_current_xact_id())")
        .bind(destination).fetch_one(&mut *conn).await?;
    if !original {
        return Err(denied());
    }
    let mut fence =
        RuntimeAuthorityRepository::selected_effect_fence(conn, &record.authority).await?;
    let snapshot: Value =
        sqlx::query_scalar("SELECT executable_snapshot FROM execution WHERE id=$1 FOR SHARE")
            .bind(record.owner_id)
            .fetch_one(&mut *conn)
            .await?;
    let snapshot: ExecutionExecutableSnapshot = serde_json::from_value(snapshot)?;
    let mut proofs = Vec::new();
    let mut expected = Vec::new();
    let mut bytes = 0usize;
    for (ordinal, item) in items.iter().enumerate() {
        let raw = WorkQueueItemRepository::find_by_id_for_update(conn, item.id)
            .await?
            .ok_or_else(denied)?;
        if raw.queue != queue.id
            || raw.queue_ref != queue.r#ref
            || raw.status != crate::models::WorkQueueItemStatus::Leased
            || raw.lease_token.is_none()
            || raw.lease_token != item.lease_token
            || raw
                .lease_expires_at
                .is_none_or(|deadline| deadline <= chrono::Utc::now())
            || raw.leased_execution != Some(record.owner_id)
        {
            return Err(denied());
        }
        let (policy,authority,expected_hash):(Id,Value,String) = sqlx::query_as("SELECT policy_revision,authority,authority_digest FROM work_queue_item_runtime_admission WHERE item=$1 FOR SHARE")
            .bind(item.id).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        if hash(&authority)? != expected_hash {
            return Err(denied());
        }
        let authority: SelectedRuntimeAuthority = serde_json::from_value(authority)?;
        if authority.policy_id != policy {
            return Err(denied());
        }
        let fixed = Fixed::load(conn, queue, &raw, &snapshot, master).await?;
        let contributors:Vec<(String,Value,String)> = sqlx::query_as("SELECT contributor,authority,authority_digest FROM (SELECT ''::TEXT AS contributor,authority,authority_digest FROM recorded_execution_input_authority WHERE owner_kind='queue_item' AND owner_id=$1 UNION ALL SELECT contributor,authority,authority_digest FROM recorded_queue_item_input WHERE item=$1) roots ORDER BY contributor")
            .bind(item.id).fetch_all(&mut *conn).await?;
        if contributors.is_empty() {
            return Err(denied());
        }
        for (name, body, digest) in &contributors {
            Contributors::from_manifest(body, digest)?;
            expected.push((item.id, name.clone(), body.clone(), digest.clone()));
        }
        let fields = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            crate::secret_values::ENTITY_QUEUE_ITEM_PAYLOAD,
            item.id,
        )
        .await?
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
        .collect::<Result<Vec<_>>>()?;
        let payload = QueuePayload {
            attestation: Attestation {
                purpose: PURPOSE.into(),
                item: item.id,
                original_admission: record.id,
                nonce: [0; 12],
                ciphertext: Vec::new(),
                encryption_key_hash: crate::crypto::hash_encryption_key(master),
            },
            fixed_input: fixed.id(),
            ordinal,
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            pack_ref: queue.pack_ref.clone(),
            schema: raw.payload_binding()?,
            payload: raw.payload.clone(),
            fields,
            parent_receipts: fixed.source_receipts().to_vec(),
            context: json!({
                "id":raw.id,"item_key":raw.item_key,"priority":raw.priority,"metadata":raw.metadata,"enqueue_source":raw.enqueue_source,
                "attempt_count":raw.attempt_count,"requested_by_identity":raw.requested_by_identity,"requested_by_execution":raw.requested_by_execution,"requested_by_enforcement":raw.requested_by_enforcement,
            }),
            dispatch,
        };
        let parents = fixed
            .validate_transfers(conn, &payload.fields, master)
            .await?;
        for parent in &parents {
            fence = fence.intersect(parent.source_fence());
        }
        payload.restore(master)?;
        let mut proof = ItemProof {
            source_admission: None,
            authority,
            executable: snapshot.clone(),
            template: serde_json::to_value(fixed.accepted_template(queue)?)?,
            schema: serde_json::to_value(fixed.schema())?,
            contributors,
            queue_payload: Some(payload),
        };
        validate_contributor_origins(conn, &proof, &parents).await?;
        seal(&mut proof, master)?;
        check_item(conn, &record, item.id, &proof, &queue.r#ref).await?;
        bytes = bytes
            .checked_add(serde_json::to_vec(&proof)?.len())
            .ok_or_else(denied)?;
        if bytes > 1024 * 1024 || expected.len() > 256 {
            return Err(denied());
        }
        proofs.push((item.id, proof));
    }
    expected.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    let copied:Vec<(Id,String,Value,String,i32)> = sqlx::query_as("SELECT item,contributor,authority,authority_digest,expected_items FROM recorded_queue_execution_input WHERE execution=$1 ORDER BY item,contributor")
        .bind(record.owner_id).fetch_all(&mut *conn).await?;
    if copied.len() != expected.len()
        || copied
            .iter()
            .zip(&expected)
            .any(|((item, name, body, digest, n), expected)| {
                (*item, name.clone(), body.clone(), digest.clone()) != *expected
                    || usize::try_from(*n).ok() != Some(copied.len())
            })
    {
        return Err(denied());
    }
    sqlx::query("INSERT INTO runtime_accepted_batch_manifest(admission,item_ids) VALUES($1,$2)")
        .bind(destination)
        .bind(&ids)
        .execute(&mut *conn)
        .await?;
    for (item, proof) in proofs {
        let fixed = proof.queue_payload.as_ref().ok_or_else(denied)?.fixed_input;
        let body = serde_json::to_value(&proof)?;
        sqlx::query("INSERT INTO runtime_accepted_batch_item(admission,item,source_admission,target_policy,proof,proof_digest,configuration_input) VALUES($1,$2,NULL,$3,$4,$5,$6)")
            .bind(destination).bind(item).bind(proof.authority.policy_id).bind(&body).bind(hash(&body)?).bind(fixed).execute(&mut *conn).await?;
    }
    RuntimeAuthorityRepository::record_admission(
        conn,
        super::super::runtime_authority::RuntimeOwner::Execution(record.owner_id),
        &record.authority,
    )
    .await?;
    fence = fence
        .intersect(&RuntimeAuthorityRepository::retained_admission_fence(conn, destination).await?);
    fence.ensure_fresh_on(conn).await?;
    Ok(fence)
}

pub(super) async fn check_item(
    conn: &mut PgConnection,
    record: &RuntimeAdmissionRecord,
    item: Id,
    proof: &ItemProof,
    queue_ref: &str,
) -> Result<()> {
    let payload = proof.queue_payload.as_ref().ok_or_else(denied)?;
    let template: WorkQueue = serde_json::from_value(proof.template.clone())?;
    let schema: super::super::schema_contract::ActionSchemaBinding =
        serde_json::from_value(proof.schema.clone())?;
    schema.validate()?;
    payload
        .schema
        .validate(crate::value_schema::SchemaContext::FlatDocument)
        .map_err(|_| denied())?;
    if proof.source_admission.is_some()
        || template.is_adhoc
        || payload.queue != template.id
        || payload.queue_ref != queue_ref
        || template.r#ref != queue_ref
        || template.pack != Some(proof.authority.owner.pack_id)
        || payload.pack_ref != template.pack_ref
        || payload.fixed_input <= 0
        || payload.context.get("id").and_then(Value::as_i64) != Some(item)
        || payload.schema.contract.source.component_ref() != queue_ref
        || !matches!(&proof.authority.attribution,RequestAttribution::QueueBatch {queue_ref:reference,items} if reference==queue_ref && items.len()==1
            && matches!(&record.authority.attribution,RequestAttribution::QueueBatch {items:all,..} if all.get(&item)==items.get(&item) && items.contains_key(&item)))
        || proof.authority.binding != record.authority.binding
        || proof.authority.owner != record.authority.owner
        || proof.authority.selected_refs != record.authority.selected_refs
        || proof.authority.grants != record.authority.grants
        || !matches!(&proof.authority.binding,InvocationBinding::Queue {queue_ref:reference,action_ref} if reference==queue_ref && action_ref==&template.dispatch_action_ref)
        || template.dispatch_action != Some(proof.executable.executable.action.id)
        || proof.authority.target_release_id != record.authority.target_release_id
        || proof.authority.target_release_digest != record.authority.target_release_digest
        || schema.input.source
            != (crate::value_schema::SourceIdentity::Released {
                component_ref: proof.authority.binding.action_ref().into(),
                release_id: proof.executable.release.id,
                release_digest: proof.executable.release.digest.clone(),
            })
    {
        return Err(denied());
    }
    let mut names = BTreeSet::new();
    if proof.contributors.is_empty() || proof.contributors.len() > 256 {
        return Err(denied());
    }
    for (name, body, digest) in &proof.contributors {
        if !names.insert(name) {
            return Err(denied());
        }
        Contributors::from_manifest(body, digest)?;
    }
    Fixed::validate_item_binding(
        conn,
        payload.fixed_input,
        item,
        &proof.authority,
        &proof.executable,
        &schema,
        &payload.schema,
        &proof.contributors,
        &payload.parent_receipts,
    )
    .await?;
    Ok(())
}

pub(in crate::repositories) struct QueueContributorSource {
    payload: QueuePayload,
    item: Id,
    count: usize,
    value: Value,
    template: WorkQueue,
    fence: ApprovalEffectFence,
    parents: Vec<crate::repositories::queue_configuration_input::QueueReceiptParent>,
}
impl QueueContributorSource {
    pub(in crate::repositories) fn fence(&self) -> &ApprovalEffectFence {
        &self.fence
    }
    pub(in crate::repositories) fn parents_for(
        &self,
        path: &str,
    ) -> Result<Vec<&crate::repositories::queue_configuration_input::QueueReceiptParent>> {
        for (root, _) in self.roots()? {
            let payload_root = if root.starts_with("/queue_items/") || root == "/queue_item" {
                format!("{root}/payload")
            } else {
                root.clone()
            };
            let selected = if crate::secret_values::pointer_suffix(path, &root).is_some() {
                if root == "/queue" || root == "/index" {
                    return Ok(Vec::new());
                }
                crate::secret_values::pointer_suffix(path, &payload_root).or_else(|| {
                    crate::secret_values::pointer_suffix(&payload_root, path).map(|_| String::new())
                })
            } else if crate::secret_values::pointer_suffix(&root, path).is_some() {
                Some(String::new())
            } else {
                continue;
            };
            return Ok(selected
                .map(|selected| {
                    self.parents
                        .iter()
                        .filter(|parent| {
                            crate::secret_values::pointer_suffix(parent.field_path(), &selected)
                                .is_some()
                                || crate::secret_values::pointer_suffix(
                                    &selected,
                                    parent.field_path(),
                                )
                                .is_some()
                        })
                        .collect()
                })
                .unwrap_or_default());
        }
        Err(denied())
    }
    pub(in crate::repositories) fn roots(&self) -> Result<Vec<(String, Value)>> {
        let mut wrapper = self.payload.context.clone();
        wrapper["payload"] = self.value.clone();
        let mut roots = vec![
            (
                format!("/items/{}", self.payload.ordinal),
                self.value.clone(),
            ),
            (
                format!("/queue_items/{}", self.payload.ordinal),
                wrapper.clone(),
            ),
        ];
        if self.template.batch_mode == crate::models::WorkQueueBatchMode::Single && self.count == 1
        {
            roots.extend([
                ("/item".into(), self.value.clone()),
                ("/queue_item".into(), wrapper),
                ("/index".into(), json!(0)),
            ]);
        }
        if self.payload.ordinal == 0 {
            let config: crate::models::WorkQueueConfig =
                serde_json::from_value(self.template.config.clone())?;
            roots.push(("/queue".into(),json!({"id":self.template.id,"ref":self.template.r#ref,"batch_mode":self.template.batch_mode,
                "leased_item_count":self.count,"dispatch_id":self.payload.dispatch,"ack_contract_version":config.ack_contract.as_ref().map(|ack|ack.version).unwrap_or(1)})));
        }
        Ok(roots)
    }
    pub(in crate::repositories) fn selection(
        &self,
        path: &str,
    ) -> Result<(Value, Vec<SecretOrigin>)> {
        for (root, value) in self.roots()? {
            if let Some(suffix) = crate::secret_values::pointer_suffix(path, &root) {
                let selected = value.pointer(&suffix).cloned().ok_or_else(denied)?;
                let payload_root = if root.starts_with("/queue_items/") || root == "/queue_item" {
                    format!("{root}/payload")
                } else {
                    root.clone()
                };
                if root == "/queue" || root == "/index" {
                    return Ok((selected, Vec::new()));
                }
                let payload_path = crate::secret_values::pointer_suffix(path, &payload_root)
                    .or_else(|| {
                        crate::secret_values::pointer_suffix(&payload_root, path)
                            .map(|_| String::new())
                    });
                let origin = payload_path
                    .as_ref()
                    .map(|payload_path| SecretOrigin::QueueItem {
                        queue_id: self.payload.queue,
                        queue_ref: self.payload.queue_ref.clone(),
                        pack_ref: self.payload.pack_ref.clone(),
                        item_id: self.item,
                        path: payload_path.clone(),
                    });
                let mut origins = origin.into_iter().collect::<Vec<_>>();
                if let Some(payload_path) = payload_path {
                    self.payload.append_origins(&payload_path, &mut origins)?;
                }
                return Ok((selected, origins));
            }
            if crate::secret_values::pointer_suffix(&root, path).is_some() {
                let mut origins = vec![SecretOrigin::QueueItem {
                    queue_id: self.payload.queue,
                    queue_ref: self.payload.queue_ref.clone(),
                    pack_ref: self.payload.pack_ref.clone(),
                    item_id: self.item,
                    path: String::new(),
                }];
                self.payload.append_origins("", &mut origins)?;
                return Ok((value, origins));
            }
        }
        Err(denied())
    }
}

pub(super) async fn source(
    conn: &mut PgConnection,
    renderer: &RuntimeAdmissionRecord,
    item: Id,
    master: &str,
) -> Result<QueueContributorSource> {
    let (body,expected,fixed):(Value,String,Option<Id>)=sqlx::query_as("SELECT proof,proof_digest,configuration_input FROM runtime_accepted_batch_item WHERE admission=$1 AND item=$2 FOR SHARE")
        .bind(renderer.id).bind(item).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
    let proof: ItemProof = serde_json::from_value(body)?;
    authenticate(&proof, item, master)?;
    if hash(&serde_json::to_value(&proof)?)? != expected {
        return Err(denied());
    }
    let current = RuntimeAuthorityRepository::require_retained_admission(conn, renderer.id).await?;
    if current.authority != renderer.authority || current.owner_kind != "execution" {
        return Err(denied());
    }
    validate_origin(conn, renderer.id, &proof).await?;
    let payload = proof.queue_payload.clone().ok_or_else(denied)?;
    if fixed != Some(payload.fixed_input) {
        return Err(denied());
    }
    check_item(conn, renderer, item, &proof, &payload.queue_ref).await?;
    let parents = payload.transfer_parents(conn, master).await?;
    validate_contributor_origins(conn, &proof, &parents).await?;
    let value = payload.restore(master)?;
    let count = match &renderer.authority.attribution {
        RequestAttribution::QueueBatch { items, .. } => items.len(),
        _ => return Err(denied()),
    };
    let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, renderer.id).await?;
    for parent in &parents {
        fence = fence.intersect(parent.source_fence());
    }
    Ok(QueueContributorSource {
        payload,
        item,
        count,
        value,
        template: serde_json::from_value(proof.template)?,
        fence,
        parents,
    })
}

impl QueuePayload {
    fn append_origins(&self, path: &str, origins: &mut Vec<SecretOrigin>) -> Result<()> {
        for field in &self.fields {
            if crate::secret_values::pointer_suffix(&field.json_path, path).is_none()
                && crate::secret_values::pointer_suffix(path, &field.json_path).is_none()
            {
                continue;
            }
            let provenance: SecretProvenance =
                serde_json::from_str(field.source_ref.as_deref().ok_or_else(denied)?)?;
            for origin in provenance.origins {
                // Canonical source edges retain/traverse typed parents. Do not
                // reinterpret their receipt IDs as generic entity origins.
                if crate::repositories::immutable_input_origin::is_input_receipt(&origin) {
                    continue;
                }
                if !origins.contains(&origin) {
                    origins.push(origin);
                }
            }
        }
        Ok(())
    }
}

pub(super) async fn validate_contributor_origins(
    conn: &mut PgConnection,
    proof: &ItemProof,
    parents: &[crate::repositories::queue_configuration_input::QueueReceiptParent],
) -> Result<()> {
    let payload = proof.queue_payload.as_ref().ok_or_else(denied)?;
    if proof.contributors.is_empty() || proof.contributors.len() > 256 || payload.fields.len() > 256
    {
        return Err(denied());
    }
    let authorities = proof
        .contributors
        .iter()
        .map(|(_, body, digest)| Contributors::from_manifest(body, digest))
        .collect::<Result<Vec<_>>>()?;
    for field in &payload.fields {
        let provenance: SecretProvenance =
            serde_json::from_str(field.source_ref.as_deref().ok_or_else(denied)?)?;
        if field.source_kind != "provenance"
            || provenance.origins.is_empty()
            || provenance.origins.len() > 256
        {
            return Err(denied());
        }
        for origin in &provenance.origins {
            match origin {
                SecretOrigin::Local { source_kind, .. }
                    if source_kind == "parameter_schema" || source_kind == "queue_item" => {}
                SecretOrigin::Key(origin) => {
                    // Every original source ceiling is independent. Metadata
                    // resolution verifies the exact retained owner tuple; it
                    // neither loads Key.value nor refreshes a sponsor's IAM.
                    for authority in &authorities {
                        for action in [crate::rbac::Action::Read]
                            .into_iter()
                            .chain(origin.encrypted.then_some(crate::rbac::Action::Decrypt))
                        {
                            if !Contributors::key_origin_allowed(conn, authority, origin, action)
                                .await?
                            {
                                return Err(denied());
                            }
                        }
                    }
                }
                other
                    if crate::repositories::immutable_input_origin::is_input_receipt(other)
                        && parents.iter().any(|parent| {
                            parent.field_path() == field.json_path && parent.origin() == *other
                        }) => {}
                _ => return Err(denied()),
            }
        }
    }
    Ok(())
}

fn attestation_key(master: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"attune:queue-input-attestation:key\0");
    hash.update(master.as_bytes());
    hash.finalize().into()
}
fn canonical_json(mut value: Value) -> Result<Vec<u8>> {
    // Preserve exact i64/u64 values. RFC8785/JCS float coercion is unsuitable
    // for authenticated BIGINT identity or exact admitted record values.
    value.sort_all_objects();
    Ok(serde_json::to_vec(&value)?)
}
fn associated_data(attestation: &Attestation) -> Result<Vec<u8>> {
    canonical_json(
        json!({"purpose":attestation.purpose,"item":attestation.item,"original_admission":attestation.original_admission}),
    )
}
fn attested_digest(proof: &ItemProof) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut body = serde_json::to_value(proof)?;
    body.get_mut("queue_payload")
        .and_then(Value::as_object_mut)
        .ok_or_else(denied)?
        .remove("attestation");
    Ok(Sha256::digest(canonical_json(body)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn seal(proof: &mut ItemProof, master: &str) -> Result<()> {
    let digest = attested_digest(proof)?;
    let attestation = &mut proof.queue_payload.as_mut().ok_or_else(denied)?.attestation;
    let encrypted = crate::crypto::encrypt_authenticated(
        digest.as_bytes(),
        &attestation_key(master),
        &associated_data(attestation)?,
    )?;
    attestation.nonce = encrypted.nonce;
    attestation.ciphertext = encrypted.ciphertext;
    Ok(())
}
pub(super) fn authenticate(proof: &ItemProof, item: Id, master: &str) -> Result<()> {
    let payload = proof.queue_payload.as_ref().ok_or_else(denied)?;
    let attestation = &payload.attestation;
    if attestation.purpose != PURPOSE
        || attestation.item != item
        || item <= 0
        || attestation.original_admission <= 0
        || attestation.encryption_key_hash != crate::crypto::hash_encryption_key(master)
    {
        return Err(denied());
    }
    // Authenticate before interpreting source fields or trusting a public hash.
    let digest = crate::crypto::decrypt_authenticated(
        &attestation.ciphertext,
        &attestation.nonce,
        &attestation_key(master),
        &associated_data(attestation)?,
    )?;
    if digest != attested_digest(proof)?.as_bytes() {
        return Err(denied());
    }
    Ok(())
}
pub(super) async fn validate_origin(
    conn: &mut PgConnection,
    admission: Id,
    proof: &ItemProof,
) -> Result<()> {
    let original = proof
        .queue_payload
        .as_ref()
        .ok_or_else(denied)?
        .attestation
        .original_admission;
    let related:bool=sqlx::query_scalar("WITH RECURSIVE lineage(id) AS (SELECT $1::BIGINT UNION SELECT edge.dependency FROM lineage JOIN runtime_admission_dependency edge ON edge.admission=lineage.id AND edge.dependency_kind='retry' WHERE edge.dependency<lineage.id) SELECT count(*)<=256 AND COALESCE(bool_or(id=$2),false) FROM (SELECT id FROM lineage LIMIT 257) bounded")
        .bind(admission).bind(original).fetch_one(conn).await?;
    if !related {
        return Err(denied());
    }
    Ok(())
}

#[cfg(test)]
mod attestation_tests {
    use super::*;
    #[test]
    fn authenticated_encoding_preserves_full_bigint_identity_and_values() {
        let first = 9_007_199_254_740_992_i64;
        assert_ne!(
            canonical_json(json!({"item":first})).unwrap(),
            canonical_json(json!({"item":first+1})).unwrap()
        );
        assert_ne!(
            canonical_json(json!({"payload":{"value":first}})).unwrap(),
            canonical_json(json!({"payload":{"value":first+1}})).unwrap()
        );
        assert_eq!(
            canonical_json(json!({"a":1,"b":2})).unwrap(),
            canonical_json(serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap()).unwrap()
        );
    }
}
