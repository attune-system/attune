//! Read-only public approval metadata. Retained rows identify targets; fresh
//! component visibility controls disclosure, never issuance or runtime authority.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgConnection;
use utoipa::ToSchema;

use super::{
    action::ActionRepository,
    declared_input::{DeclaredInputBinding, DeclaredInputRepository},
    managed_executable::ManagedExecutableRepository,
    runtime_authority::{
        ExternalApprovalRecord, RuntimeAuthorityRepository, RuntimePolicyRevision,
    },
    runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
    FindById,
};
use crate::{
    delegation::DelegationAuthority,
    models::{ActionReferenceVisibility, Id},
    rbac::Action,
    runtime_authority::{InvocationBinding, InvocationPolicy},
    Error, Result,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeclaredInputApprovalTarget {
    pub policy_revision: Id,
    pub invocation_binding: InvocationBinding,
    pub action_id: Id,
    pub action_ref: String,
    pub pack_id: Id,
    pub pack_ref: String,
    pub release_id: Id,
    pub metadata_revision: Option<Id>,
    pub declaration_revision: String,
    pub source_path: String,
    /// Authored metadata only, never the evaluated input or source configuration.
    pub expression: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalPurpose {
    Runtime,
    DeclaredInput {
        /// Only currently visible retained targets. Empty when target rows were
        /// collected or current component visibility denies disclosure. Purpose
        /// never falls back to runtime merely because no target is visible.
        #[schema(max_items = 256)]
        targets: Vec<DeclaredInputApprovalTarget>,
    },
}

#[derive(sqlx::FromRow)]
struct DeclarationRow {
    invocation_digest: String,
    declaration: Value,
    declaration_digest: String,
}

pub struct PublicInputApprovalRepository;

impl PublicInputApprovalRepository {
    /// A bounded ledger page is enriched without changing its membership/count.
    /// Input markers come from the retained purpose table, not current policy.
    /// The caller must independently authorize Access-token subject PackRead.
    pub async fn purposes(
        connection: &mut PgConnection,
        identity: Id,
        approvals: &[ExternalApprovalRecord],
    ) -> Result<BTreeMap<Id, ApprovalPurpose>> {
        if approvals.len() > 100 {
            return Err(Error::validation(
                "Approval metadata page exceeds 100 records",
            ));
        }
        if approvals.is_empty() {
            return Ok(BTreeMap::new());
        }
        let actor = DelegationAuthority::load_for_share(connection, identity).await?;
        let mut result = BTreeMap::new();
        let mut policies = BTreeMap::new();
        for approval in approvals {
            if result.contains_key(&approval.id) {
                return Err(invalid());
            }
            let purpose = if !metadata(
                DeclaredInputRepository::approval_is_input_only(connection, approval.id).await,
            )? {
                ApprovalPurpose::Runtime
            } else {
                let retained = metadata(
                    DeclaredInputRepository::approval_targets(connection, approval.id).await,
                )?;
                let mut targets = Vec::new();
                for retained in retained {
                    let policy_id = retained.policy_revision();
                    if !policies.contains_key(&policy_id) {
                        let policy = metadata(
                            RuntimeAuthorityRepository::find_policy(connection, policy_id).await,
                        )?
                        .ok_or_else(invalid)?;
                        policies.insert(policy_id, policy);
                    }
                    let declaration = retained.declaration().clone();
                    if declaration.pack != approval.subject_pack {
                        return Err(invalid());
                    }
                    let policy = policies.get(&policy_id).ok_or_else(invalid)?;
                    let entry = policy
                        .policy
                        .invocations
                        .iter()
                        .find(|entry| &entry.binding == retained.invocation_binding())
                        .ok_or_else(invalid)?;
                    if let Some(summary) =
                        target(connection, &actor, policy, entry, declaration).await?
                    {
                        targets.push(summary);
                    }
                }
                ApprovalPurpose::DeclaredInput { targets }
            };
            result.insert(approval.id, purpose);
        }
        Ok(result)
    }

    /// Select authorized invocation digests BEFORE count/order/paging. No private
    /// expression, hidden source count or target ID enters a visible page.
    /// The caller must independently authorize Access-token subject PackRead.
    pub async fn discover(
        connection: &mut PgConnection,
        identity: Id,
        subject_pack: Id,
        policy_revision: Option<Id>,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<DeclaredInputApprovalTarget>, i64)> {
        if !(1..=100).contains(&limit) || offset < 0 || policy_revision.is_some_and(|id| id <= 0) {
            return Err(Error::validation(
                "Invalid input target page or policy revision",
            ));
        }
        let policy = match policy_revision {
            Some(id) => metadata(RuntimeAuthorityRepository::find_policy(connection, id).await)?,
            None => {
                // This pointer read is metadata, not live policy consumption.
                // Runtime suspension/expiry must not rewrite historical views.
                let revision: Option<Id> = sqlx::query_scalar(
                    "SELECT revision FROM pack_runtime_policy_current WHERE pack=$1 FOR SHARE",
                )
                .bind(subject_pack)
                .fetch_optional(&mut *connection)
                .await?;
                match revision {
                    Some(id) => {
                        metadata(RuntimeAuthorityRepository::find_policy(connection, id).await)?
                    }
                    None => None,
                }
            }
        }
        .ok_or_else(|| {
            Error::not_found(
                "input policy metadata",
                "subject pack",
                subject_pack.to_string(),
            )
        })?;
        if policy_revision.is_none() && policy.policy.owner.pack_id != subject_pack {
            return Err(invalid());
        }
        let actor = DelegationAuthority::load_for_share(connection, identity).await?;
        let mut allowed = Vec::new();
        for entry in &policy.policy.invocations {
            if !source_visible(connection, &actor, &policy, entry).await? {
                continue;
            }
            let executable = metadata(
                ManagedExecutableRepository::invocation_snapshot(connection, entry).await,
            )?;
            let action = &executable.executable.action;
            if action.pack == subject_pack
                && component_visible(
                    connection,
                    &actor,
                    ComponentTarget::Action,
                    action.id,
                    &action.r#ref,
                    action.pack,
                )
                .await?
            {
                allowed.push(digest(&serde_json::to_value(&entry.binding)?)?);
            }
        }
        if allowed.is_empty() {
            if policy_revision.is_some() {
                return Err(Error::not_found(
                    "input policy metadata",
                    "subject pack",
                    subject_pack.to_string(),
                ));
            }
            return Ok((Vec::new(), 0));
        }
        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_declared_input_binding WHERE policy_revision=$1 AND invocation_digest=ANY($2) AND (declaration->>'pack')::BIGINT=$3")
            .bind(policy.id).bind(&allowed).bind(subject_pack).fetch_one(&mut *connection).await?;
        let rows: Vec<DeclarationRow> = sqlx::query_as("SELECT invocation_digest,declaration,declaration_digest FROM runtime_declared_input_binding WHERE policy_revision=$1 AND invocation_digest=ANY($2) AND (declaration->>'pack')::BIGINT=$3 ORDER BY id LIMIT $4 OFFSET $5")
            .bind(policy.id).bind(&allowed).bind(subject_pack).bind(limit).bind(offset).fetch_all(&mut *connection).await?;
        let mut targets = Vec::with_capacity(rows.len());
        for row in rows {
            let declaration = decode(row.declaration, &row.declaration_digest)?;
            if declaration.pack != subject_pack {
                return Err(invalid());
            }
            let entry = invocation(&policy, &row.invocation_digest)?;
            targets.push(
                target(connection, &actor, &policy, entry, declaration)
                    .await?
                    .ok_or_else(invalid)?,
            );
        }
        Ok((targets, total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hidden_or_collected_input_targets_keep_the_input_only_marker() {
        let value =
            serde_json::to_value(ApprovalPurpose::DeclaredInput { targets: vec![] }).unwrap();
        assert_eq!(value, json!({"kind":"declared_input","targets":[]}));
        assert_ne!(
            value,
            serde_json::to_value(ApprovalPurpose::Runtime).unwrap()
        );
    }

    #[test]
    fn retained_declaration_digest_is_required_and_private_fields_are_not_public_data() {
        let declaration = DeclaredInputBinding {
            action: 1,
            pack: 2,
            release: 3,
            metadata_revision: None,
            declaration_revision: "a".repeat(64),
            source_path: "/workflow/vars/token".into(),
            expression: "{{ keystore['foreign.token'] }}".into(),
        };
        let body = serde_json::to_value(&declaration).unwrap();
        assert!(decode(body.clone(), &"0".repeat(64)).is_err());
        assert_eq!(
            decode(body.clone(), &digest(&body).unwrap()).unwrap(),
            declaration
        );
        let target = DeclaredInputApprovalTarget {
            policy_revision: 4,
            invocation_binding: InvocationBinding::Action {
                action_ref: "example.flow".into(),
            },
            action_id: 1,
            action_ref: "example.flow".into(),
            pack_id: 2,
            pack_ref: "example".into(),
            release_id: 3,
            metadata_revision: None,
            declaration_revision: declaration.declaration_revision,
            source_path: declaration.source_path,
            expression: declaration.expression,
        };
        let mut public = serde_json::to_value(target).unwrap();
        let fields = public.as_object().unwrap();
        for private in [
            "proof",
            "origin",
            "value",
            "config",
            "ciphertext",
            "source_admission",
            "permission_set_refs",
            "concrete_targets",
        ] {
            assert!(!fields.contains_key(private));
        }
        public["proof"] = json!({"origin":"not public metadata"});
        assert!(serde_json::from_value::<DeclaredInputApprovalTarget>(public).is_err());
    }

    #[test]
    fn metadata_decoder_errors_do_not_quote_private_retained_values() {
        let raw = serde_json::from_value::<DeclaredInputBinding>(
            json!({"action":"PRIVATE_GRAPH_CANARY"}),
        )
        .unwrap_err();
        let error = metadata::<()>(Err(raw.into())).unwrap_err();
        assert!(matches!(&error, Error::Internal(_)));
        assert!(!error.to_string().contains("PRIVATE_GRAPH_CANARY"));
    }
}

fn digest(value: &impl Serialize) -> Result<String> {
    crate::value_schema::digest(value).map_err(|_| invalid())
}

fn invalid() -> Error {
    Error::validation("Retained input approval metadata is unavailable or invalid")
}

// Decoder errors can quote untrusted retained field values. Metadata inspection
// fails with a generic server error, never a private graph/proof value in an API
// diagnostic. This does not intercept issuance/revocation errors or grant rights.
fn metadata<T>(result: Result<T>) -> Result<T> {
    result
        .map_err(|_| Error::internal("Retained input approval metadata is unavailable or invalid"))
}

fn decode(body: Value, expected: &str) -> Result<DeclaredInputBinding> {
    if digest(&body)? != expected {
        return Err(invalid());
    }
    serde_json::from_value(body).map_err(|_| invalid())
}

fn invocation<'a>(
    policy: &'a RuntimePolicyRevision,
    expected: &str,
) -> Result<&'a InvocationPolicy> {
    let mut found = None;
    for entry in &policy.policy.invocations {
        if digest(&serde_json::to_value(&entry.binding)?)? == expected {
            if found.is_some() {
                return Err(invalid());
            }
            found = Some(entry);
        }
    }
    found.ok_or_else(invalid)
}

async fn source_visible(
    connection: &mut PgConnection,
    actor: &DelegationAuthority,
    policy: &RuntimePolicyRevision,
    entry: &InvocationPolicy,
) -> Result<bool> {
    let (kind, reference) = match &entry.binding {
        InvocationBinding::Action { action_ref } => (ComponentTarget::Action, action_ref),
        InvocationBinding::WorkflowTask { workflow_ref, .. } => {
            (ComponentTarget::Action, workflow_ref)
        }
        InvocationBinding::Rule { rule_ref, .. } => (ComponentTarget::Rule, rule_ref),
        InvocationBinding::Queue { queue_ref, .. } => (ComponentTarget::Queue, queue_ref),
    };
    let Some(id) = entry.source_id else {
        return Err(invalid());
    };
    component_visible(
        connection,
        actor,
        kind,
        id,
        reference,
        policy.policy.owner.pack_id,
    )
    .await
}

async fn component_visible(
    connection: &mut PgConnection,
    actor: &DelegationAuthority,
    kind: ComponentTarget,
    id: Id,
    reference: &str,
    pack: Id,
) -> Result<bool> {
    let Some(actual) = RuntimeTargetRepository::resolve(
        connection,
        RuntimeTarget::Component(kind, id),
        actor.identity_id,
    )
    .await?
    else {
        return Ok(false);
    };
    if actual.pack_id() != Some(pack) || actual.context().target_ref.as_deref() != Some(reference) {
        return Ok(false);
    }
    if actor.allows(actual.resource(), Action::Read, actual.context().clone()) {
        return Ok(true);
    }
    if matches!(kind, ComponentTarget::Action) {
        return Ok(ActionRepository::find_by_id(&mut *connection, id)
            .await?
            .is_some_and(|action| {
                action.r#ref == reference
                    && action.pack == pack
                    && action.reference_visibility == ActionReferenceVisibility::Public
            }));
    }
    Ok(false)
}

async fn target(
    connection: &mut PgConnection,
    actor: &DelegationAuthority,
    policy: &RuntimePolicyRevision,
    entry: &InvocationPolicy,
    declaration: DeclaredInputBinding,
) -> Result<Option<DeclaredInputApprovalTarget>> {
    if !source_visible(connection, actor, policy, entry).await?
        || !component_visible(
            connection,
            actor,
            ComponentTarget::Action,
            declaration.action,
            entry.binding.action_ref(),
            declaration.pack,
        )
        .await?
    {
        return Ok(None);
    }
    let snapshot =
        metadata(ManagedExecutableRepository::invocation_snapshot(connection, entry).await)?;
    let action = &snapshot.executable.action;
    let actual = if let Some(path) = declaration.source_path.strip_prefix("/param_schema/") {
        action
            .param_schema
            .as_ref()
            .and_then(|schema| schema.pointer(&format!("/{path}")))
    } else if let Some(path) = declaration.source_path.strip_prefix("/workflow/") {
        snapshot
            .executable
            .workflow_definition
            .as_ref()
            .and_then(|workflow| workflow.definition.pointer(&format!("/{path}")))
    } else {
        None
    };
    if declaration.action != action.id
        || declaration.pack != action.pack
        || declaration.release != snapshot.release.id
        || declaration.metadata_revision != entry.metadata_revision
        || declaration.declaration_revision != digest(&serde_json::to_value(&snapshot.executable)?)?
        || actual.and_then(Value::as_str) != Some(declaration.expression.as_str())
    {
        return Err(invalid());
    }
    Ok(Some(DeclaredInputApprovalTarget {
        policy_revision: policy.id,
        invocation_binding: entry.binding.clone(),
        action_id: action.id,
        action_ref: action.r#ref.clone(),
        pack_id: action.pack,
        pack_ref: action.pack_ref.clone(),
        release_id: declaration.release,
        metadata_revision: declaration.metadata_revision,
        declaration_revision: declaration.declaration_revision,
        source_path: declaration.source_path,
        expression: declaration.expression,
    }))
}
