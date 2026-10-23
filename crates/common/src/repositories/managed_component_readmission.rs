//! Transactional managed graph and dispatch edits. No runtime or requester IAM
//! is consulted when retaining existing invocation authority.

use super::{
    identity::PermissionSetRepository,
    managed_executable::ManagedExecutableRepository,
    runtime_authority::{RuntimeAuthorityRepository, RuntimePolicyRevision},
    schema_type::SchemaTypeRepository,
    FindById,
};
use crate::{models::Id, pack_registry::runtime_policy::CandidateRuntimePolicyPlan, Error, Result};
use serde_json::Value;
use sqlx::PgConnection;

mod deployment;

pub struct ManagedComponentReadmission;

impl ManagedComponentReadmission {
    pub(crate) async fn source_metadata(
        connection: &mut PgConnection,
        pack: Id,
        kind: &str,
        reference: &str,
    ) -> Result<Value> {
        let query = if kind == "rules" {
            "SELECT jsonb_build_object('rule_id',id,'rule_ref',ref,'pack_ref',pack_ref,'action',action,'action_ref',action_ref,'trigger',trigger,'trigger_ref',trigger_ref,'sensor_id',(SELECT sensor FROM trigger WHERE trigger.id = rule.trigger),'sensor_worker_selector',sensor_worker_selector,'sensor_worker_tolerations',sensor_worker_tolerations,'sensor_worker_affinity',sensor_worker_affinity,'conditions',conditions,'action_params',action_params,'trigger_params',trigger_params,'permission_set_refs',permission_set_refs,'trace_tag_template',trace_tag_template) FROM rule WHERE ref = $1 AND pack = $2 AND management_origin = 'pack' AND retired_at IS NULL FOR SHARE"
        } else {
            "SELECT jsonb_build_object('action_params',action_params,'dispatch_action',dispatch_action,'dispatch_action_ref',dispatch_action_ref,'permission_set_refs',permission_set_refs,'item_schema',item_schema,'trace_tag_template',trace_tag_template,'config',config) FROM work_queue WHERE ref = $1 AND pack = $2 AND management_origin = 'pack' AND retired_at IS NULL FOR SHARE"
        };
        let mut body: Value = sqlx::query_scalar(query)
            .bind(reference)
            .bind(pack)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(|| Error::validation("Managed dispatch source is unavailable"))?;
        if kind == "rules" {
            use super::{iam_ownership::IamOwnershipRepository, runtime_target::ComponentTarget};
            let id = body["rule_id"]
                .as_i64()
                .ok_or_else(|| Error::validation("Rule source identity is unavailable"))?;
            let managing_pack =
                IamOwnershipRepository::component_pack(&mut *connection, ComponentTarget::Rule, id)
                    .await?;
            if managing_pack != Some(pack) {
                return Err(Error::validation(
                    "Rule source has no matching structural managing pack",
                ));
            }
            body["managing_pack_id"] = serde_json::json!(managing_pack);
            if let Some(trigger) = body["trigger"].as_i64() {
                body["trigger_managing_pack_id"] = serde_json::json!(
                    IamOwnershipRepository::component_pack(
                        &mut *connection,
                        ComponentTarget::Trigger,
                        trigger
                    )
                    .await?
                );
            }
            if let Some(sensor) = body["sensor_id"].as_i64() {
                body["sensor_managing_pack_id"] = serde_json::json!(
                    IamOwnershipRepository::component_pack(
                        &mut *connection,
                        ComponentTarget::Sensor,
                        sensor
                    )
                    .await?
                );
            }
        }
        Ok(body)
    }

    pub async fn lock_queue(
        connection: &mut PgConnection,
        id: Id,
    ) -> Result<crate::models::WorkQueue> {
        sqlx::query("SELECT id FROM work_queue WHERE id = $1 FOR UPDATE")
            .bind(id)
            .execute(&mut *connection)
            .await?;
        super::work_queue::WorkQueueRepository::get_by_id(connection, id).await
    }

    /// Call after the authorized projection write, before commit. Graph admission
    /// must already be exclusive before taking any component/pack row locks.
    pub async fn rule(
        connection: &mut PgConnection,
        id: Id,
        editor: Id,
    ) -> Result<Option<RuntimePolicyRevision>> {
        Self::dispatch(connection, id, editor, "rules").await
    }

    pub async fn queue(
        connection: &mut PgConnection,
        id: Id,
        editor: Id,
    ) -> Result<Option<RuntimePolicyRevision>> {
        Self::dispatch(connection, id, editor, "queues").await
    }

