//! Purpose authority for immutable authored selectors. This module never returns
//! runtime/API grants or a general-purpose human delegation authority.

use super::{
    managed_executable::ManagedExecutableRepository,
    runtime_authority::{
        ApprovalEffectFence, RuntimeAdmissionRecord, RuntimeAuthorityRepository, RuntimeOwner,
    },
    runtime_target::{ResolvedRuntimeTarget, RuntimeTarget, RuntimeTargetRepository},
};
use crate::{
    models::{ActionExecutableSnapshot, Id},
    rbac::{Action, Resource},
    secret_provenance::SecretOrigin,
    value_schema::digest,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;
use std::sync::LazyLock;

static KEY_LOOKUP: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{\s*keystore(?:\.|\[)").expect("authored Key lookup regex")
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredInputBinding {
    pub action: Id,
    pub pack: Id,
    pub release: Id,
    pub metadata_revision: Option<Id>,
    /// Digest of the complete immutable action/graph metadata, not the text alone.
    pub declaration_revision: String,
    /// `/param_schema/.../default/...` or `/workflow/...` in the pinned metadata.
    pub source_path: String,
    pub expression: String,
}

impl DeclaredInputBinding {
    fn bare_workflow_condition(&self) -> bool {
        self.source_path.starts_with("/workflow/tasks/")
            && self.source_path.ends_with("/when")
            && !self.expression.contains("{{")
    }
    pub fn uses_pack_configuration(&self) -> bool {
        crate::template_resolver::references_pack_configuration(&Value::String(
            self.expression.clone(),
        )) || self.bare_workflow_condition()
            && crate::workflow::expression::uses_namespace(&self.expression, "config")
                .unwrap_or(false)
            || self.source_path.starts_with("/workflow/")
                && crate::template_resolver::workflow_template_uses_namespace(
                    &self.expression,
                    "config",
                )
    }
    pub(crate) fn uses_keystore(&self) -> bool {
        if self.bare_workflow_condition() {
            crate::workflow::expression::uses_namespace_access(&self.expression, "keystore")
                .unwrap_or(false)
        } else if self.source_path.starts_with("/workflow/") {
            crate::template_resolver::workflow_template_uses_lookup(&self.expression, "keystore")
        } else {
            KEY_LOOKUP.is_match(&self.expression)
        }
    }
}

/// Only repository validation constructs this permit. It cannot be serialized,
/// forged from grants, or used as an execution credential.
pub struct DeclaredInputPermit {
    pub(crate) binding_id: Id,
    pub(crate) source: RuntimeAdmissionRecord,
    pub(crate) declaration: DeclaredInputBinding,
    pub(crate) effect_fence: ApprovalEffectFence,
}

impl DeclaredInputPermit {
    pub async fn ensure_fresh_on(&self, connection: &mut PgConnection) -> Result<()> {
        self.effect_fence.ensure_fresh_on(connection).await
    }
    pub fn declaration(&self) -> &DeclaredInputBinding {
        &self.declaration
    }

    pub fn allows_target(&self, target: &ResolvedRuntimeTarget, action: Action) -> bool {
        if self.effect_fence.ensure_fresh().is_err() {
            return false;
        }
        (match target.resource() {
            Resource::Keys => self.declaration.uses_keystore(),
            Resource::Packs => self.declaration.uses_pack_configuration(),
            _ => false,
        }) && matches!(action, Action::Read | Action::Decrypt)
            && target.pack_id() == Some(self.declaration.pack)
    }
}

pub struct DeclaredInputRepository;

/// Read-only authenticated target metadata. It is not a render/input permit.
pub struct InputApprovalTarget {
    policy: Id,
    binding: crate::runtime_authority::InvocationBinding,
    declaration: DeclaredInputBinding,
}
impl InputApprovalTarget {
    pub fn policy_revision(&self) -> Id {
        self.policy
    }
    pub fn invocation_binding(&self) -> &crate::runtime_authority::InvocationBinding {
        &self.binding
    }
    pub fn declaration(&self) -> &DeclaredInputBinding {
        &self.declaration
    }
}

impl DeclaredInputRepository {
    pub async fn approval_is_input_only(
        connection: &mut PgConnection,
        approval: Id,
    ) -> Result<bool> {
        RuntimeAuthorityRepository::find_approval(connection, approval)
            .await?
            .ok_or_else(denied)?;
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_input_only_approval WHERE approval = $1)",
        )
        .bind(approval)
        .fetch_one(connection)
        .await?)
    }

    /// Caller supplies its independently authorized approval subject. Expired,
    /// revoked and retired metadata remains inspectable without consuming it.
    pub async fn approval_targets(
        connection: &mut PgConnection,
        approval: Id,
    ) -> Result<Vec<InputApprovalTarget>> {
        if !Self::approval_is_input_only(connection, approval).await? {
            return Ok(vec![]);
        }
        let subject = RuntimeAuthorityRepository::find_approval(connection, approval)
            .await?
            .ok_or_else(denied)?
            .subject_pack;
        let rows:Vec<(Id,String,Value,String)>=sqlx::query_as("SELECT binding.policy_revision,binding.invocation_digest,binding.declaration,binding.declaration_digest FROM runtime_declared_input_approval approved JOIN runtime_declared_input_binding binding ON binding.id = approved.declaration WHERE approved.approval = $1 ORDER BY binding.policy_revision,binding.id LIMIT 257")
            .bind(approval).fetch_all(&mut *connection).await?;
        if rows.len() > 256 {
            return Err(denied());
        }
        let mut result = Vec::new();
        for (policy, invocation, body, expected) in rows {
            if hash(&body)? != expected {
                return Err(denied());
            }
            let record = RuntimeAuthorityRepository::find_policy(connection, policy)
                .await?
                .ok_or_else(denied)?;
            let mut matching = record.policy.invocations.iter().filter(|entry| {
                serde_json::to_value(&entry.binding)
                    .ok()
                    .and_then(|body| hash(&body).ok())
                    .as_deref()
                    == Some(invocation.as_str())
            });
            let binding = matching.next().ok_or_else(denied)?.binding.clone();
            if matching.next().is_some() {
                return Err(denied());
            }
            let declaration: DeclaredInputBinding = serde_json::from_value(body)?;
            if declaration.pack != subject {
                return Err(denied());
            }
            result.push(InputApprovalTarget {
                policy,
                binding,
                declaration,
            });
        }
        Ok(result)
    }
    /// Resolve an authored declaration BEFORE any execution exists. This is
    /// immutable policy metadata, not an invocation permit or execution grant.
    pub async fn declaration_for_policy(
        connection: &mut PgConnection,
        policy_revision: Id,
        binding: &crate::runtime_authority::InvocationBinding,
        revision: &str,
        path: &str,
        expression: &str,
    ) -> Result<(Id, DeclaredInputBinding)> {
        let policy =
            RuntimeAuthorityRepository::require_policy(connection, policy_revision).await?;
        let invocation = policy
            .policy
            .invocations
            .iter()
            .find(|entry| &entry.binding == binding)
            .ok_or_else(denied)?;
        let snapshot =
            ManagedExecutableRepository::invocation_snapshot(connection, invocation).await?;
        let actual = declarations(
            &snapshot.executable,
            snapshot.release.id,
            invocation.metadata_revision,
        )?;
        let rows:Vec<(Id,Value,String)>=sqlx::query_as("SELECT id,declaration,declaration_digest FROM runtime_declared_input_binding WHERE policy_revision = $1 AND invocation_digest = $2 AND declaration->>'source_path' = $3 FOR SHARE")
            .bind(policy_revision).bind(hash(&serde_json::to_value(binding)?)?).bind(path).fetch_all(&mut *connection).await?;
        let mut matched = None;
        for (id, body, expected) in rows {
            if hash(&body)? != expected {
                return Err(denied());
            }
            let declaration: DeclaredInputBinding = serde_json::from_value(body)?;
            if declaration.declaration_revision == revision && declaration.expression == expression
            {
                if matched.is_some() || !actual.contains(&declaration) {
                    return Err(denied());
                }
                matched = Some((id, declaration));
            }
        }
        let (id, declaration) = matched.ok_or_else(denied)?;
        let owner = RuntimeAuthorityRepository::find_owner(connection, declaration.pack)
            .await?
            .ok_or_else(denied)?;
        super::runtime_authority::lock_sponsor(connection, &owner).await?;
        Ok((id, declaration))
    }
    /// Discover executable-owned fields, not user values. Renderers should walk
    /// these fields in the pinned declaration and leave supplied strings literal.
    pub async fn bindings(
        connection: &mut PgConnection,
        execution: Id,
    ) -> Result<Vec<DeclaredInputBinding>> {
        let source = Self::require_source(connection, execution).await?;
        let invocation = hash(&serde_json::to_value(&source.authority.binding)?)?;
        let rows:Vec<(Value,String)> = sqlx::query_as("SELECT declaration,declaration_digest FROM runtime_declared_input_binding WHERE policy_revision = $1 AND invocation_digest = $2 ORDER BY id FOR SHARE")
            .bind(source.policy_revision).bind(invocation).fetch_all(connection).await?;
        rows.into_iter()
            .map(|(value, expected)| {
                if hash(&value)? != expected {
                    return Err(denied());
                }
                serde_json::from_value(value).map_err(Into::into)
            })
            .collect()
    }

    /// Capture from repository-pinned executable metadata only. No supplied
    /// execution parameters, invoker rights or selected API grants enter here.
    pub(crate) async fn capture(
        connection: &mut PgConnection,
        policy: &super::runtime_authority::RuntimePolicyRevision,
    ) -> Result<()> {
        for invocation in &policy.policy.invocations {
            let executable =
                ManagedExecutableRepository::invocation_snapshot(connection, invocation).await?;
            let declarations = declarations(
                &executable.executable,
                executable.release.id,
                invocation.metadata_revision,
            )?;
            let invocation_digest = hash(&serde_json::to_value(&invocation.binding)?)?;
            for declaration in declarations {
                let value = serde_json::to_value(&declaration)?;
                sqlx::query("INSERT INTO runtime_declared_input_binding (policy_revision,invocation_digest,declaration,declaration_digest) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
                    .bind(policy.id).bind(&invocation_digest).bind(&value).bind(hash(&value)?).execute(&mut *connection).await?;
            }
        }
        Ok(())
    }

    /// The caller must identify the immutable authored field being evaluated.
    /// Never call this for supplied parameter strings, nor recursively evaluate
    /// strings produced by a template. Exact expression/path/revision must match.
    pub async fn permit(
        connection: &mut PgConnection,
        execution: Id,
        declaration_revision: &str,
        source_path: &str,
        expression: &str,
    ) -> Result<DeclaredInputPermit> {
        let source = Self::require_source(connection, execution).await?;
        let invocation_digest = hash(&serde_json::to_value(&source.authority.binding)?)?;
        let rows:Vec<(Id,Value,String)> = sqlx::query_as("SELECT id,declaration,declaration_digest FROM runtime_declared_input_binding WHERE policy_revision = $1 AND invocation_digest = $2 AND declaration->>'source_path' = $3 FOR SHARE")
            .bind(source.policy_revision).bind(invocation_digest).bind(source_path).fetch_all(&mut *connection).await?;
        let mut matched = None;
        for (id, value, expected) in rows {
            if hash(&value)? != expected {
                return Err(denied());
            }
            let declaration: DeclaredInputBinding = serde_json::from_value(value)?;
            if declaration.declaration_revision == declaration_revision
                && declaration.expression == expression
            {
                if matched.is_some() {
                    return Err(denied());
                }
                matched = Some((id, declaration));
            }
        }
        let (binding_id, declaration) = matched.ok_or_else(denied)?;
        Self::validate_declaration(connection, &source, &declaration).await?;
        // A foreign action's defaults remain owned by its author, not the outer
        // invoker. A suspended/deleted author pack cannot deliver inputs.
        let owner = RuntimeAuthorityRepository::find_owner(connection, declaration.pack)
            .await?
            .ok_or_else(denied)?;
        super::runtime_authority::lock_sponsor(connection, &owner).await?;
        let effect_fence =
            RuntimeAuthorityRepository::retained_admission_fence(connection, source.id).await?;
        Ok(DeclaredInputPermit {
            binding_id,
            source,
            declaration,
            effect_fence,
        })
    }

    pub(crate) async fn require_source(
        connection: &mut PgConnection,
        execution: Id,
    ) -> Result<RuntimeAdmissionRecord> {
        let source = RuntimeAuthorityRepository::execution_admission(connection, execution)
            .await?
            .ok_or_else(denied)?;
        let source = RuntimeAuthorityRepository::record_admission(
            connection,
            RuntimeOwner::Execution(execution),
            &source.authority,
        )
        .await?;
        let eligible:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution WHERE id = $1 AND status IN ('requested','scheduling','scheduled','running'))")
            .bind(execution).fetch_one(&mut *connection).await?;
        if !eligible {
            return Err(denied());
        }
        Ok(source)
    }

    pub(crate) async fn validate_declaration(
        connection: &mut PgConnection,
        source: &RuntimeAdmissionRecord,
        declaration: &DeclaredInputBinding,
    ) -> Result<()> {
        let policy =
            RuntimeAuthorityRepository::require_policy(connection, source.policy_revision).await?;
        let invocation = policy
            .policy
            .invocations
            .iter()
            .find(|entry| entry.binding == source.authority.binding)
            .ok_or_else(denied)?;
        let executable =
            ManagedExecutableRepository::invocation_snapshot(connection, invocation).await?;
        if !declarations(
            &executable.executable,
            executable.release.id,
            invocation.metadata_revision,
        )?
        .contains(declaration)
        {
            return Err(denied());
        }
        let owner = RuntimeAuthorityRepository::find_owner(connection, declaration.pack)
            .await?
            .ok_or_else(denied)?;
        super::runtime_authority::lock_sponsor(connection, &owner).await
    }

    async fn validate_permit(
        connection: &mut PgConnection,
        permit: &DeclaredInputPermit,
    ) -> Result<()> {
        let source = Self::require_source(connection, permit.source.owner_id).await?;
        if source.id != permit.source.id {
            return Err(denied());
        }
        Self::validate_declaration(connection, &source, &permit.declaration).await
    }

    /// C resolves Keys on this same transaction, checks the permit against the
    /// opaque target, restores with the original Key/schema binding, then records
    /// its actual origin before persisting delivered input. No enumeration API.
    pub async fn authorize_key(
        connection: &mut PgConnection,
        permit: &DeclaredInputPermit,
        key: Id,
        encrypted: bool,
    ) -> Result<()> {
        Self::validate_permit(connection, permit).await?;
        let target = RuntimeTargetRepository::resolve(
            connection,
            RuntimeTarget::Key(key),
            permit.source.authority.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        if !permit.allows_target(&target, Action::Read)
            || encrypted && !permit.allows_target(&target, Action::Decrypt)
        {
            return Err(denied());
        }
        permit.effect_fence.ensure_fresh_on(connection).await?;
        Ok(())
    }

    /// Configuration is resolved under the declaration's pack scope. Restoration
    /// uses D's immutable binding and returns its revision for origin retention.
    pub async fn pack_configuration(
        connection: &mut PgConnection,
        permit: &DeclaredInputPermit,
        pack: Id,
        encryption_key: Option<&str>,
    ) -> Result<(Value, Id)> {
        Self::validate_permit(connection, permit).await?;
        let target = RuntimeTargetRepository::resolve(
            connection,
            RuntimeTarget::Pack(pack),
            permit.source.authority.owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        if !permit.allows_target(&target, Action::Read)
            || !permit.allows_target(&target, Action::Decrypt)
        {
            return Err(denied());
        }
        let revision = super::component_configuration::ComponentConfigurationRepository::current(
            &mut *connection,
            "pack",
            pack,
        )
        .await?
        .ok_or_else(denied)?;
        if !super::component_configuration::ComponentConfigurationRepository::owner_survives(
            connection, &revision,
        )
        .await?
        {
            return Err(denied());
        }
        let value = super::component_configuration::ComponentConfigurationRepository::restore(
            connection,
            &revision,
            encryption_key,
        )
        .await?;
        permit.effect_fence.ensure_fresh_on(connection).await?;
        Ok((value, revision.id))
    }

    /// Persist together with the destination's encrypted value/provenance. The
    /// receipt is input-purpose evidence only, never SDK/API access authority.
    pub async fn record_delivery(
        connection: &mut PgConnection,
        permit: &DeclaredInputPermit,
        execution: Id,
        destination_path: &str,
        origin: &SecretOrigin,
    ) -> Result<Id> {
        Self::validate_permit(connection, permit).await?;
        let destination = Self::require_source(connection, execution).await?;
        let destination_fence =
            RuntimeAuthorityRepository::retained_admission_fence(connection, destination.id)
                .await?;
        if destination.id != permit.source.id {
            let ancestor:bool = sqlx::query_scalar("WITH RECURSIVE lineage(id) AS (SELECT dependency FROM runtime_admission_dependency WHERE admission = $1 UNION SELECT edge.dependency FROM runtime_admission_dependency edge JOIN lineage ON edge.admission = lineage.id) SELECT EXISTS(SELECT 1 FROM lineage WHERE id = $2)")
                .bind(destination.id).bind(permit.source.id).fetch_one(&mut *connection).await?;
            if !ancestor {
                return Err(denied());
            }
        }
        Self::check_origin(connection, permit, origin).await?;
        let value = serde_json::to_value(origin)?;
        let hash = hash(&value)?;
        let id:Id = sqlx::query_scalar("INSERT INTO runtime_declared_input_receipt (execution,destination_path,source_admission,declaration,origin,origin_digest) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (execution,destination_path,source_admission,declaration,origin_digest) DO UPDATE SET execution = EXCLUDED.execution RETURNING id")
            .bind(execution).bind(destination_path).bind(permit.source.id).bind(permit.binding_id).bind(value).bind(hash).fetch_one(&mut *connection).await?;
        permit
            .effect_fence
            .intersect(&destination_fence)
            .ensure_fresh_on(connection)
            .await?;
        Ok(id)
    }

    async fn check_origin(
        connection: &mut PgConnection,
        permit: &DeclaredInputPermit,
        origin: &SecretOrigin,
    ) -> Result<()> {
        match origin {
            SecretOrigin::Key(key) => {
                Self::authorize_key(connection, permit, key.key_id, key.encrypted).await?;
                use super::FindById;
                let actual = super::key::KeyRepository::find_by_id(&mut *connection, key.key_id)
                    .await?
                    .ok_or_else(denied)?;
                if crate::key_access::key_origin(&actual) != *key {
                    return Err(denied());
                }
                Ok(())
            }
            SecretOrigin::Entity {
                entity_type,
                entity_id,
                ..
            } if entity_type == "component_configuration" => {
                if !permit.declaration.uses_pack_configuration() {
                    return Err(denied());
                }
                let revision =
                    super::component_configuration::ComponentConfigurationRepository::find(
                        &mut *connection,
                        *entity_id,
                    )
                    .await?
                    .ok_or_else(denied)?;
                if revision.owner_kind != "pack" || revision.owner_id != permit.declaration.pack
                    || !super::component_configuration::ComponentConfigurationRepository::owner_survives(connection,&revision).await? { return Err(denied()); }
                Ok(())
            }
            _ => Err(denied()),
        }
    }

    /// C's worker/restoration hook checks the exact destination field and origin.
    /// An authored receipt is an alternative ONLY for that delivered origin,
    /// not a replacement for contributor authority on caller-supplied inputs.
    pub async fn delivery_allowed(
        connection: &mut PgConnection,
        execution: Id,
        destination_path: &str,
        origin: &SecretOrigin,
    ) -> Result<bool> {
        let destination = Self::require_source(connection, execution).await?;
        let destination_fence =
            RuntimeAuthorityRepository::retained_admission_fence(connection, destination.id)
                .await?;
        let value = serde_json::to_value(origin)?;
        let rows:Vec<(Id,Id,Id,Value,String)> = sqlx::query_as("SELECT receipt.declaration,receipt.source_admission,binding.policy_revision,binding.declaration,binding.declaration_digest FROM runtime_declared_input_receipt receipt JOIN runtime_declared_input_binding binding ON binding.id = receipt.declaration WHERE receipt.execution = $1 AND receipt.destination_path = $2 AND receipt.origin_digest = $3 AND receipt.origin = $4 FOR SHARE OF receipt,binding")
            .bind(execution).bind(destination_path).bind(hash(&value)?).bind(value).fetch_all(&mut *connection).await?;
        for (binding_id, source_id, policy_id, value, expected) in rows {
            if hash(&value)? != expected {
                return Err(denied());
            }
            let source =
                RuntimeAuthorityRepository::require_retained_admission(connection, source_id)
                    .await?;
            let effect_fence =
                RuntimeAuthorityRepository::retained_admission_fence(connection, source.id)
                    .await?
                    .intersect(&destination_fence);
            if source.policy_revision != policy_id {
                return Err(denied());
            }
            let declaration: DeclaredInputBinding = serde_json::from_value(value)?;
            Self::validate_declaration(connection, &source, &declaration).await?;
            let owner = RuntimeAuthorityRepository::find_owner(connection, declaration.pack)
                .await?
                .ok_or_else(denied)?;
            super::runtime_authority::lock_sponsor(connection, &owner).await?;
            let permit = DeclaredInputPermit {
                binding_id,
                source,
                declaration,
                effect_fence,
            };
            // Historical source execution may already be completed/retained
            // away. Revalidate immutable origin ownership without new delivery.
            if let SecretOrigin::Key(key) = origin {
                let target = RuntimeTargetRepository::resolve(
                    connection,
                    RuntimeTarget::Key(key.key_id),
                    permit.source.authority.owner.sponsor_identity_id,
                )
                .await?
                .ok_or_else(denied)?;
                if !permit.allows_target(&target, Action::Read)
                    || key.encrypted && !permit.allows_target(&target, Action::Decrypt)
                {
                    continue;
                }
                use super::FindById;
                let actual = super::key::KeyRepository::find_by_id(&mut *connection, key.key_id)
                    .await?
                    .ok_or_else(denied)?;
                if crate::key_access::key_origin(&actual) == *key {
                    permit.effect_fence.ensure_fresh_on(connection).await?;
                    return Ok(true);
                }
            } else if let SecretOrigin::Entity {
                entity_type,
                entity_id,
                ..
            } = origin
            {
                if entity_type != "component_configuration" {
                    continue;
                }
                if !permit.declaration.uses_pack_configuration() {
                    continue;
                }
                let revision =
                    super::component_configuration::ComponentConfigurationRepository::find(
                        &mut *connection,
                        *entity_id,
                    )
                    .await?
                    .ok_or_else(denied)?;
                if revision.owner_kind == "pack" && revision.owner_id == permit.declaration.pack
                    && super::component_configuration::ComponentConfigurationRepository::owner_survives(connection,&revision).await? {
                        permit.effect_fence.ensure_fresh_on(connection).await?;
                        return Ok(true);
                    }
            }
        }
        Ok(false)
    }

    /// B copies receipts with identical-input retries in the creation transaction.
    /// First attach the retry's admission dependency. Caller values
    /// do not acquire receipts merely by mentioning a previously delivered Key.
    pub async fn copy_deliveries(
        connection: &mut PgConnection,
        source: Id,
        target: Id,
    ) -> Result<()> {
        let origin = RuntimeAuthorityRepository::execution_admission(connection, source)
            .await?
            .ok_or_else(denied)?;
        RuntimeAuthorityRepository::require_retained_admission(connection, origin.id).await?;
        let source_fence =
            RuntimeAuthorityRepository::retained_admission_fence(connection, origin.id).await?;
        let destination = Self::require_source(connection, target).await?;
        let destination_fence =
            RuntimeAuthorityRepository::retained_admission_fence(connection, destination.id)
                .await?;
        let mut expected = origin.authority.0.clone();
        expected.attempt = destination.authority.attempt;
        if expected != destination.authority.0 {
            return Err(denied());
        }
        let same_inputs:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution source JOIN execution target ON target.id = $2 WHERE source.id = $1 AND source.config IS NOT DISTINCT FROM target.config)")
            .bind(source).bind(target).fetch_one(&mut *connection).await?;
        if !same_inputs {
            return Err(denied());
        }
        let related:bool = sqlx::query_scalar("WITH RECURSIVE lineage(id) AS (SELECT dependency FROM runtime_admission_dependency WHERE admission = $1 UNION SELECT edge.dependency FROM runtime_admission_dependency edge JOIN lineage ON edge.admission = lineage.id) SELECT EXISTS(SELECT 1 FROM lineage WHERE id = $2)")
            .bind(destination.id).bind(origin.id).fetch_one(&mut *connection).await?;
        if !related {
            return Err(denied());
        }
        sqlx::query("INSERT INTO runtime_declared_input_receipt (execution,destination_path,source_admission,declaration,origin,origin_digest) SELECT $2,destination_path,source_admission,declaration,origin,origin_digest FROM runtime_declared_input_receipt WHERE execution = $1 ON CONFLICT DO NOTHING")
            .bind(source).bind(target).execute(&mut *connection).await?;
        source_fence
            .intersect(&destination_fence)
            .ensure_fresh_on(connection)
            .await
    }
}

fn denied() -> Error {
    Error::PermissionDenied("Authored input declaration or scoped origin is unavailable".into())
}
fn hash(value: &Value) -> Result<String> {
    digest(value).map_err(|_| Error::validation("Invalid authored input binding digest"))
}

fn declarations(
    snapshot: &ActionExecutableSnapshot,
    release: Id,
    metadata_revision: Option<Id>,
) -> Result<Vec<DeclaredInputBinding>> {
    let revision = hash(&serde_json::to_value(snapshot)?)?;
    let mut leaves = Vec::new();
    if let Some(fields) = snapshot
        .action
        .param_schema
        .as_ref()
        .and_then(Value::as_object)
    {
        for (field, schema) in fields {
            if let Some(default) = schema.get("default") {
                collect(
                    default,
                    &format!("/param_schema/{}/default", escape(field)),
                    &mut leaves,
                )?;
            }
        }
    }
    if let Some(workflow) = &snapshot.workflow_definition {
        collect(&workflow.definition, "/workflow", &mut leaves)?;
        collect_workflow_conditions(&workflow.definition, &mut leaves)?;
    }
    if leaves.len() > 4096 {
        return Err(Error::validation("Too many authored input declarations"));
    }
    Ok(leaves
        .into_iter()
        .map(|(source_path, expression)| DeclaredInputBinding {
            action: snapshot.action.id,
            pack: snapshot.action.pack,
            release,
            metadata_revision,
            declaration_revision: revision.clone(),
            source_path,
            expression,
        })
        .collect())
}

fn escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

/// Bare transition conditions are executable declarations too. Walk only task
/// graph structure, never a caller field named `when` inside input/vars data.
fn collect_workflow_conditions(graph: &Value, leaves: &mut Vec<(String, String)>) -> Result<()> {
    let mut pending = vec![(graph, "/workflow".to_owned(), 0usize)];
    let mut nodes = 0usize;
    while let Some((parent, prefix, depth)) = pending.pop() {
        nodes += 1;
        if depth > 32 || nodes > 16384 || leaves.len() > 4096 {
            return Err(Error::validation(
                "Workflow condition declarations exceed bounds",
            ));
        }
        let tasks: Vec<(String, &Value)> = match parent.get("tasks") {
            Some(Value::Object(tasks)) => tasks
                .iter()
                .map(|(name, task)| (escape(name), task))
                .collect(),
            Some(Value::Array(tasks)) => tasks
                .iter()
                .enumerate()
                .map(|(index, task)| (index.to_string(), task))
                .collect(),
            _ => continue,
        };
        for (name, task) in tasks {
            let path = format!("{prefix}/tasks/{name}");
            if let Some(transitions) = task.get("next").and_then(Value::as_array) {
                for (index, transition) in transitions.iter().enumerate() {
                    if let Some(expression) = transition
                        .get("when")
                        .and_then(Value::as_str)
                        .filter(|value| !value.contains("{{"))
                    {
                        leaves.push((format!("{path}/next/{index}/when"), expression.into()));
                        if leaves.len() > 4096 {
                            return Err(Error::validation("Too many authored input declarations"));
                        }
                    }
                }
            }
            pending.push((task, path, depth + 1));
        }
    }
    Ok(())
}
fn collect(value: &Value, path: &str, leaves: &mut Vec<(String, String)>) -> Result<()> {
    let mut pending = vec![(value, path.to_owned(), 0)];
    let mut visited = 0;
    while let Some((value, path, depth)) = pending.pop() {
        visited += 1;
        if depth > 32 || visited > 16384 || leaves.len() >= 4096 {
            return Err(Error::validation(
                "Authored input declaration exceeds bounds",
            ));
        }
        match value {
            Value::String(expression) if expression.contains("{{") => {
                leaves.push((path, expression.clone()))
            }
            Value::Object(values) => {
                for (key, value) in values.iter().rev() {
                    pending.push((value, format!("{path}/{}", escape(key)), depth + 1));
                }
            }
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate().rev() {
                    pending.push((value, format!("{path}/{index}"), depth + 1));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_paths_preserve_escaped_fields_and_distinct_template_sites() {
        let document = serde_json::json!({"group/name":{"~field":["{{ keystore['pack.one'] }}","literal","{{ parameters.user_string }}"]}});
        let mut bindings = Vec::new();
        collect(&document, "/workflow/tasks", &mut bindings).unwrap();
        assert_eq!(
            bindings,
            vec![
                (
                    "/workflow/tasks/group~1name/~0field/0".into(),
                    "{{ keystore['pack.one'] }}".into()
                ),
                (
                    "/workflow/tasks/group~1name/~0field/2".into(),
                    "{{ parameters.user_string }}".into()
                ),
            ]
        );
        // The supplied value is not a declaration and is never recursively
        // scanned by permit/materialization after evaluating parameters.
        assert!(!bindings
            .iter()
            .any(|(_, expression)| expression == "{{ keystore['system.user_choice'] }}"));
    }

    #[test]
    fn bare_condition_capture_follows_task_structure_not_similar_data_fields() {
        let graph = serde_json::json!({"tasks": {"group/name": {
            "input": {"when": "keystore['foreign.not_a_declaration']"},
            "next": [{"when": "config.enabled and parameters.ready"}, {"when": "{{ parameters.ready }}"}],
            "tasks": [{"name":"nested", "next":[{"when":"length(keystore['pack.token']) > 0"}]}]
        }}});
        let mut leaves = Vec::new();
        collect_workflow_conditions(&graph, &mut leaves).unwrap();
        assert_eq!(
            leaves,
            vec![
                (
                    "/workflow/tasks/group~1name/next/0/when".into(),
                    "config.enabled and parameters.ready".into()
                ),
                (
                    "/workflow/tasks/group~1name/tasks/0/next/0/when".into(),
                    "length(keystore['pack.token']) > 0".into()
                ),
            ]
        );
    }

    #[test]
    fn workflow_namespace_detection_uses_ast_not_literal_or_prefix_matches() {
        let mut site = DeclaredInputBinding {
            action: 1,
            pack: 2,
            release: 3,
            metadata_revision: None,
            declaration_revision: "a".repeat(64),
            source_path: "/workflow/vars/token".into(),
            expression: String::new(),
        };
        for expression in [
            "{{ config.token }}",
            "{{ config[parameters.field] }}",
            "{{ length(config) }}",
        ] {
            site.expression = expression.into();
            assert!(site.uses_pack_configuration(), "{expression}");
        }
        for expression in [
            "{{ parameters.config }}",
            "{{ 'config.token' }}",
            "{{ parameters.value }}",
        ] {
            site.expression = expression.into();
            assert!(!site.uses_pack_configuration(), "{expression}");
        }
        for expression in [
            "{{ length(keystore['pack.token']) }}",
            "{{ keystore[parameters.key] }}",
        ] {
            site.expression = expression.into();
            assert!(site.uses_keystore(), "{expression}");
        }
        site.expression = "{{ 'keystore[parameters.key]' }}".into();
        assert!(!site.uses_keystore());
        // Rule/config resolver namespaces do not inherit workflow `config`.
        site.source_path = "/param_schema/token/default".into();
        site.expression = "{{ config.token }}".into();
        assert!(!site.uses_pack_configuration());
    }

    #[test]
    fn declaration_digest_changes_for_field_expression_or_revision() {
        let binding = DeclaredInputBinding {
            action: 1,
            pack: 2,
            release: 3,
            metadata_revision: None,
            declaration_revision: "a".repeat(64),
            source_path: "/workflow/tasks/0/params/token".into(),
            expression: "{{ keystore['pack.one'] }}".into(),
        };
        let original = hash(&serde_json::to_value(&binding).unwrap()).unwrap();
        let mut changed = binding.clone();
        changed.source_path.push_str("/nested");
        assert_ne!(
            original,
            hash(&serde_json::to_value(changed).unwrap()).unwrap()
        );
        let mut changed = binding.clone();
        changed.expression = "{{ keystore['system.other'] }}".into();
        assert_ne!(
            original,
            hash(&serde_json::to_value(changed).unwrap()).unwrap()
        );
        let mut changed = binding;
        changed.metadata_revision = Some(9);
        assert_ne!(
            original,
            hash(&serde_json::to_value(changed).unwrap()).unwrap()
        );
    }
}
