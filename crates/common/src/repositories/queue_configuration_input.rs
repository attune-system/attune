//! Item-owned fixed configuration inputs. This proof never yields execution API
//! grants and never uses the submitter's IAM authority.
use super::{
    component_configuration::ComponentConfigurationRepository as Configuration,
    declared_input::{DeclaredInputBinding, DeclaredInputRepository},
    execution_runtime_admission::ExecutionAdmissionRepository as Admission,
    key::KeyRepository,
    runtime_target::{RuntimeTarget, RuntimeTargetRepository},
    schema_contract::{ActionSchemaBinding, SchemaContractRepository},
};
use crate::{
    crypto,
    models::{ExecutionExecutableSnapshot, Id, WorkQueue, WorkQueueItem},
    secret_provenance::KeyOrigin,
    value_schema::digest,
    workflow::expression::{self, EvalContext, EvalError, EvalResult},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgConnection;
use std::collections::BTreeMap;
mod transfer;
pub use transfer::QueueInputTransfer;
pub(in crate::repositories) use transfer::QueueReceiptParent;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyInputVersion {
    key: Id,
    revision: i64,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    id: Id,
    item: Id,
    queue: Id,
    queue_ref: String,
    policy: Id,
    authority_digest: String,
    authority: crate::runtime_authority::SelectedRuntimeAuthority,
    contributor_digests: BTreeMap<String, String>,
    source_execution: Option<Id>,
    source_receipts: Vec<Id>,
    executable_digest: String,
    item_schema_digest: String,
    template_digest: String,
    pack: Id,
    configuration: Option<Id>,
    configuration_digest: Option<String>,
    declarations: Vec<DeclaredInputBinding>,
    keys: BTreeMap<String, KeyOrigin>,
    key_targets: BTreeMap<String, crate::runtime_authority::BoundRuntimeTarget>,
    key_versions: BTreeMap<String, KeyInputVersion>,
    schema: ActionSchemaBinding,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sealed {
    binding: Binding,
    configuration: Value,
    keys: BTreeMap<String, Value>,
    transfers: Vec<transfer::TransferProof>,
}

/// Constructed only by validating an immutable owning record and active source
/// fences. No serialized caller body can assert this proof.
pub struct QueueConfigurationInput {
    sealed: Sealed,
    fence: super::runtime_authority::ApprovalEffectFence,
    metadata: Value,
    acceptance_sources: Vec<transfer::LiveTransferSource>,
}
impl QueueConfigurationInput {
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        for source in &self.acceptance_sources {
            source.ensure_fresh_on(conn).await?;
        }
        self.fence.ensure_fresh_on(conn).await
    }
    pub(in crate::repositories) fn source_receipts(&self) -> &[Id] {
        &self.sealed.binding.source_receipts
    }
    pub(in crate::repositories) async fn validate_transfers(
        &self,
        conn: &mut PgConnection,
        fields: &[crate::secret_values::PreparedSecretValue],
        master: &str,
    ) -> Result<Vec<QueueReceiptParent>> {
        validate_transfer_fields(conn, &self.sealed, fields, master).await
    }
    pub fn compatible_fixed_inputs(&self, other: &Self) -> bool {
        self.sealed.binding.policy == other.sealed.binding.policy
            && self.sealed.binding.template_digest == other.sealed.binding.template_digest
            && self.sealed.binding.configuration == other.sealed.binding.configuration
            && self.sealed.binding.key_versions == other.sealed.binding.key_versions
            && self.sealed.binding.keys == other.sealed.binding.keys
            && self.sealed.binding.key_targets == other.sealed.binding.key_targets
            && self.sealed.binding.schema == other.sealed.binding.schema
            && self.sealed.configuration == other.sealed.configuration
            && self.sealed.keys == other.sealed.keys
    }
    pub async fn configuration_secret_paths(&self, conn: &mut PgConnection) -> Result<Vec<String>> {
        self.fence.ensure_fresh_on(conn).await?;
        let Some(id) = self.sealed.binding.configuration else {
            return Ok(Vec::new());
        };
        let revision = Configuration::find(&mut *conn, id)
            .await?
            .ok_or_else(denied)?;
        crate::value_schema::CompiledContract::new(&revision.contract()?)
            .map_err(|_| denied())?
            .secret_paths(&self.sealed.configuration)
            .map_err(|_| denied())
    }
    pub fn accepted_template(&self, current: &WorkQueue) -> Result<WorkQueue> {
        if current.id != self.sealed.binding.queue
            || current.r#ref != self.sealed.binding.queue_ref
            || current.pack != Some(self.sealed.binding.pack)
        {
            return Err(denied());
        }
        let mut queue = current.clone();
        queue.action_params = self
            .metadata
            .get("action_params")
            .cloned()
            .ok_or_else(denied)?;
        queue.item_schema = self
            .metadata
            .get("item_schema")
            .cloned()
            .ok_or_else(denied)?;
        queue.config = self.metadata.get("config").cloned().ok_or_else(denied)?;
        queue.dispatch_action = Some(
            self.metadata["dispatch_action"]
                .as_i64()
                .ok_or_else(denied)?,
        );
        queue.dispatch_action_ref = self.metadata["dispatch_action_ref"]
            .as_str()
            .ok_or_else(denied)?
            .into();
        queue.permission_set_refs =
            serde_json::from_value(self.metadata["permission_set_refs"].clone())?;
        queue.trace_tag_template =
            serde_json::from_value(self.metadata["trace_tag_template"].clone())?;
        queue.batch_mode = serde_json::from_value(
            self.metadata
                .get("batch_mode")
                .cloned()
                .ok_or_else(denied)?,
        )?;
        Ok(queue)
    }
    pub fn id(&self) -> Id {
        self.sealed.binding.id
    }
    pub fn configuration(&self) -> &Value {
        &self.sealed.configuration
    }
    pub fn configuration_revision(&self) -> Option<Id> {
        self.sealed.binding.configuration
    }
    pub fn schema(&self) -> &ActionSchemaBinding {
        &self.sealed.binding.schema
    }
    pub fn declarations(&self) -> &[DeclaredInputBinding] {
        &self.sealed.binding.declarations
    }
    pub(in crate::repositories) fn fence(&self) -> &super::runtime_authority::ApprovalEffectFence {
        &self.fence
    }
    pub(in crate::repositories) fn selection(
        &self,
        path: &str,
    ) -> Result<(
        Value,
        Vec<crate::secret_provenance::SecretOrigin>,
        crate::runtime_authority::BoundRuntimeTarget,
    )> {
        use crate::secret_provenance::SecretOrigin;
        if path == "$queue_input" {
            return Ok((
                Value::Null,
                Vec::new(),
                crate::runtime_authority::BoundRuntimeTarget {
                    kind: "queue".into(),
                    id: self.sealed.binding.queue,
                    pack_id: Some(self.sealed.binding.pack),
                    owner_type: None,
                    owner_id: None,
                },
            ));
        }
        for (reference, origin) in &self.sealed.binding.keys {
            let root = format!(
                "/keystore/{}",
                reference.replace('~', "~0").replace('/', "~1")
            );
            if let Some(suffix) = crate::secret_values::pointer_suffix(path, &root) {
                let value = self.sealed.keys[reference]
                    .pointer(&suffix)
                    .cloned()
                    .ok_or_else(denied)?;
                return Ok((
                    value,
                    vec![SecretOrigin::Key(origin.clone())],
                    self.sealed
                        .binding
                        .key_targets
                        .get(reference)
                        .cloned()
                        .ok_or_else(denied)?,
                ));
            }
        }
        for root in ["/config", "/pack/config"] {
            if let Some(suffix) = crate::secret_values::pointer_suffix(path, root) {
                let value = self
                    .sealed
                    .configuration
                    .pointer(&suffix)
                    .cloned()
                    .ok_or_else(denied)?;
                return Ok((
                    value,
                    vec![SecretOrigin::Entity {
                        entity_type: "component_configuration".into(),
                        entity_id: self.sealed.binding.configuration.ok_or_else(denied)?,
                        path: suffix,
                    }],
                    crate::runtime_authority::BoundRuntimeTarget {
                        kind: "pack".into(),
                        id: self.sealed.binding.pack,
                        pack_id: Some(self.sealed.binding.pack),
                        owner_type: None,
                        owner_id: None,
                    },
                ));
            }
        }
        Err(denied())
    }
    pub fn resolved_keys(&self) -> BTreeMap<String, crate::key_access::ResolvedKey> {
        self.sealed
            .binding
            .keys
            .iter()
            .map(|(reference, origin)| {
                (
                    reference.clone(),
                    crate::key_access::ResolvedKey {
                        origin: origin.clone(),
                        value: self.sealed.keys[reference].clone(),
                    },
                )
            })
            .collect()
    }
}

pub struct QueueConfigurationInputRepository;
impl QueueConfigurationInputRepository {
    pub(in crate::repositories) async fn authenticate_record(
        conn: &mut PgConnection,
        id: Id,
        master: &str,
    ) -> Result<()> {
        let row:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'item',item,'policy_revision',policy_revision,'configuration_revision',configuration_revision,'binding',binding,'binding_digest',binding_digest,'encrypted_context',encrypted_context,'encryption_key_hash',encryption_key_hash,'source_receipts',source_receipts) FROM work_queue_item_configuration_input WHERE id=$1 FOR SHARE")
            .bind(id).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        authenticate_stored(&row, master)?;
        let sealed: Sealed =
            serde_json::from_value(crypto::decrypt_json(&row["encrypted_context"], master)?)?;
        let fields = sealed
            .transfers
            .iter()
            .map(|transfer| transfer.field().clone())
            .collect::<Vec<_>>();
        validate_transfer_fields(conn, &sealed, &fields, master).await?;
        Ok(())
    }
    pub(in crate::repositories) async fn transfer_contexts(
        conn: &mut PgConnection,
        id: Id,
        fields: &[crate::secret_values::PreparedSecretValue],
        master: &str,
    ) -> Result<(Vec<Id>, Vec<QueueReceiptParent>)> {
        let row:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'item',item,'policy_revision',policy_revision,'configuration_revision',configuration_revision,'binding',binding,'binding_digest',binding_digest,'encrypted_context',encrypted_context,'encryption_key_hash',encryption_key_hash,'source_receipts',source_receipts) FROM work_queue_item_configuration_input WHERE id=$1 FOR SHARE")
            .bind(id).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        authenticate_stored(&row, master)?;
        let sealed: Sealed =
            serde_json::from_value(crypto::decrypt_json(&row["encrypted_context"], master)?)?;
        let contexts = validate_transfer_fields(conn, &sealed, fields, master).await?;
        Ok((sealed.binding.source_receipts, contexts))
    }
    pub(in crate::repositories) async fn cohort_identity(
        conn: &mut PgConnection,
        id: Id,
    ) -> Result<String> {
        let (body,expected):(Value,String)=sqlx::query_as("SELECT binding,binding_digest FROM work_queue_item_configuration_input WHERE id=$1 FOR SHARE")
            .bind(id).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        if hash(&body)? != expected {
            return Err(denied());
        }
        // Versions contain only repository-owned Key IDs and durable update
        // revisions, never a value hash. Grouping is not authorization.
        hash(
            &json!({"policy":body["policy"],"configuration":body["configuration"],"template":body["template_digest"],
            "schema":body["item_schema_digest"],"keys":body["key_versions"]}),
        )
    }
    pub(in crate::repositories) async fn validate_item_binding(
        conn: &mut PgConnection,
        id: Id,
        item: Id,
        authority: &crate::runtime_authority::SelectedRuntimeAuthority,
        executable: &ExecutionExecutableSnapshot,
        schema: &ActionSchemaBinding,
        payload_schema: &crate::value_schema::ValueSchemaBinding,
        contributors: &[(String, Value, String)],
        source_receipts: &[Id],
    ) -> Result<()> {
        let (owner,policy,configuration,body,expected,retained):(Id,Id,Option<Id>,Value,String,Vec<Id>)=sqlx::query_as("SELECT item,policy_revision,configuration_revision,binding,binding_digest,source_receipts FROM work_queue_item_configuration_input WHERE id=$1 FOR SHARE")
            .bind(id).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        if hash(&body)? != expected {
            return Err(denied());
        }
        let binding: Binding = serde_json::from_value(body)?;
        let mut digests = BTreeMap::new();
        for (name, body, expected) in contributors {
            super::recorded_input_authority::RecordedInputAuthorityRepository::from_manifest(
                body, expected,
            )?;
            if digests.insert(name.clone(), expected.clone()).is_some() {
                return Err(denied());
            }
        }
        if binding.id != id
            || owner != item
            || binding.item != item
            || binding.policy != policy
            || binding.configuration != configuration
            || binding.source_receipts != source_receipts
            || binding.source_receipts != retained
            || binding.authority != *authority
            || digests.is_empty()
            || binding.contributor_digests != digests
            || binding.authority_digest != hash(&serde_json::to_value(authority)?)?
            || binding.executable_digest != hash(&serde_json::to_value(executable)?)?
            || binding.schema != *schema
            || binding.item_schema_digest != hash(&serde_json::to_value(payload_schema)?)?
        {
            return Err(denied());
        }
        Ok(())
    }
    /// Exclusive graph admission must precede any source locks. The graph gate
    /// serializes receipt attachment against reclamation; the FK independently
    /// protects every retained receipt, including receipts for deleted items.
    pub async fn reclaim_with_conn(conn: &mut PgConnection, limit: u32) -> Result<usize> {
        let held: bool = sqlx::query_scalar("SELECT file_evidence_graph_held(true)")
            .fetch_one(&mut *conn)
            .await?;
        if !held {
            return Err(denied());
        }
        let removed: Vec<Id> = sqlx::query_scalar(
            "WITH candidates AS (SELECT q.id FROM work_queue_item_configuration_input q \
             WHERE NOT EXISTS(SELECT 1 FROM work_queue_item i WHERE i.id=q.item) \
               AND NOT EXISTS(SELECT 1 FROM runtime_input_queue_configuration_reference r WHERE r.input=q.id) \
               AND NOT EXISTS(SELECT 1 FROM runtime_accepted_batch_item batch WHERE batch.configuration_input=q.id) \
               AND NOT file_evidence_pinned('work_queue_item_configuration_input',q.id) \
             ORDER BY q.id LIMIT $1 FOR UPDATE SKIP LOCKED) \
             DELETE FROM work_queue_item_configuration_input q USING candidates c WHERE q.id=c.id RETURNING q.id",
        ).bind(i64::from(limit.min(1000))).fetch_all(conn).await?;
        Ok(removed.len())
    }

    /// Retained receipt validation never requires the source item to survive.
    /// Its actual renderer admission and exact authored declaration remain the
    /// authority boundary; record IDs and FKs alone authorize nothing.
    pub(in crate::repositories) async fn for_renderer(
        conn: &mut PgConnection,
        input: Id,
        source: &super::runtime_authority::RuntimeAdmissionRecord,
        declaration: &DeclaredInputBinding,
        master: &str,
    ) -> Result<QueueConfigurationInput> {
        use super::runtime_authority::RuntimeAuthorityRepository as Authority;
        let current = Authority::require_retained_admission(conn, source.id).await?;
        if current.owner_kind != "execution" || current.authority != source.authority {
            return Err(denied());
        }
        let mut fence = Authority::retained_admission_fence(conn, source.id).await?;
        let row:Value = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'item',item,'policy_revision',policy_revision,'configuration_revision',configuration_revision,'binding',binding,'binding_digest',binding_digest,'encrypted_context',encrypted_context,'encryption_key_hash',encryption_key_hash,'source_receipts',source_receipts) FROM work_queue_item_configuration_input WHERE id=$1 FOR SHARE")
            .bind(input).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        authenticate_stored(&row, master)?;
        let sealed: Sealed =
            serde_json::from_value(crypto::decrypt_json(&row["encrypted_context"], master)?)?;
        let (accepted_body,accepted_hash,fixed):(Value,String,Option<Id>)=sqlx::query_as("SELECT proof,proof_digest,configuration_input FROM runtime_accepted_batch_item WHERE admission=$1 AND item=$2 FOR SHARE")
            .bind(source.id).bind(sealed.binding.item).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        super::accepted_batch_dependency::AcceptedBatchDependencyRepository::authenticate_queue_member(conn,source.id,sealed.binding.item,&accepted_body,master).await?;
        if hash(&accepted_body)? != accepted_hash
            || fixed != Some(input)
            || accepted_body.get("authority")
                != Some(&serde_json::to_value(&sealed.binding.authority)?)
        {
            return Err(denied());
        }
        let fields: Vec<crate::secret_values::PreparedSecretValue> =
            serde_json::from_value(accepted_body["queue_payload"]["fields"].clone())?;
        for parent in validate_transfer_fields(conn, &sealed, &fields, master).await? {
            fence = fence.intersect(parent.source_fence());
        }
        let first:Option<Id>=sqlx::query_scalar("SELECT configuration_input FROM runtime_accepted_batch_item WHERE admission=$1 AND proof->'queue_payload'->>'ordinal'='0'")
            .bind(source.id).fetch_optional(&mut *conn).await?.flatten();
        if first != Some(input) {
            return Err(denied());
        }
        let accepted = &sealed.binding.authority;
        let renderer = &source.authority;
        if sealed.binding.id != input
            || accepted.policy_id != sealed.binding.policy
            || renderer.policy_id != accepted.policy_id
            || renderer.owner != accepted.owner
            || renderer.binding != accepted.binding
            || renderer.selected_refs != accepted.selected_refs
            || renderer.grants != accepted.grants
            || !sealed.binding.declarations.contains(declaration)
        {
            return Err(denied());
        }
        match &renderer.attribution {
            crate::runtime_authority::RequestAttribution::QueueBatch { queue_ref, items }
                if queue_ref == &sealed.binding.queue_ref
                    && items.contains_key(&sealed.binding.item) =>
            {
                ()
            }
            _ => return Err(denied()),
        }
        let policy = Authority::require_policy(conn, sealed.binding.policy).await?;
        let invocation = policy
            .policy
            .invocations
            .iter()
            .find(|entry| entry.binding == accepted.binding)
            .ok_or_else(denied)?;
        if invocation.source_id != Some(sealed.binding.queue)
            || invocation.dispatch_template_digest.as_deref()
                != Some(sealed.binding.template_digest.as_str())
        {
            return Err(denied());
        }
        let executable =
            super::managed_executable::ManagedExecutableRepository::resolve_invocation(
                conn,
                &policy.policy,
                &accepted.binding,
            )
            .await?;
        if hash(&serde_json::to_value(&executable)?)? != sealed.binding.executable_digest
            || SchemaContractRepository::materialize_action_pair(conn, &executable).await?
                != sealed.binding.schema
        {
            return Err(denied());
        }
        DeclaredInputRepository::validate_declaration(conn, source, declaration).await?;
        if let Some(id) = sealed.binding.configuration {
            let revision = Configuration::find(&mut *conn, id)
                .await?
                .ok_or_else(denied)?;
            if revision.owner_kind != "pack"
                || revision.owner_id != sealed.binding.pack
                || !Configuration::owner_survives(conn, &revision).await?
                || sealed.binding.configuration_digest
                    != Some(hash(
                        &json!({"contract":revision.contract,"config":revision.config}),
                    )?)
            {
                return Err(denied());
            }
        }
        fence.ensure_fresh_on(conn).await?;
        let metadata:Value = sqlx::query_scalar("SELECT metadata FROM managed_dispatch_metadata_revision WHERE policy_revision=$1 AND component_kind='queues' AND component_id=$2")
            .bind(sealed.binding.policy).bind(sealed.binding.queue).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
        if hash(&metadata)? != sealed.binding.template_digest {
            return Err(denied());
        }
        Ok(QueueConfigurationInput {
            sealed,
            fence,
            metadata,
            acceptance_sources: Vec::new(),
        })
    }

    /// Capture only during ORIGINAL item/admission creation. Repeating this call
    /// reads the accepted ciphertext, never today's Key/configuration value.
    pub async fn capture(
        conn: &mut PgConnection,
        queue: &WorkQueue,
        item: &WorkQueueItem,
        snapshot: &ExecutionExecutableSnapshot,
        encryption_key: &str,
    ) -> Result<QueueConfigurationInput> {
        Self::capture_with_transfers(conn, queue, item, snapshot, encryption_key, &[]).await
    }

    /// Internal protected-input producer. Capabilities have no serialized
    /// constructor and remain bound to this exact item/acceptance transaction.
    pub async fn capture_with_transfers(
        conn: &mut PgConnection,
        queue: &WorkQueue,
        item: &WorkQueueItem,
        snapshot: &ExecutionExecutableSnapshot,
        encryption_key: &str,
        transfers: &[QueueInputTransfer],
    ) -> Result<QueueConfigurationInput> {
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM work_queue_item_configuration_input WHERE item=$1)",
        )
        .bind(item.id)
        .fetch_one(&mut *conn)
        .await?
        {
            let retained = Self::load(conn, queue, item, snapshot, encryption_key).await?;
            if !transfers.is_empty() {
                let mut expected = Vec::new();
                for transfer in transfers {
                    expected.push(
                        transfer
                            .accepted(conn, &retained.sealed.binding, encryption_key)
                            .await?,
                    );
                }
                expected.sort_by(|a, b| a.path().cmp(b.path()));
                if expected != retained.sealed.transfers {
                    return Err(denied());
                }
            }
            return Ok(retained);
        }
        let original: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM work_queue_item i JOIN work_queue_item_runtime_admission a ON a.item=i.id \
             WHERE i.id=$1 AND i.input_creation_transaction=pg_current_xact_id())",
        ).bind(item.id).fetch_one(&mut *conn).await?;
        if !original {
            return Err(denied());
        }
        let selected =
            Admission::queue_batch(conn, queue, std::slice::from_ref(item), snapshot).await?;
        let (mut binding, metadata) =
            binding(conn, queue, item, snapshot, selected.authority().policy_id).await?;
        binding.id = sqlx::query_scalar(
            "SELECT nextval(pg_get_serial_sequence('work_queue_item_configuration_input','id'))",
        )
        .fetch_one(&mut *conn)
        .await?;
        let mut accepted_transfers = Vec::new();
        let mut fence = selected.effect_fence().clone();
        for transfer in transfers {
            accepted_transfers.push(transfer.accepted(conn, &binding, encryption_key).await?);
            fence = fence.intersect(transfer.fence());
        }
        accepted_transfers.sort_by(|a, b| a.path().cmp(b.path()));
        if accepted_transfers.len() > 256
            || accepted_transfers
                .windows(2)
                .any(|pair| pair[0].path() == pair[1].path())
        {
            return Err(denied());
        }
        binding.source_receipts = accepted_transfers
            .iter()
            .map(|transfer| transfer.receipt())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut configuration = Value::Null;
        if binding
            .declarations
            .iter()
            .any(|site| site.uses_pack_configuration())
        {
            let revision = Configuration::current(&mut *conn, "pack", binding.pack)
                .await?
                .ok_or_else(denied)?;
            if !Configuration::owner_survives(conn, &revision).await? {
                return Err(denied());
            }
            configuration = Configuration::restore(conn, &revision, Some(encryption_key)).await?;
            binding.configuration = Some(revision.id);
            binding.configuration_digest = Some(hash(
                &json!({"contract":revision.contract,"config":revision.config}),
            )?);
        }
        let mut values = BTreeMap::new();
        for reference in fixed_key_references(&metadata["action_params"], &configuration)? {
            let key = KeyRepository::find_by_ref_for_share(conn, &reference)
                .await?
                .ok_or_else(denied)?;
            let target = RuntimeTargetRepository::resolve(
                conn,
                RuntimeTarget::Key(key.id),
                selected.authority().owner.sponsor_identity_id,
            )
            .await?
            .ok_or_else(denied)?;
            // Business-owned credential delivery is purpose-specific. External
            // owner inputs require an explicit input approval, not ambient IAM.
            if target.pack_id() != Some(binding.pack) {
                return Err(denied());
            }
            binding.key_versions.insert(
                reference.clone(),
                KeyInputVersion {
                    key: key.id,
                    revision: KeyRepository::input_revision(conn, key.id)
                        .await?
                        .filter(|revision| *revision > 0)
                        .ok_or_else(denied)?,
                },
            );
            binding.key_targets.insert(
                reference.clone(),
                crate::runtime_authority::BoundRuntimeTarget {
                    kind: "key".into(),
                    id: key.id,
                    pack_id: target.pack_id(),
                    owner_type: Some(
                        serde_json::to_value(key.owner_type)?
                            .as_str()
                            .ok_or_else(denied)?
                            .into(),
                    ),
                    owner_id: target.owner_id(),
                },
            );
            binding
                .keys
                .insert(reference.clone(), crate::key_access::key_origin(&key));
            values.insert(
                reference,
                if key.encrypted {
                    crypto::keystore::decrypt_json(&key.value, encryption_key, key.id)?
                } else {
                    key.value
                },
            );
        }
        let sealed = Sealed {
            binding,
            configuration,
            keys: values,
            transfers: accepted_transfers,
        };
        let fields = transfer::fields(conn, item.id).await?;
        for parent in validate_transfer_fields(conn, &sealed, &fields, encryption_key).await? {
            fence = fence.intersect(parent.source_fence());
        }
        fence.ensure_fresh_on(conn).await?;
        let body = serde_json::to_value(&sealed.binding)?;
        let plaintext = serde_json::to_value(&sealed)?;
        if serde_json::to_vec(&plaintext)?.len() > 1024 * 1024 {
            return Err(denied());
        }
        // The authenticated encrypted envelope includes the complete public
        // binding, preventing ciphertext substitution across items or policies.
        let ciphertext = crypto::encrypt_json(&plaintext, encryption_key)?;
        sqlx::query("INSERT INTO work_queue_item_configuration_input(item,policy_revision,configuration_revision,binding,binding_digest,encrypted_context,encryption_key_hash,id,source_receipts) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(item.id).bind(sealed.binding.policy).bind(sealed.binding.configuration).bind(&body).bind(hash(&body)?)
            .bind(ciphertext).bind(crypto::hash_encryption_key(encryption_key)).bind(sealed.binding.id).bind(&sealed.binding.source_receipts).execute(&mut *conn).await?;
        let input = QueueConfigurationInput {
            sealed,
            fence,
            metadata,
            acceptance_sources: transfers
                .iter()
                .map(QueueInputTransfer::live_source)
                .collect(),
        };
        input.ensure_fresh_on(conn).await?;
        Ok(input)
    }

    pub async fn load(
        conn: &mut PgConnection,
        queue: &WorkQueue,
        item: &WorkQueueItem,
        snapshot: &ExecutionExecutableSnapshot,
        encryption_key: &str,
    ) -> Result<QueueConfigurationInput> {
        let selected =
            Admission::queue_batch(conn, queue, std::slice::from_ref(item), snapshot).await?;
        let row: Option<(Id,Option<Id>,Value,String,Value,String,Id,Vec<Id>)> = sqlx::query_as(
            "SELECT policy_revision,configuration_revision,binding,binding_digest,encrypted_context,encryption_key_hash,id,source_receipts FROM work_queue_item_configuration_input WHERE item=$1 FOR SHARE",
        ).bind(item.id).fetch_optional(&mut *conn).await?;
        let (policy, configuration, body, expected, encrypted, key_hash, id, source_receipts) =
            row.ok_or_else(denied)?;
        if hash(&body)? != expected || key_hash != crypto::hash_encryption_key(encryption_key) {
            return Err(denied());
        }
        let sealed: Sealed =
            serde_json::from_value(crypto::decrypt_json(&encrypted, encryption_key)?)?;
        validate_transfer_ids(&sealed, encryption_key)?;
        if serde_json::to_value(&sealed.binding)? != body
            || sealed.binding.policy != policy
            || sealed.binding.configuration != configuration
            || sealed.binding.id != id
            || sealed.binding.source_receipts != source_receipts
            || policy != selected.authority().policy_id
        {
            return Err(denied());
        }
        let (mut actual, metadata) = binding(conn, queue, item, snapshot, policy).await?;
        actual.id = id;
        actual.configuration = sealed.binding.configuration;
        actual.configuration_digest = sealed.binding.configuration_digest.clone();
        actual.keys = sealed.binding.keys.clone();
        actual.key_targets = sealed.binding.key_targets.clone();
        actual.key_versions = sealed.binding.key_versions.clone();
        actual.source_receipts = sealed.binding.source_receipts.clone();
        if actual != sealed.binding || sealed.keys.keys().ne(sealed.binding.keys.keys()) {
            return Err(denied());
        }
        if let Some(id) = configuration {
            let revision = Configuration::find(&mut *conn, id)
                .await?
                .ok_or_else(denied)?;
            if revision.owner_kind != "pack"
                || revision.owner_id != sealed.binding.pack
                || Some(hash(
                    &json!({"contract":revision.contract,"config":revision.config}),
                )?) != sealed.binding.configuration_digest
                || !Configuration::owner_survives(conn, &revision).await?
            {
                return Err(denied());
            }
        }
        let fields = transfer::fields(conn, item.id).await?;
        let mut fence = selected.effect_fence().clone();
        for parent in validate_transfer_fields(conn, &sealed, &fields, encryption_key).await? {
            fence = fence.intersect(parent.source_fence());
        }
        fence.ensure_fresh_on(conn).await?;
        Ok(QueueConfigurationInput {
            sealed,
            fence,
            metadata,
            acceptance_sources: Vec::new(),
        })
    }
}

