//! Immutable metadata readmission over an existing immutable code release.

use crate::{
    models::{ActionExecutableSnapshot, ExecutionExecutableSnapshot, Id, PackReleasePin},
    runtime_authority::{InvocationBinding, InvocationPolicy, RuntimePolicy},
    value_schema::digest,
    Error, Result,
};
use sqlx::{types::Json, PgConnection};
use std::collections::BTreeMap;

use super::{
    identity::PermissionSetRepository,
    runtime_authority::{RuntimeAuthorityRepository, RuntimePolicyRevision},
    schema_type::SchemaTypeRepository,
};

#[derive(sqlx::FromRow)]
pub struct ManagedExecutableMetadata {
    pub id: Id,
    pub pack: Id,
    pub release: Id,
    pub action: Id,
    pub action_ref: String,
    pub snapshot: Json<ActionExecutableSnapshot>,
    pub snapshot_digest: String,
}

impl ManagedExecutableMetadata {
    fn validate(&self) -> Result<()> {
        if self.snapshot.action.id != self.action
            || self.snapshot.action.pack != self.pack
            || self.snapshot.action.r#ref != self.action_ref
            || digest(&serde_json::to_value(&self.snapshot.0)?)
                .map_err(|_| Error::validation("Invalid executable metadata digest"))?
                != self.snapshot_digest
        {
            return Err(Error::validation(
                "Managed executable metadata integrity mismatch",
            ));
        }
        Ok(())
    }
}

pub struct ManagedExecutableRepository;