    async fn dispatch(
        connection: &mut PgConnection,
        id: Id,
        editor: Id,
        kind: &str,
    ) -> Result<Option<RuntimePolicyRevision>> {
        SchemaTypeRepository::lock_reclamation(connection).await?;
        let query = if kind == "rules" {
            "SELECT pack,pack_ref,jsonb_build_object('ref',ref,'action_ref',action_ref,'permission_set_refs',permission_set_refs) FROM rule WHERE id = $1 AND management_origin = 'pack' AND retired_at IS NULL FOR SHARE"
        } else {
            "SELECT pack,pack_ref,jsonb_build_object('ref',ref,'action_ref',dispatch_action_ref,'permission_set_refs',permission_set_refs) FROM work_queue WHERE id = $1 AND management_origin = 'pack' AND retired_at IS NULL FOR SHARE"
        };
        let row: Option<(Id, String, Value)> = sqlx::query_as(query)
            .bind(id)
            .fetch_optional(&mut *connection)
            .await?;
        let Some((pack, reference, definition)) = row else {
            return Ok(None);
        };
        let source_ref = definition["ref"]
            .as_str()
            .ok_or_else(|| Error::validation("Managed dispatch ref is missing"))?
            .to_string();
        if kind == "rules" {
            let source = Self::source_metadata(connection, pack, kind, &source_ref).await?;
            let trigger_id = source["trigger"]
                .as_i64()
                .ok_or_else(|| Error::validation("Managed rule trigger identity is unavailable"))?;
            let trigger =
                super::trigger::TriggerRepository::find_by_id(&mut *connection, trigger_id)
                    .await?
                    .ok_or_else(|| Error::validation("Managed rule trigger is unavailable"))?;
            if source["trigger_ref"].as_str() != Some(trigger.r#ref.as_str()) {
                return Err(Error::validation("Managed rule trigger identity changed"));
            }
            if let Some(owner) = trigger.pack {
                let actual = super::pack::PackRepository::find_by_id(&mut *connection, owner)
                    .await?
                    .ok_or_else(|| {
                        Error::validation("Managed rule trigger owner is unavailable")
                    })?;
                if trigger.pack_ref.as_deref() != Some(actual.r#ref.as_str()) {
                    return Err(Error::validation(
                        "Managed rule trigger pack reference disagrees with its owner",
                    ));
                }
            }
            crate::action_visibility::ensure_trigger_reference_allowed(
                &trigger,
                Some(&reference),
                "rule",
                &source_ref,
            )?;
        }
        let revision = Self::admit(connection, pack, &reference, definition, kind, editor).await?;
        // Policy capture persists every dispatch source before activation,
        // including unrelated bindings retained by this component edit.
        Ok(Some(revision))
    }

    pub async fn workflow(
        connection: &mut PgConnection,
        action: Id,
        editor: Id,
    ) -> Result<Option<RuntimePolicyRevision>> {
        // Capture immutable graph/schema metadata over the original code bytes.
        let Some(revision) =
            ManagedExecutableRepository::readmit_action(connection, action, editor).await?
        else {
            return Ok(None);
        };
        let source = revision
            .policy
            .invocations
            .iter()
            .find(|entry| {
                matches!(
                    &entry.binding,
                    crate::runtime_authority::InvocationBinding::Action { .. }
                ) && entry.source_id == Some(action)
            })
            .ok_or_else(|| Error::validation("Managed workflow action binding is unavailable"))?;
        let executable = ManagedExecutableRepository::resolve_invocation(
            connection,
            &revision.policy,
            &source.binding,
        )
        .await?;
        super::schema_contract::SchemaContractRepository::materialize_action_pair(
            connection,
            &executable,
        )
        .await?;
        let definition = serde_json::json!({
            "ref": executable.executable.action.r#ref,
            "default_execution_permission_set_refs": executable.executable.action.default_execution_permission_set_refs,
            "definition": executable.executable.workflow_definition.ok_or_else(|| Error::validation("Managed workflow graph is unavailable"))?.definition,
        });
        Self::admit(
            connection,
            revision.policy.owner.pack_id,
            &revision.policy.owner.pack_ref,
            definition,
            "workflows",
            editor,
        )
        .await
        .map(Some)
    }

    async fn admit(
        connection: &mut PgConnection,
        pack: Id,
        pack_ref: &str,
        definition: Value,
        kind: &str,
        editor: Id,
    ) -> Result<RuntimePolicyRevision> {
        let current = RuntimeAuthorityRepository::current_policy(connection, pack)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Managed component has no admitted runtime policy".into())
            })?;
        let refs: Vec<String> = sqlx::query_scalar("SELECT ref FROM permission_set WHERE pack = $1 AND management_origin = 'pack' AND retired_at IS NULL ORDER BY id FOR SHARE")
            .bind(pack).fetch_all(&mut *connection).await?;
        let mut definitions = Vec::new();
        for reference in refs {
            definitions.push(
                PermissionSetRepository::authored_definition(connection, &reference)
                    .await?
                    .ok_or_else(|| {
                        Error::validation("Managed permission authoring is unavailable")
                    })?,
            );
        }
        let plan = CandidateRuntimePolicyPlan::component(pack_ref, definition, kind, definitions)?;
        plan.admit_component(connection, pack, editor, current.policy.0)
            .await
    }
}