async fn binding(
    conn: &mut PgConnection,
    queue: &WorkQueue,
    item: &WorkQueueItem,
    snapshot: &ExecutionExecutableSnapshot,
    policy: Id,
) -> Result<(Binding, Value)> {
    if queue.is_adhoc || queue.id != item.queue || queue.r#ref != item.queue_ref {
        return Err(denied());
    }
    let (authority, authority_digest): (Value,String) = sqlx::query_as(
        "SELECT authority,authority_digest FROM work_queue_item_runtime_admission WHERE item=$1 AND policy_revision=$2 FOR SHARE",
    ).bind(item.id).bind(policy).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
    if hash(&authority)? != authority_digest {
        return Err(denied());
    }
    let admitted: crate::runtime_authority::SelectedRuntimeAuthority =
        serde_json::from_value(authority)?;
    if admitted.policy_id != policy
        || queue.pack != Some(admitted.owner.pack_id)
        || queue.pack_ref.as_deref() != Some(admitted.owner.pack_ref.as_str())
    {
        return Err(denied());
    }
    item.payload_binding()?;
    let (metadata, template_digest): (Value,String) = sqlx::query_as(
        "SELECT metadata,metadata_digest FROM managed_dispatch_metadata_revision WHERE policy_revision=$1 AND component_kind='queues' AND component_id=$2 FOR SHARE",
    ).bind(policy).bind(queue.id).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
    if hash(&metadata)? != template_digest {
        return Err(denied());
    }
    let invocation = crate::runtime_authority::InvocationBinding::Queue {
        queue_ref: queue.r#ref.clone(),
        action_ref: snapshot.executable.action.r#ref.clone(),
    };
    let mut declarations = Vec::new();
    let sites: Vec<(String,String)> = sqlx::query_as(
        "SELECT declaration->>'source_path',declaration->>'expression' FROM runtime_declared_input_binding WHERE policy_revision=$1 AND invocation_digest=$2 AND declaration->>'source_path' LIKE '/queue/action_params/%' ORDER BY declaration->>'source_path'",
    ).bind(policy).bind(hash(&serde_json::to_value(&invocation)?)?).fetch_all(&mut *conn).await?;
    for (path, expression) in sites {
        declarations.push(
            DeclaredInputRepository::declaration_for_policy(
                conn,
                policy,
                &invocation,
                &template_digest,
                &path,
                &expression,
            )
            .await?
            .1,
        );
    }
    let contributors:Vec<(String,Value,String)>=sqlx::query_as("SELECT contributor,authority,authority_digest FROM (SELECT ''::TEXT AS contributor,authority,authority_digest FROM recorded_execution_input_authority WHERE owner_kind='queue_item' AND owner_id=$1 UNION ALL SELECT contributor,authority,authority_digest FROM recorded_queue_item_input WHERE item=$1) roots ORDER BY contributor")
        .bind(item.id).fetch_all(&mut *conn).await?;
    let mut contributor_digests = BTreeMap::new();
    if contributors.is_empty() || contributors.len() > 256 {
        return Err(denied());
    }
    for (name, body, expected) in contributors {
        super::recorded_input_authority::RecordedInputAuthorityRepository::from_manifest(
            &body, &expected,
        )?;
        if contributor_digests.insert(name, expected).is_some() {
            return Err(denied());
        }
    }
    let schema = SchemaContractRepository::materialize_action_pair(conn, snapshot).await?;
    Ok((
        Binding {
            id: 0,
            item: item.id,
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            policy,
            authority_digest,
            authority: admitted,
            contributor_digests,
            source_execution: item.requested_by_execution,
            source_receipts: Vec::new(),
            executable_digest: hash(&serde_json::to_value(snapshot)?)?,
            item_schema_digest: hash(&item.schema_binding.clone().ok_or_else(denied)?)?,
            template_digest,
            pack: queue.pack.ok_or_else(denied)?,
            configuration: None,
            configuration_digest: None,
            declarations,
            keys: BTreeMap::new(),
            key_targets: BTreeMap::new(),
            key_versions: BTreeMap::new(),
            schema,
        },
        metadata,
    ))
}