impl ManagedExecutableRepository {
    /// Prepare editable metadata pins from immutable deployment bytes before the
    /// policy compiler resolves bindings. Never capture a mutable projection or
    /// rewrite an old policy to fill missing evidence.
    pub async fn capture_deployment_actions(
        connection: &mut PgConnection,
        release: Id,
    ) -> Result<()> {
        SchemaTypeRepository::lock_reclamation(connection).await?;
        for snapshot in
            super::executable_snapshot::ExecutableSnapshotRepository::resolve_release_actions(
                connection, release,
            )
            .await?
            .into_values()
        {
            let action = &snapshot.executable.action;
            let body = serde_json::to_value(&snapshot.executable)?;
            let hash = digest(&body)
                .map_err(|_| Error::validation("Invalid deployment executable metadata"))?;
            sqlx::query("INSERT INTO managed_executable_metadata_revision (pack,release,action,action_ref,snapshot,snapshot_digest) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (release,action,snapshot_digest) DO NOTHING")
                .bind(action.pack).bind(release).bind(action.id).bind(&action.r#ref).bind(body).bind(hash).execute(&mut *connection).await?;
            if Self::revision_for_snapshot(connection, &snapshot)
                .await?
                .is_none()
            {
                return Err(Error::validation(
                    "Deployment executable metadata was not captured",
                ));
            }
        }
        Ok(())
    }

    pub fn changes_admitted_metadata(input: &super::action::UpdateActionInput) -> bool {
        input.entrypoint.is_some()
            || input.runtime.is_some()
            || input.runtime_version_constraint.is_some()
            || input.required_worker_runtimes.is_some()
            || input.worker_selector.is_some()
            || input.worker_tolerations.is_some()
            || input.worker_affinity.is_some()
            || input.param_schema.is_some()
            || input.out_schema.is_some()
            || input.parameter_delivery.is_some()
            || input.parameter_format.is_some()
            || input.output_format.is_some()
            || input.accesses_mcp.is_some()
            || input.default_execution_permission_set_refs.is_some()
            || input.reference_visibility.is_some()
            || input.reference_allowed_pack_refs.is_some()
            || input.artifact_retention_policy.is_some()
            || input.artifact_retention_limit.is_some()
            || input.log_retention_policy.is_some()
            || input.log_retention_limit.is_some()
            || input.timeout_seconds.is_some()
    }

    pub async fn revision_for_snapshot(
        connection: &mut PgConnection,
        snapshot: &ExecutionExecutableSnapshot,
    ) -> Result<Option<Id>> {
        let digest = digest(&serde_json::to_value(&snapshot.executable)?)
            .map_err(|_| Error::validation("Invalid executable metadata digest"))?;
        Ok(sqlx::query_scalar("SELECT id FROM managed_executable_metadata_revision WHERE action = $1 AND release = $2 AND snapshot_digest = $3 FOR SHARE")
            .bind(snapshot.executable.action.id).bind(snapshot.release.id).bind(digest).fetch_optional(connection).await?)
    }

    pub(crate) async fn workflow_source_snapshot(
        connection: &mut PgConnection,
        policy: &RuntimePolicy,
        reference: &str,
    ) -> Result<ActionExecutableSnapshot> {
        let source=if let Some(invocation)=policy.invocations.iter().find(|entry| matches!(&entry.binding,InvocationBinding::Action {action_ref} if action_ref == reference)) {
            Self::invocation_snapshot(connection,invocation).await?.executable
        } else {
            let body:serde_json::Value=sqlx::query_scalar("SELECT snapshot FROM pack_release_executable WHERE release = $1 AND component_kind = 'action' AND component_ref = $2 FOR SHARE")
                .bind(policy.release_id).bind(reference).fetch_optional(connection).await?
                .ok_or_else(|| Error::validation("Workflow source is absent from admitted immutable metadata"))?;
            serde_json::from_value(body)?
        };
        if source.action.r#ref != reference
            || source.action.pack != policy.owner.pack_id
            || source.workflow_definition.is_none()
        {
            return Err(Error::validation(
                "Admitted workflow source ownership mismatch",
            ));
        }
        Ok(source)
    }

    pub(crate) async fn capture_sources(
        connection: &mut PgConnection,
        policy: &mut RuntimePolicy,
    ) -> Result<()> {
        for index in 0..policy.invocations.len() {
            if let InvocationBinding::Rule { rule_ref, .. } = &policy.invocations[index].binding {
                // Fresh rule edits get an immutable dispatch revision just like
                // queues. Retained invocation bodies must never be recaptured.
                if policy.invocations[index].source_id.is_none() {
                    let body = super::managed_component_readmission::ManagedComponentReadmission::source_metadata(connection, policy.owner.pack_id, "rules", rule_ref).await?;
                    policy.invocations[index].dispatch_template_digest = Some(
                        digest(&body)
                            .map_err(|_| Error::validation("Invalid rule dispatch digest"))?,
                    );
                }
            }
            if let InvocationBinding::Queue { queue_ref, .. } = &policy.invocations[index].binding {
                if policy.invocations[index].source_id.is_none() {
                    let body:serde_json::Value=sqlx::query_scalar(
                    "SELECT jsonb_build_object('action_params',action_params,'dispatch_action',dispatch_action,'dispatch_action_ref',dispatch_action_ref,'permission_set_refs',permission_set_refs,'item_schema',item_schema,'trace_tag_template',trace_tag_template,'config',config) FROM work_queue WHERE ref = $1 AND pack = $2 AND management_origin = 'pack' AND retired_at IS NULL FOR SHARE",
                ).bind(queue_ref).bind(policy.owner.pack_id).fetch_optional(&mut *connection).await?
                    .ok_or_else(|| Error::validation("Admitted queue source is unavailable"))?;
                    policy.invocations[index].dispatch_template_digest =
                        Some(digest(&body).map_err(|_| {
                            Error::validation("Invalid admitted queue template digest")
                        })?);
                }
            }
            if policy.invocations[index].source_id.is_some() {
                continue;
            }
            let invocation = &policy.invocations[index];
            let target = Self::invocation_snapshot(connection, invocation).await?;
            let source=match &invocation.binding {
                InvocationBinding::Action {..}=>target.executable.action.id,
                InvocationBinding::WorkflowTask {workflow_ref,task_path,action_ref}=> {
                    let source=Self::workflow_source_snapshot(connection,policy,workflow_ref).await?;
                    let task=source.workflow_definition.as_ref().and_then(|workflow| super::workflow_policy::task_at_authored_path(&workflow.definition,task_path))
                        .ok_or_else(|| Error::validation("Admitted workflow task path does not exist in pinned metadata"))?;
                    let action=task.get("action").or_else(||task.get("action_ref")).and_then(serde_json::Value::as_str)
                        .ok_or_else(|| Error::validation("Admitted workflow task has no static action target"))?;
                    let targets=crate::pack_registry::runtime_policy::task_action_targets(action, task.get("allowed_action_refs"), &policy.owner.pack_ref, "admitted workflow", task_path)?;
                    if !targets.contains(action_ref) {return Err(Error::validation("Workflow task action differs from its admitted binding"));}
                    source.action.id
                },
                InvocationBinding::Rule {rule_ref,..}=>sqlx::query_scalar(
                    "SELECT id FROM rule WHERE ref = $1 AND pack = $2 AND action = $3 AND management_origin = 'pack' AND retired_at IS NULL FOR SHARE",
                ).bind(rule_ref).bind(policy.owner.pack_id).bind(target.executable.action.id).fetch_optional(&mut *connection).await?
                    .ok_or_else(|| Error::validation("Rule invocation has no structurally owned dispatch source"))?,
                InvocationBinding::Queue {queue_ref,..}=>sqlx::query_scalar(
                    "SELECT id FROM work_queue WHERE ref = $1 AND pack = $2 AND dispatch_action = $3 AND management_origin = 'pack' AND retired_at IS NULL FOR SHARE",
                ).bind(queue_ref).bind(policy.owner.pack_id).bind(target.executable.action.id).fetch_optional(&mut *connection).await?
                    .ok_or_else(|| Error::validation("Queue invocation has no structurally owned dispatch source"))?,
            };
            policy.invocations[index].source_id = Some(source);
        }
        Ok(())
    }

    pub async fn find_revision(
        connection: &mut PgConnection,
        id: Id,
    ) -> Result<ManagedExecutableMetadata> {
        let record: ManagedExecutableMetadata = sqlx::query_as(
            "SELECT id,pack,release,action,action_ref,snapshot,snapshot_digest \
             FROM managed_executable_metadata_revision WHERE id = $1 FOR SHARE",
        )
        .bind(id)
        .fetch_optional(connection)
        .await?
        .ok_or_else(|| Error::validation("Managed executable metadata revision is unavailable"))?;
        record.validate()?;
        Ok(record)
    }

    /// Resolve policy-pinned metadata, never a mutable current pointer. Production
    /// execution admission must use this for direct, dispatch and retry snapshots.
    pub async fn resolve_invocation(
        connection: &mut PgConnection,
        policy: &RuntimePolicy,
        binding: &InvocationBinding,
    ) -> Result<ExecutionExecutableSnapshot> {
        let invocation = policy
            .invocations
            .iter()
            .find(|entry| &entry.binding == binding)
            .ok_or_else(|| {
                Error::PermissionDenied("Invocation has no admitted executable".into())
            })?;
        let executable = Self::invocation_snapshot(connection, invocation).await?;
        let mut pack_executables = BTreeMap::new();
        if executable.executable.workflow_definition.is_some() {
            let publisher;
            let graph_policy = if let Some(pin) = &invocation.foreign_workflow_policy {
                publisher =
                    RuntimeAuthorityRepository::require_policy(connection, pin.revision).await?;
                pin.verify(&publisher)?;
                &publisher.policy.0
            } else {
                policy
            };
            let mut ambiguous = std::collections::BTreeSet::new();
            for entry in &graph_policy.invocations {
                if matches!(&entry.binding, InvocationBinding::WorkflowTask {workflow_ref,..}
                    if workflow_ref == &executable.executable.action.r#ref)
                {
                    let action = Self::invocation_snapshot(connection, entry).await?;
                    let reference = entry.binding.action_ref();
                    if ambiguous.contains(reference) {
                        continue;
                    }
                    if let Some(previous) = pack_executables.get(reference) {
                        let previous: &crate::models::ReleasedActionExecutableSnapshot = previous;
                        if previous.release.id != action.release.id
                            || serde_json::to_value(&previous.executable)?
                                != serde_json::to_value(&action.executable)?
                        {
                            // A ref map cannot represent distinct task pins. Do
                            // not pick one, follow current metadata, or reject
                            // an otherwise supported graph. B resolves those
                            // task targets through their exact policy binding.
                            pack_executables.remove(reference);
                            ambiguous.insert(reference.to_owned());
                            continue;
                        }
                    }
                    pack_executables.insert(
                        entry.binding.action_ref().into(),
                        crate::models::ReleasedActionExecutableSnapshot {
                            release: action.release,
                            executable: action.executable,
                        },
                    );
                }
            }
        }
        Ok(ExecutionExecutableSnapshot {
            pack_executables,
            ..executable
        })
    }

    pub(crate) async fn invocation_snapshot(
        connection: &mut PgConnection,
        invocation: &InvocationPolicy,
    ) -> Result<ExecutionExecutableSnapshot> {
        let release = super::pack_release::PackReleaseRepository::find_by_id(
            &mut *connection,
            invocation.target_release_id,
        )
        .await?
        .ok_or_else(|| Error::validation("Admitted executable code release is unavailable"))?;
        if release.digest != invocation.target_release_digest {
            return Err(Error::validation(
                "Admitted executable code digest mismatch",
            ));
        }
        let executable = if let Some(id) = invocation.metadata_revision {
            let metadata = Self::find_revision(connection, id).await?;
            if metadata.release != release.id
                || metadata.pack != release.pack
                || metadata.action_ref != invocation.binding.action_ref()
            {
                return Err(Error::validation(
                    "Admitted metadata does not match its code release",
                ));
            }
            let source: Option<Id> = sqlx::query_scalar(
                "SELECT component_id FROM pack_release_executable WHERE release = $1 AND component_kind = 'action' AND component_ref = $2 FOR SHARE",
            ).bind(release.id).bind(&metadata.action_ref).fetch_optional(&mut *connection).await?;
            if source != Some(metadata.action) {
                return Err(Error::validation(
                    "Managed metadata action is absent from its immutable code release",
                ));
            }
            metadata.snapshot.0
        } else {
            let body: serde_json::Value = sqlx::query_scalar(
                "SELECT snapshot FROM pack_release_executable WHERE release = $1 \
                 AND component_kind = 'action' AND component_ref = $2 FOR SHARE",
            )
            .bind(release.id)
            .bind(invocation.binding.action_ref())
            .fetch_optional(connection)
            .await?
            .ok_or_else(|| Error::validation("Admitted action is absent from its code release"))?;
            serde_json::from_value(body)?
        };
        if executable.action.pack != release.pack
            || executable.action.r#ref != invocation.binding.action_ref()
        {
            return Err(Error::validation("Admitted executable ownership mismatch"));
        }
        Ok(ExecutionExecutableSnapshot {
            release: PackReleasePin {
                id: release.id,
                digest: release.digest,
                content_path: release.content_path,
            },
            executable,
            pack_executables: BTreeMap::new(),
        })
    }

    /// Readmit an already-authorized managed action update in the same transaction
    /// as its projection write. Does not grant Actions/Update or pack management.
    /// Existing dispatch selections stay bounded to their previously approved sets.
    pub async fn readmit_action(
        connection: &mut PgConnection,
        action_id: Id,
        installer_identity: Id,
    ) -> Result<Option<RuntimePolicyRevision>> {
        SchemaTypeRepository::lock_reclamation(connection).await?;
        let row: Option<(Id, Id, String, serde_json::Value)> = sqlx::query_as(
            "SELECT a.pack,a.managed_release,a.ref,jsonb_build_object( \
             'action',to_jsonb(a),'runtime',to_jsonb(r), \
             'runtime_versions',COALESCE((SELECT jsonb_agg(to_jsonb(rv) ORDER BY rv.version,rv.id) \
                 FROM runtime_version rv WHERE rv.runtime = r.id AND rv.retired_at IS NULL),'[]'::jsonb), \
             'workflow_definition',to_jsonb(wd)) \
             FROM action a LEFT JOIN runtime r ON r.id = a.runtime AND r.retired_at IS NULL \
             LEFT JOIN workflow_definition wd ON wd.id = a.workflow_def AND wd.retired_at IS NULL \
             WHERE a.id = $1 AND a.managed_release IS NOT NULL AND a.retired_at IS NULL FOR SHARE OF a",
        ).bind(action_id).fetch_optional(&mut *connection).await?;
        let Some((pack, release, reference, body)) = row else {
            return Ok(None);
        };
        let current = RuntimeAuthorityRepository::current_policy(connection, pack)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied(
                    "Managed executable requires deployment readmission before editing".into(),
                )
            })?;
        if release != current.policy.release_id {
            return Err(Error::validation(
                "Managed action does not belong to the active admitted release",
            ));
        }
        let snapshot: ActionExecutableSnapshot = serde_json::from_value(body)?;
        let digest = digest(&serde_json::to_value(&snapshot)?)
            .map_err(|_| Error::validation("Invalid executable metadata digest"))?;
        let metadata: Option<Id> = sqlx::query_scalar(
            "INSERT INTO managed_executable_metadata_revision (pack,release,action,action_ref,snapshot,snapshot_digest) \
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (release,action,snapshot_digest) DO NOTHING RETURNING id",
        ).bind(pack).bind(release).bind(action_id).bind(&reference).bind(Json(&snapshot)).bind(&digest)
            .fetch_optional(&mut *connection).await?;
        let metadata = match metadata {
            Some(id) => id,
            None => sqlx::query_scalar("SELECT id FROM managed_executable_metadata_revision WHERE release = $1 AND action = $2 AND snapshot_digest = $3")
                .bind(release).bind(action_id).bind(&digest).fetch_one(&mut *connection).await?
        };
        let mut policy = current.policy.0;
        let mut sets = policy
            .invocations
            .iter()
            .flat_map(|entry| entry.permission_sets.clone())
            .collect::<BTreeMap<_, _>>();
        let named = snapshot
            .action
            .default_execution_permission_set_refs
            .iter()
            .filter(|reference| reference.as_str() != "standard" && !sets.contains_key(*reference))
            .cloned()
            .collect::<Vec<_>>();
        let new_sets = PermissionSetRepository::find_by_refs_for_share(connection, &named).await?;
        if new_sets.len()
            != named
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        {
            return Err(Error::validation(
                "Managed action selects an unavailable policy",
            ));
        }
        for set in new_sets {
            if set.pack != Some(pack) {
                return Err(Error::PermissionDenied(
                    "New foreign selections require deployment admission".into(),
                ));
            }
            let definition = PermissionSetRepository::authored_definition(connection, &set.r#ref)
                .await?
                .ok_or_else(|| {
                    Error::validation("Managed action policy authoring is unavailable")
                })?;
            // No new external scope is inferred from mutable set names or the
            // editor's unrelated personal grants.
            sets.insert(
                definition.reference.clone(),
                definition.compile(&policy.owner, &[])?,
            );
        }
        let code =
            super::pack_release::PackReleaseRepository::find_by_id(&mut *connection, release)
                .await?
                .ok_or_else(|| Error::validation("Managed action code release is unavailable"))?;
        let executable = ExecutionExecutableSnapshot {
            release: PackReleasePin {
                id: code.id,
                digest: code.digest,
                content_path: code.content_path,
            },
            executable: snapshot,
            pack_executables: BTreeMap::new(),
        };
        for invocation in &mut policy.invocations {
            if invocation.binding.action_ref() == reference {
                if matches!(invocation.binding, InvocationBinding::Action { .. }) {
                    *invocation = InvocationPolicy::from_executable(
                        invocation.binding.clone(),
                        &executable,
                        None,
                        &sets,
                    )?;
                }
                invocation.metadata_revision = Some(metadata);
            }
        }
        // Issuer identity is attribution for the authorized control-plane edit,
        // not a future runtime dependency. External grants are rechecked by capture.
        crate::runtime_authority::positive_id(installer_identity)?;
        let revision = RuntimeAuthorityRepository::capture_policy(connection, &policy).await?;
        RuntimeAuthorityRepository::activate_policy(connection, revision.id).await?;
        sqlx::query("INSERT INTO managed_executable_metadata_current (action,revision) VALUES ($1,$2) ON CONFLICT (action) DO UPDATE SET revision = EXCLUDED.revision")
            .bind(action_id).bind(metadata).execute(connection).await?;
        Ok(Some(revision))
    }
}
