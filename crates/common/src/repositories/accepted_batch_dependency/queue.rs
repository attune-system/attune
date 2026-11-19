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
pub(super) struct QueuePayload {
    pub(super) fixed_input: Id,
    pub(super) ordinal: usize,
    queue: Id,
    queue_ref: String,
    pack_ref: Option<String>,
    schema: ValueSchemaBinding,
    payload: Value,
    fields: Vec<PreparedSecretValue>,
    context: Value,
    dispatch: Option<Id>,
}
impl QueuePayload {
    pub(super) fn field_count(&self) -> usize {
        self.fields.len()
    }
    pub(super) fn restore(&self, master: &str) -> Result<Value> {
        self.schema
            .validate(crate::value_schema::SchemaContext::FlatDocument)
            .map_err(|_| denied())?;
        for field in &self.fields {
            let provenance: SecretProvenance =
                serde_json::from_str(field.source_ref.as_deref().ok_or_else(denied)?)?;
            if field.source_kind != "provenance" || provenance.origins.is_empty()
                || provenance.origins.iter().any(|origin|!matches!(origin,SecretOrigin::Local {source_kind,..} if source_kind=="parameter_schema" || source_kind=="queue_item")) {
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
            fixed_input: fixed.id(),
            ordinal,
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            pack_ref: queue.pack_ref.clone(),
            schema: raw.payload_binding()?,
            payload: raw.payload.clone(),
            fields,
            context: json!({
                "id":raw.id,"item_key":raw.item_key,"priority":raw.priority,"metadata":raw.metadata,"enqueue_source":raw.enqueue_source,
                "attempt_count":raw.attempt_count,"requested_by_identity":raw.requested_by_identity,"requested_by_execution":raw.requested_by_execution,"requested_by_enforcement":raw.requested_by_enforcement,
            }),
            dispatch,
        };
        payload.restore(master)?;
        let proof = ItemProof {
            source_admission: None,
            authority,
            executable: snapshot.clone(),
            template: serde_json::to_value(fixed.accepted_template(queue)?)?,
            schema: serde_json::to_value(fixed.schema())?,
            contributors,
            queue_payload: Some(payload),
        };
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
}
impl QueueContributorSource {
    pub(in crate::repositories) fn fence(&self) -> &ApprovalEffectFence {
        &self.fence
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
                let origin = payload_path.map(|payload_path| SecretOrigin::QueueItem {
                    queue_id: self.payload.queue,
                    queue_ref: self.payload.queue_ref.clone(),
                    pack_ref: self.payload.pack_ref.clone(),
                    item_id: self.item,
                    path: payload_path,
                });
                return Ok((selected, origin.into_iter().collect()));
            }
            if crate::secret_values::pointer_suffix(&root, path).is_some() {
                return Ok((
                    value,
                    vec![SecretOrigin::QueueItem {
                        queue_id: self.payload.queue,
                        queue_ref: self.payload.queue_ref.clone(),
                        pack_ref: self.payload.pack_ref.clone(),
                        item_id: self.item,
                        path: String::new(),
                    }],
                ));
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
    let current = RuntimeAuthorityRepository::require_retained_admission(conn, renderer.id).await?;
    if current.authority != renderer.authority || current.owner_kind != "execution" {
        return Err(denied());
    }
    let (body,expected,fixed):(Value,String,Option<Id>)=sqlx::query_as("SELECT proof,proof_digest,configuration_input FROM runtime_accepted_batch_item WHERE admission=$1 AND item=$2 FOR SHARE")
        .bind(renderer.id).bind(item).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
    if hash(&body)? != expected {
        return Err(denied());
    }
    let proof: ItemProof = serde_json::from_value(body)?;
    let payload = proof.queue_payload.clone().ok_or_else(denied)?;
    if fixed != Some(payload.fixed_input) {
        return Err(denied());
    }
    check_item(conn, renderer, item, &proof, &payload.queue_ref).await?;
    Fixed::authenticate_record(conn, payload.fixed_input, master).await?;
    let value = payload.restore(master)?;
    let count = match &renderer.authority.attribution {
        RequestAttribution::QueueBatch { items, .. } => items.len(),
        _ => return Err(denied()),
    };
    let fence = RuntimeAuthorityRepository::retained_admission_fence(conn, renderer.id).await?;
    Ok(QueueContributorSource {
        payload,
        item,
        count,
        value,
        template: serde_json::from_value(proof.template)?,
        fence,
    })
}