async fn validate_transfer_fields(
    conn: &mut PgConnection,
    sealed: &Sealed,
    fields: &[crate::secret_values::PreparedSecretValue],
    master: &str,
) -> Result<Vec<QueueReceiptParent>> {
    validate_transfer_ids(sealed, master)?;
    if fields.len() > 256 {
        return Err(denied());
    }
    let mut parents = Vec::new();
    let mut paths = std::collections::BTreeSet::new();
    for transfer in &sealed.transfers {
        if !paths.insert(transfer.path())
            || fields
                .iter()
                .filter(|field| field.json_path == transfer.path())
                .count()
                != 1
            || fields
                .iter()
                .find(|field| field.json_path == transfer.path())
                != Some(transfer.field())
        {
            return Err(denied());
        }
        parents.push(transfer.validate(conn, &sealed.binding, master).await?);
    }
    // Neither missing nor additional typed source fields can borrow an
    // admitted transfer for a different selector or protected destination.
    for field in fields {
        let provenance: crate::secret_provenance::SecretProvenance =
            serde_json::from_str(field.source_ref.as_deref().ok_or_else(denied)?)?;
        if provenance
            .origins
            .iter()
            .any(super::immutable_input_origin::is_input_receipt)
            && !paths.contains(field.json_path.as_str())
        {
            return Err(denied());
        }
    }
    Ok(parents)
}

