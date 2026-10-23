//! Immutable cross-pack graph handoff. Outer invocation rights stay with the
//! source sponsor; inner tasks use the pinned publisher policy, never current.

use super::{
    managed_executable::ManagedExecutableRepository,
    runtime_authority::{RuntimeAuthorityRepository, RuntimePolicyRevision},
};
use crate::{
    models::{ExecutionExecutableSnapshot, Id},
    runtime_authority::{InvocationBinding, InvocationPolicy, RuntimePolicy},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPolicyPin {
    pub revision: Id,
    pub digest: String,
}

impl WorkflowPolicyPin {
    pub fn verify(&self, revision: &RuntimePolicyRevision) -> Result<()> {
        if self.revision != revision.id || self.digest != revision.policy_digest {
            return Err(Error::PermissionDenied(
                "Pinned workflow policy identity mismatch".into(),
            ));
        }
        Ok(())
    }
}

pub struct WorkflowPolicyRepository;

/// Resolve a planner-authored task pointer against the immutable normalized
/// graph. Named YAML tasks become array entries with retained `name` fields.
/// Ambiguous numeric-name/index selectors do not establish source identity.
pub fn task_at_authored_path<'a>(
    definition: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let tokens = path.strip_prefix('/')?.split('/').collect::<Vec<_>>();
    if tokens.is_empty() || tokens.len() % 2 != 0 {
        return None;
    }
    let mut node = definition;
    for pair in tokens.as_chunks::<2>().0 {
        if pair[0] != "tasks" {
            return None;
        }
        let selector = pair[1].replace("~1", "/").replace("~0", "~");
        node = match node.get("tasks")? {
            serde_json::Value::Object(tasks) => tasks.get(&selector)?,
            serde_json::Value::Array(tasks) => {
                let mut candidates = tasks.iter().enumerate().filter(|(index, task)| {
                    index.to_string() == selector
                        || task.get("name").and_then(serde_json::Value::as_str)
                            == Some(selector.as_str())
                });
                let (_, task) = candidates.next()?;
                if candidates.next().is_some() {
                    return None;
                }
                task
            }
            _ => return None,
        };
    }
    Some(node)
}

