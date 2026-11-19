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

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    item: Id,
    queue: Id,
    queue_ref: String,
    policy: Id,
    authority_digest: String,
    executable_digest: String,
    item_schema_digest: String,
    template_digest: String,
    pack: Id,
    configuration: Option<Id>,
    configuration_digest: Option<String>,
    declarations: Vec<DeclaredInputBinding>,
    keys: BTreeMap<String, KeyOrigin>,
    schema: ActionSchemaBinding,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sealed {
    binding: Binding,
    configuration: Value,
    keys: BTreeMap<String, Value>,
}

/// Constructed only by validating an immutable owning record and active source
/// fences. No serialized caller body can assert this proof.
pub struct QueueConfigurationInput {
    sealed: Sealed,
}
impl QueueConfigurationInput {
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
    /// Capture only during ORIGINAL item/admission creation. Repeating this call
    /// reads the accepted ciphertext, never today's Key/configuration value.
    pub async fn capture(
        conn: &mut PgConnection,
        queue: &WorkQueue,
        item: &WorkQueueItem,
        snapshot: &ExecutionExecutableSnapshot,
        encryption_key: &str,
    ) -> Result<QueueConfigurationInput> {
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM work_queue_item_configuration_input WHERE item=$1)",
        )
        .bind(item.id)
        .fetch_one(&mut *conn)
        .await?
        {
            return Self::load(conn, queue, item, snapshot, encryption_key).await;
        }
        let original: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM work_queue_item i JOIN work_queue_item_runtime_admission a ON a.item=i.id \
             WHERE i.id=$1 AND i.xmin::text::numeric=mod(pg_current_xact_id()::text::numeric,4294967296) \
             AND a.xmin::text::numeric=mod(pg_current_xact_id()::text::numeric,4294967296))",
        ).bind(item.id).fetch_one(&mut *conn).await?;
        if !original {
            return Err(denied());
        }
        let selected =
            Admission::queue_batch(conn, queue, std::slice::from_ref(item), snapshot).await?;
        let (mut binding, metadata) =
            binding(conn, queue, item, snapshot, selected.authority().policy_id).await?;
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
        };
        let body = serde_json::to_value(&sealed.binding)?;
        let plaintext = serde_json::to_value(&sealed)?;
        if serde_json::to_vec(&plaintext)?.len() > 1024 * 1024 {
            return Err(denied());
        }
        // The authenticated encrypted envelope includes the complete public
        // binding, preventing ciphertext substitution across items or policies.
        let ciphertext = crypto::encrypt_json(&plaintext, encryption_key)?;
        sqlx::query("INSERT INTO work_queue_item_configuration_input(item,policy_revision,configuration_revision,binding,binding_digest,encrypted_context,encryption_key_hash) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(item.id).bind(sealed.binding.policy).bind(sealed.binding.configuration).bind(&body).bind(hash(&body)?)
            .bind(ciphertext).bind(crypto::hash_encryption_key(encryption_key)).execute(&mut *conn).await?;
        selected.ensure_fresh_on(conn).await?;
        Ok(QueueConfigurationInput { sealed })
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
        let row: Option<(Id,Option<Id>,Value,String,Value,String)> = sqlx::query_as(
            "SELECT policy_revision,configuration_revision,binding,binding_digest,encrypted_context,encryption_key_hash FROM work_queue_item_configuration_input WHERE item=$1 FOR SHARE",
        ).bind(item.id).fetch_optional(&mut *conn).await?;
        let (policy, configuration, body, expected, encrypted, key_hash) =
            row.ok_or_else(denied)?;
        if hash(&body)? != expected || key_hash != crypto::hash_encryption_key(encryption_key) {
            return Err(denied());
        }
        let sealed: Sealed =
            serde_json::from_value(crypto::decrypt_json(&encrypted, encryption_key)?)?;
        if serde_json::to_value(&sealed.binding)? != body
            || sealed.binding.policy != policy
            || sealed.binding.configuration != configuration
            || policy != selected.authority().policy_id
        {
            return Err(denied());
        }
        let (mut actual, _) = binding(conn, queue, item, snapshot, policy).await?;
        actual.configuration = sealed.binding.configuration;
        actual.configuration_digest = sealed.binding.configuration_digest.clone();
        actual.keys = sealed.binding.keys.clone();
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
        selected.ensure_fresh_on(conn).await?;
        Ok(QueueConfigurationInput { sealed })
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
    let schema = SchemaContractRepository::materialize_action_pair(conn, snapshot).await?;
    Ok((
        Binding {
            item: item.id,
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            policy,
            authority_digest,
            executable_digest: hash(&serde_json::to_value(snapshot)?)?,
            item_schema_digest: hash(&item.schema_binding.clone().ok_or_else(denied)?)?,
            template_digest,
            pack: admitted.owner.pack_id,
            configuration: None,
            configuration_digest: None,
            declarations,
            keys: BTreeMap::new(),
            schema,
        },
        metadata,
    ))
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