/// Offline maintenance authenticates this new, current-verifier-only store
/// without reviving a revoked input permit or changing any accepted bytes.
pub(in crate::repositories) fn authenticate_stored(row: &Value, master: &str) -> Result<()> {
    let body = row.get("binding").ok_or_else(denied)?;
    if row.get("binding_digest").and_then(Value::as_str) != Some(hash(body)?.as_str())
        || row.get("encryption_key_hash").and_then(Value::as_str)
            != Some(crypto::hash_encryption_key(master).as_str())
    {
        return Err(denied());
    }
    let sealed: Sealed = serde_json::from_value(crypto::decrypt_json(
        row.get("encrypted_context").ok_or_else(denied)?,
        master,
    )?)?;
    if serde_json::to_value(&sealed.binding)? != *body
        || row.get("id").and_then(Value::as_i64) != Some(sealed.binding.id)
        || row.get("item").and_then(Value::as_i64) != Some(sealed.binding.item)
        || row.get("policy_revision").and_then(Value::as_i64) != Some(sealed.binding.policy)
        || row.get("configuration_revision").and_then(Value::as_i64) != sealed.binding.configuration
        || row.get("source_receipts")
            != Some(&serde_json::to_value(&sealed.binding.source_receipts)?)
        || sealed.keys.keys().ne(sealed.binding.keys.keys())
    {
        return Err(denied());
    }
    if hash(&serde_json::to_value(&sealed.binding.authority)?)? != sealed.binding.authority_digest
        || sealed.binding.contributor_digests.is_empty()
        || sealed.binding.id <= 0
        || sealed.binding.item <= 0
        || sealed.binding.authority.policy_id != sealed.binding.policy
        || sealed.binding.authority.owner.pack_id != sealed.binding.pack
        || sealed
            .binding
            .keys
            .keys()
            .ne(sealed.binding.key_versions.keys())
        || sealed
            .binding
            .key_versions
            .iter()
            .any(|(reference, version)| {
                version.revision <= 0
                    || sealed
                        .binding
                        .keys
                        .get(reference)
                        .is_none_or(|origin| origin.key_id != version.key)
            })
        || sealed
            .binding
            .keys
            .keys()
            .ne(sealed.binding.key_targets.keys())
    {
        return Err(denied());
    }
    sealed.binding.schema.validate()?;
    validate_transfer_ids(&sealed, master)?;
    Ok(())
}