impl WorkflowPolicyRepository {
    /// Deployment lookup only. Runtime callers must use `for_invocation`.
    pub async fn deployment_executable(
        connection: &mut PgConnection,
        pack: Id,
        reference: &str,
    ) -> Result<ExecutionExecutableSnapshot> {
        let publisher = RuntimeAuthorityRepository::current_policy(connection, pack)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Foreign workflow has no admitted publisher policy".into())
            })?;
        ManagedExecutableRepository::resolve_invocation(
            connection,
            &publisher.policy,
            &InvocationBinding::Action {
                action_ref: reference.into(),
            },
        )
        .await
    }

    pub(crate) async fn capture(
        connection: &mut PgConnection,
        policy: &mut RuntimePolicy,
    ) -> Result<()> {
        for invocation in &mut policy.invocations {
            let target =
                ManagedExecutableRepository::invocation_snapshot(connection, invocation).await?;
            let foreign = target.executable.action.pack != policy.owner.pack_id
                && target.executable.workflow_definition.is_some();
            if !foreign {
                if invocation.foreign_workflow_policy.is_some() {
                    return Err(Error::validation(
                        "Only foreign workflow invocations carry a graph policy pin",
                    ));
                }
                continue;
            }
            let publisher = match &invocation.foreign_workflow_policy {
                Some(pin) => {
                    let revision =
                        RuntimeAuthorityRepository::require_policy(connection, pin.revision)
                            .await?;
                    pin.verify(&revision)?;
                    revision
                }
                None => RuntimeAuthorityRepository::current_policy(
                    connection,
                    target.executable.action.pack,
                )
                .await?
                .ok_or_else(|| {
                    Error::PermissionDenied(
                        "Foreign workflow has no admitted publisher policy".into(),
                    )
                })?,
            };
            Self::check_target(connection, invocation, &target, &publisher).await?;
            invocation.foreign_workflow_policy = Some(WorkflowPolicyPin {
                revision: publisher.id,
                digest: publisher.policy_digest,
            });
        }
        Ok(())
    }

    async fn check_target(
        connection: &mut PgConnection,
        invocation: &InvocationPolicy,
        target: &ExecutionExecutableSnapshot,
        publisher: &RuntimePolicyRevision,
    ) -> Result<()> {
        let binding = InvocationBinding::Action {
            action_ref: invocation.binding.action_ref().into(),
        };
        let declared = publisher
            .policy
            .invocations
            .iter()
            .find(|entry| entry.binding == binding)
            .ok_or_else(|| {
                Error::PermissionDenied("Foreign workflow has no publisher Action admission".into())
            })?;
        let expected =
            ManagedExecutableRepository::invocation_snapshot(connection, declared).await?;
        if publisher.policy.owner.pack_id != target.executable.action.pack
            || declared.source_id != Some(target.executable.action.id)
            || expected.release.id != target.release.id
            || expected.release.digest != target.release.digest
            || serde_json::to_value(&expected.executable)?
                != serde_json::to_value(&target.executable)?
            || declared.metadata_revision != invocation.metadata_revision
            || expected.executable.workflow_definition.is_none()
        {
            return Err(Error::PermissionDenied("Foreign workflow graph/release/metadata differs from its pinned publisher admission".into()));
        }
        Ok(())
    }

    /// B calls this after requiring the parent admission. The returned policy
    /// supplies task bindings and sponsor, while the original parent remains a
    /// live revocation dependency through ParentExecution attribution.
    pub async fn for_invocation(
        connection: &mut PgConnection,
        source: &RuntimePolicyRevision,
        binding: &InvocationBinding,
        snapshot: &ExecutionExecutableSnapshot,
    ) -> Result<RuntimePolicyRevision> {
        let source = RuntimeAuthorityRepository::require_policy(connection, source.id).await?;
        let invocation = source
            .policy
            .invocations
            .iter()
            .find(|entry| &entry.binding == binding)
            .ok_or_else(|| Error::PermissionDenied("Workflow has no source admission".into()))?;
        let expected =
            ManagedExecutableRepository::invocation_snapshot(connection, invocation).await?;
        if expected.release.id != snapshot.release.id
            || expected.release.digest != snapshot.release.digest
            || serde_json::to_value(&expected.executable)?
                != serde_json::to_value(&snapshot.executable)?
            || snapshot.executable.workflow_definition.is_none()
        {
            return Err(Error::PermissionDenied(
                "Parent workflow differs from its retained admission".into(),
            ));
        }
        if let Some(pin) = &invocation.foreign_workflow_policy {
            let publisher =
                RuntimeAuthorityRepository::require_policy(connection, pin.revision).await?;
            pin.verify(&publisher)?;
            Self::check_target(connection, invocation, snapshot, &publisher).await?;
            Ok(publisher)
        } else if snapshot.executable.action.pack == source.policy.owner.pack_id {
            Ok(source)
        } else {
            Err(Error::PermissionDenied(
                "Foreign workflow is missing its immutable graph policy pin".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn source_pointer_survives_named_task_normalization() {
        let graph = json!({"tasks":[{"name":"group/name","tasks":[{"name":"leaf","action":"target.run"}]}]});
        assert_eq!(
            task_at_authored_path(&graph, "/tasks/group~1name/tasks/0").unwrap()["action"],
            "target.run"
        );
        assert!(task_at_authored_path(&graph, "/variables/leaf").is_none());
        let ambiguous = json!({"tasks":[{"name":"first"},{"name":"0"}]});
        assert!(task_at_authored_path(&ambiguous, "/tasks/0").is_none());
    }
}