fn validate_transfer_ids(sealed: &Sealed, master: &str) -> Result<()> {
    if sealed.transfers.len() > 256
        || sealed
            .transfers
            .windows(2)
            .any(|pair| pair[0].path() >= pair[1].path())
    {
        return Err(denied());
    }
    for transfer in &sealed.transfers {
        transfer.validate_shape(master)?;
    }
    let ids = sealed
        .transfers
        .iter()
        .map(|transfer| transfer.receipt())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if ids != sealed.binding.source_receipts {
        return Err(denied());
    }
    Ok(())
}

struct FixedContext<'a>(&'a Value);
impl EvalContext for FixedContext<'_> {
    fn resolve_variable(&self, name: &str) -> EvalResult<Value> {
        match name {
            "config" => Ok(self.0.clone()),
            "pack" => Ok(json!({"config":self.0})),
            _ => Err(EvalError::VariableNotFound(name.into())),
        }
    }
    fn call_workflow_function(&self, _name: &str, _args: &[Value]) -> EvalResult<Option<Value>> {
        Ok(None)
    }
}
fn fixed_key_references(params: &Value, configuration: &Value) -> Result<Vec<String>> {
    let mut found = std::collections::BTreeSet::new();
    let mut pending = vec![params];
    let mut count = 0;
    while let Some(value) = pending.pop() {
        count += 1;
        if count > 16384 {
            return Err(denied());
        }
        match value {
            Value::String(value) => {
                let mut remaining = value.as_str();
                while let Some(open) = remaining.find("{{") {
                    let rest = &remaining[open + 2..];
                    let close = rest.find("}}").ok_or_else(denied)?;
                    found.extend(
                        expression::keystore_references(
                            rest[..close].trim(),
                            &FixedContext(configuration),
                        )
                        .map_err(|_| Error::PermissionDenied(
                            "Managed queue fixed Key selectors must resolve exclusively from admitted configuration; record-dependent or unavailable selectors are not allowed".into()
                        ))?,
                    );
                    remaining = &rest[close + 2..];
                }
            }
            Value::Array(values) => pending.extend(values),
            Value::Object(values) => pending.extend(values.values()),
            _ => (),
        }
    }
    if found.len() > 256 {
        return Err(Error::PermissionDenied(
            "Managed queue fixed Key selectors exceed the admission bound".into(),
        ));
    }
    Ok(found.into_iter().collect())
}
fn hash(value: &Value) -> Result<String> {
    digest(value).map_err(|_| denied())
}
fn denied() -> Error {
    Error::PermissionDenied(
        "Accepted queue configuration input binding is unavailable or mismatched".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_selectors_use_configuration_but_never_record_contributor_data() {
        let params = json!({"credential":"{{ keystore[config.credential] }}", "record":"{{ item.message }}"});
        assert_eq!(
            fixed_key_references(&params, &json!({"credential":"business.credential"})).unwrap(),
            vec!["business.credential"]
        );
        assert!(fixed_key_references(
            &json!({"credential":"{{ keystore[item.credential] }}"}),
            &json!({})
        )
        .is_err());
        assert!(fixed_key_references(
            &json!({"credential":"{{ keystore[queue_items[0].payload.credential] }}"}),
            &json!({})
        )
        .is_err());
        assert!(fixed_key_references(
            &json!({"credential":"{{ keystore[config.missing] }}"}),
            &json!({})
        )
        .is_err());
    }
}
