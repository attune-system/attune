//! Candidate runtime policy compilation. This does not bypass deployment checks
//! or grant runtime access before the production admission consumers are converted.

use super::loader::{read_yaml_files, resolve_workflow_path};
use crate::{
    models::{ExecutionExecutableSnapshot, PackReleasePin},
    runtime_authority::{
        AdmittedPermissionSet, ExternalApproval, InvocationBinding, InvocationPolicy,
        PackAuthorityOwner, PermissionScope, PermissionSetDefinition, RuntimePolicy,
    },
    schema::RefValidator,
    Error, Result,
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

struct CandidateInvocation {
    binding: InvocationBinding,
    refs: Option<Vec<String>>,
    include_target_defaults: bool,
    file: String,
    path: String,
    target_path: String,
}

impl CandidateInvocation {
    fn maximum_selection(&self, executable: &ExecutionExecutableSnapshot) -> Option<Vec<String>> {
        if !self.include_target_defaults {
            return self.refs.clone();
        }
        Some(
            executable
                .executable
                .action
                .default_execution_permission_set_refs
                .iter()
                .chain(self.refs.iter().flatten())
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
        )
    }
}

/// Full authored declarations, with source locations retained through compilation.
pub struct CandidateRuntimePolicyPlan {
    pack_ref: String,
    definitions: BTreeMap<String, (PermissionSetDefinition, String)>,
    invocations: Vec<CandidateInvocation>,
}

impl CandidateRuntimePolicyPlan {
    /// Structural API/YAML validation uses the same finite-target grammar and
    /// authored JSON pointers as deployment and managed readmission.
    pub fn validate_workflow_targets(
        definition: &Value,
        pack_ref: &str,
        workflow_ref: &str,
        file: &str,
    ) -> Result<()> {
        let mut plan = Self {
            pack_ref: pack_ref.into(),
            definitions: BTreeMap::new(),
            invocations: Vec::new(),
        };
        plan.read_tasks(definition, workflow_ref, file, "")
    }

    /// Capture and activate the same located plan used by deployment validation.
    /// Call after immutable executable capture and sponsor creation, in the
    /// activation transaction. This does not waive registrar authorization.
    pub async fn admit_deployment(
        &self,
        connection: &mut sqlx::PgConnection,
        pack_id: crate::models::Id,
        installer_identity: crate::models::Id,
    ) -> Result<crate::repositories::runtime_authority::RuntimePolicyRevision> {
        self.admit(connection, pack_id, installer_identity, None)
            .await
    }

    /// Use the deployment compiler for a component edit, retaining all unrelated
    /// invocation bodies and concrete ownership pins from the admitted policy.
    pub(crate) async fn admit_component(
        &self,
        connection: &mut sqlx::PgConnection,
        pack_id: crate::models::Id,
        editor: crate::models::Id,
        retained: RuntimePolicy,
    ) -> Result<crate::repositories::runtime_authority::RuntimePolicyRevision> {
        self.admit(connection, pack_id, editor, Some(retained))
            .await
    }

    async fn admit(
        &self,
        connection: &mut sqlx::PgConnection,
        pack_id: crate::models::Id,
        installer_identity: crate::models::Id,
        retained: Option<RuntimePolicy>,
    ) -> Result<crate::repositories::runtime_authority::RuntimePolicyRevision> {
        crate::repositories::schema_type::SchemaTypeRepository::lock_reclamation(connection)
            .await?;
        use crate::repositories::{
            executable_snapshot::ExecutableSnapshotRepository, identity::PermissionSetRepository,
            pack::PackRepository, runtime_authority::RuntimeAuthorityRepository, FindById,
        };
        let owner = RuntimeAuthorityRepository::find_owner(connection, pack_id)
            .await?
            .ok_or_else(|| Error::validation("Deployment runtime sponsor is unavailable"))?;
        let pack = PackRepository::find_by_id(&mut *connection, pack_id)
            .await?
            .ok_or_else(|| Error::validation("Deployment pack is unavailable"))?;
        let release_id = pack
            .active_release
            .ok_or_else(|| Error::validation("Deployment has no immutable code release"))?;
        if retained
            .as_ref()
            .is_some_and(|policy| policy.owner != owner || policy.release_id != release_id)
        {
            return Err(Error::validation(
                "Managed readmission source differs from its retained sponsor or code release",
            ));
        }
        if retained.is_none() {
            crate::repositories::managed_executable::ManagedExecutableRepository::capture_deployment_actions(
                connection, release_id,
            ).await?;
        }
        let mut executables =
            ExecutableSnapshotRepository::resolve_release_actions(connection, release_id).await?;
        if let Some(retained) = &retained {
            for invocation in &retained.invocations {
                if matches!(invocation.binding, InvocationBinding::Action { .. }) {
                    let snapshot = crate::repositories::managed_executable::ManagedExecutableRepository::resolve_invocation(
                        connection, retained, &invocation.binding,
                    ).await?;
                    executables.insert(invocation.binding.action_ref().into(), snapshot);
                }
            }
        }
        let mut required_refs = std::collections::BTreeSet::new();
        let mut bound_executables = BTreeMap::new();
        let mut retained_targets = BTreeMap::new();
        for candidate in &self.invocations {
            let target = candidate.binding.action_ref();
            let binding_key = serde_json::to_string(&candidate.binding)?;
            if let Some(retained_policy) = &retained {
                if let Some(previous) = retained_policy
                    .invocations
                    .iter()
                    .find(|entry| entry.binding == candidate.binding)
                {
                    let mut snapshot = crate::repositories::managed_executable::ManagedExecutableRepository::resolve_invocation(connection, retained_policy, &candidate.binding).await?;
                    snapshot.pack_executables.clear();
                    bound_executables.insert(binding_key.clone(), snapshot);
                    retained_targets.insert(binding_key.clone(), previous.clone());
                }
            }
            // Even a local immutable executable must still have its original
            // structural identity. Ref spelling is not ownership evidence.
            if let Some(snapshot) = bound_executables
                .get(&binding_key)
                .or_else(|| executables.get(target))
            {
                let current =
                    crate::repositories::action::ActionRepository::find_by_refs_for_share(
                        connection,
                        &[target.into()],
                    )
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        super::diagnostics::metadata_error(
                            &candidate.file,
                            &candidate.target_path,
                            "Invocation target action is unavailable",
                        )
                    })?;
                if current.id != snapshot.executable.action.id
                    || current.pack != snapshot.executable.action.pack
                {
                    return Err(Error::validation(
                        "Invocation target ownership changed during admission",
                    ));
                }
                let owning_pack = PackRepository::find_by_id(&mut *connection, current.pack)
                    .await?
                    .ok_or_else(|| {
                        Error::validation("Invocation target owning pack is unavailable")
                    })?;
                if current.pack_ref != owning_pack.r#ref
                    || snapshot.executable.action.pack_ref != owning_pack.r#ref
                {
                    return Err(super::diagnostics::metadata_error(
                        &candidate.file,
                        &candidate.target_path,
                        "Invocation target pack reference disagrees with its structural owner",
                    ));
                }
                crate::action_visibility::ensure_action_reference_allowed(
                    &current,
                    Some(&owner.pack_ref),
                    "invocation",
                    target,
                )
                .map_err(|error| {
                    super::diagnostics::located(error, &candidate.file, &candidate.target_path)
                })?;
            }
            if !bound_executables.contains_key(&binding_key) && !executables.contains_key(target) {
                let current =
                    crate::repositories::action::ActionRepository::find_by_refs_for_share(
                        connection,
                        &[target.into()],
                    )
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        super::diagnostics::metadata_error(
                            &candidate.file,
                            &candidate.target_path,
                            "Invocation target action is unavailable",
                        )
                    })?;
                let owning_pack = PackRepository::find_by_id(&mut *connection, current.pack)
                    .await?
                    .ok_or_else(|| {
                        Error::validation("Invocation target owning pack is unavailable")
                    })?;
                if current.pack_ref != owning_pack.r#ref {
                    return Err(super::diagnostics::metadata_error(
                        &candidate.file,
                        &candidate.target_path,
                        "Invocation target pack reference disagrees with its structural owner",
                    ));
                }
                crate::action_visibility::ensure_action_reference_allowed(
                    &current,
                    Some(&owner.pack_ref),
                    "invocation",
                    target,
                )
                .map_err(|error| {
                    super::diagnostics::located(error, &candidate.file, &candidate.target_path)
                })?;
                let mut executable =
                    ExecutableSnapshotRepository::resolve_for_action_ref(&mut *connection, target)
                        .await
                        .map_err(|error| {
                            super::diagnostics::located(
                                error,
                                &candidate.file,
                                &candidate.target_path,
                            )
                        })?;
                if current.id != executable.executable.action.id
                    || current.pack != executable.executable.action.pack
                    || current.pack_ref != executable.executable.action.pack_ref
                {
                    return Err(Error::validation(
                        "Invocation target ownership changed during deployment admission",
                    ));
                }
                if current.pack != owner.pack_id
                    && executable.executable.workflow_definition.is_some()
                {
                    executable = crate::repositories::workflow_policy::WorkflowPolicyRepository::deployment_executable(
                        connection, current.pack, target,
                    ).await.map_err(|error|super::diagnostics::located(error,&candidate.file,&candidate.path))?;
                }
                crate::action_visibility::ensure_action_reference_allowed(
                    &executable.executable.action,
                    Some(&owner.pack_ref),
                    "invocation",
                    target,
                )?;
                executables.insert(target.into(), executable);
            }
            let executable = if let Some(bound) = bound_executables.get(&binding_key) {
                bound
            } else {
                &executables[target]
            };
            let selected = candidate.maximum_selection(executable);
            required_refs.extend(
                selected
                    .as_deref()
                    .unwrap_or(
                        &executable
                            .executable
                            .action
                            .default_execution_permission_set_refs,
                    )
                    .iter()
                    .filter(|reference| reference.as_str() != "standard")
                    .filter(|reference| {
                        !retained
                            .as_ref()
                            .and_then(|policy| {
                                policy
                                    .invocations
                                    .iter()
                                    .find(|entry| entry.binding == candidate.binding)
                            })
                            .is_some_and(|entry| {
                                entry.permission_sets.contains_key(reference.as_str())
                            })
                    })
                    .cloned(),
            );
            let snapshot_for_binding = executable.clone();
            bound_executables
                .entry(binding_key)
                .or_insert(snapshot_for_binding);
        }
        let external_refs = required_refs
            .into_iter()
            .filter(|reference| !self.definitions.contains_key(reference))
            .collect::<Vec<_>>();
        let external_sets =
            PermissionSetRepository::find_by_refs_for_share(connection, &external_refs).await?;
        if external_sets.len() != external_refs.len() {
            return Err(Error::validation(
                "Selected runtime permission set is unavailable",
            ));
        }
        let mut external_grants = Vec::new();
        for (definition, _) in self.definitions.values() {
            external_grants.extend(
                definition
                    .grants
                    .iter()
                    .filter(|grant| {
                        grant.scope.unwrap_or(definition.scope) == PermissionScope::External
                    })
                    .cloned()
                    .map(|grant| grant.into_external_approval_grant())
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        let mut external_bodies = BTreeMap::new();
        for set in external_sets {
            // Repository reads retain managing_pack_id inside each effective
            // grant. External approval preserves that structural envelope.
            let grants: Vec<crate::rbac::Grant> = serde_json::from_value(set.grants)?;
            external_grants.extend(grants.iter().cloned());
            external_bodies.insert(set.r#ref, grants);
        }
        let approvals = RuntimeAuthorityRepository::admit_external_grants(
            connection,
            pack_id,
            installer_identity,
            &external_grants,
        )
        .await?;
        let external_sets = external_bodies
            .into_iter()
            .map(|(reference, grants)| {
                AdmittedPermissionSet::external(&reference, &owner, &grants, &approvals)
                    .map(|set| (reference, set))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let release = crate::repositories::pack_release::PackReleaseRepository::find_by_id(
            &mut *connection,
            release_id,
        )
        .await?
        .ok_or_else(|| Error::validation("Deployment code release is unavailable"))?;
        let pin = PackReleasePin {
            id: release.id,
            digest: release.digest,
            content_path: release.content_path,
        };
        let mut policy = self.compile_bound(
            owner,
            &pin,
            &executables,
            &approvals,
            &external_sets,
            Some((&bound_executables, retained.as_ref())),
        )?;
        for invocation in &mut policy.invocations {
            let binding_key = serde_json::to_string(&invocation.binding)?;
            invocation.metadata_revision=crate::repositories::managed_executable::ManagedExecutableRepository::revision_for_snapshot(
                connection,&bound_executables[&binding_key],
            ).await?;
            if let Some(previous) = retained_targets.get(&binding_key) {
                invocation.metadata_revision = previous.metadata_revision;
                invocation.foreign_workflow_policy = previous.foreign_workflow_policy.clone();
            }
        }
        if let Some(retained) = retained {
            policy.concrete_targets = retained.concrete_targets;
            policy.invocations.extend(
                retained
                    .invocations
                    .into_iter()
                    .filter(|entry| !self.replaces(&entry.binding)),
            );
        } else if let Some(previous) =
            RuntimeAuthorityRepository::retained_deployment_policy(connection, pack_id).await?
        {
            if previous.policy.owner != policy.owner {
                return Err(Error::validation(
                    "Retained runtime sponsor changed during deployment",
                ));
            }
            for invocation in &previous.policy.invocations {
                if policy
                    .invocations
                    .iter()
                    .any(|entry| entry.binding == invocation.binding)
                {
                    continue;
                }
                let Some(source_id) = invocation.source_id else {
                    continue;
                };
                let (table, reference) = match &invocation.binding {
                    InvocationBinding::Action { action_ref } => ("action", action_ref),
                    InvocationBinding::WorkflowTask { workflow_ref, .. } => {
                        ("action", workflow_ref)
                    }
                    InvocationBinding::Rule { rule_ref, .. } => ("rule", rule_ref),
                    InvocationBinding::Queue { queue_ref, .. } => ("work_queue", queue_ref),
                };
                // Presence alone is not retention: a newly declared source may
                // have replaced its targets. Only unchanged structural source
                // identities still projected from an older release qualify.
                let query=format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = $1 AND ref = $2 AND pack = $3 AND management_origin = 'pack' AND retired_at IS NULL AND managed_release IS NOT NULL AND managed_release <> $4)");
                let retained: bool = sqlx::query_scalar(&query)
                    .bind(source_id)
                    .bind(reference)
                    .bind(pack_id)
                    .bind(pin.id)
                    .fetch_one(&mut *connection)
                    .await?;
                if retained {
                    policy.invocations.push(invocation.clone());
                    policy
                        .concrete_targets
                        .extend(previous.policy.concrete_targets.clone());
                }
            }
        }
        let revision = RuntimeAuthorityRepository::capture_policy(connection, &policy).await?;
        RuntimeAuthorityRepository::activate_policy(connection, revision.id).await?;
        Ok(revision)
    }

    fn replaces(&self, binding: &InvocationBinding) -> bool {
        self.invocations
            .iter()
            .any(|candidate| match (&candidate.binding, binding) {
                (
                    InvocationBinding::WorkflowTask {
                        workflow_ref: left, ..
                    },
                    InvocationBinding::WorkflowTask {
                        workflow_ref: right,
                        ..
                    },
                ) => left == right,
                (
                    InvocationBinding::Action { action_ref: left },
                    InvocationBinding::Action { action_ref: right },
                ) => left == right,
                (
                    InvocationBinding::Rule { rule_ref: left, .. },
                    InvocationBinding::Rule {
                        rule_ref: right, ..
                    },
                ) => left == right,
                (
                    InvocationBinding::Queue {
                        queue_ref: left, ..
                    },
                    InvocationBinding::Queue {
                        queue_ref: right, ..
                    },
                ) => left == right,
                // A workflow with no remaining tasks still removes its old bindings.
                (
                    InvocationBinding::Action { action_ref },
                    InvocationBinding::WorkflowTask { workflow_ref, .. },
                ) => action_ref == workflow_ref,
                _ => false,
            })
    }

    pub(crate) fn component(
        pack_ref: &str,
        definition: Value,
        kind: &str,
        definitions: Vec<PermissionSetDefinition>,
    ) -> Result<Self> {
        let file = format!("api/{kind}");
        let mut plan = Self {
            pack_ref: pack_ref.into(),
            definitions: definitions
                .into_iter()
                .map(|value| (value.reference.clone(), (value, file.clone())))
                .collect(),
            invocations: Vec::new(),
        };
        let reference = component_ref(
            &definition,
            "ref",
            pack_ref,
            &file,
            "/ref",
            kind == "queues",
        )?;
        if kind == "workflows" {
            plan.invocations.push(CandidateInvocation {
                binding: InvocationBinding::Action {
                    action_ref: reference.clone(),
                },
                refs: references(
                    definition.get("default_execution_permission_set_refs"),
                    &file,
                    "/default_execution_permission_set_refs",
                )?,
                include_target_defaults: false,
                file: file.clone(),
                path: "/ref".into(),
                target_path: "/ref".into(),
            });
            plan.read_tasks(&definition["definition"], &reference, &file, "")?;
        } else {
            let target = definition["action_ref"]
                .as_str()
                .ok_or_else(|| Error::validation("Dispatch target is missing"))?
                .to_string();
            let binding = if kind == "rules" {
                InvocationBinding::Rule {
                    rule_ref: reference,
                    action_ref: target,
                }
            } else {
                InvocationBinding::Queue {
                    queue_ref: reference,
                    action_ref: target,
                }
            };
            plan.invocations.push(CandidateInvocation {
                binding,
                refs: references(
                    definition.get("permission_set_refs"),
                    &file,
                    "/permission_set_refs",
                )?,
                include_target_defaults: false,
                file,
                path: "/action_ref".into(),
                target_path: "/action_ref".into(),
            });
        }
        Ok(plan)
    }

    pub fn read(root: &Path, pack_ref: &str) -> Result<Self> {
        RefValidator::validate_pack_ref(pack_ref)?;
        let mut plan = Self {
            pack_ref: pack_ref.into(),
            definitions: BTreeMap::new(),
            invocations: Vec::new(),
        };
        for (name, content) in files(&root.join("permission_sets"))? {
            let file = format!("permission_sets/{name}");
            let value = parse(&content, &file)?;
            // Exact bundled catalog definitions remain platform-owned. They are
            // neither republished nor reinterpreted as pack-local declarations.
            if pack_ref == "core"
                && crate::platform_catalog::is_legacy_definition(
                    crate::platform_catalog::ManagedComponentKind::PermissionSet,
                    &value,
                )?
            {
                continue;
            }
            let definition: PermissionSetDefinition =
                serde_json::from_value(value).map_err(|_| {
                    super::diagnostics::metadata_error(
                        &file,
                        "",
                        "Invalid canonical permission set definition",
                    )
                })?;
            let reference = definition.reference.clone();
            if plan
                .definitions
                .insert(reference, (definition, file.clone()))
                .is_some()
            {
                return Err(super::diagnostics::metadata_error(
                    &file,
                    "/ref",
                    "Duplicate permission set definition",
                ));
            }
        }
        let mut action_refs = std::collections::BTreeSet::new();
        for (name, content) in files(&root.join("actions"))? {
            let file = format!("actions/{name}");
            let action = parse(&content, &file)?;
            let reference = component_ref(&action, "ref", pack_ref, &file, "/ref", false)?;
            if !action_refs.insert(reference.clone()) {
                return Err(super::diagnostics::metadata_error(
                    &file,
                    "/ref",
                    "Duplicate action definition",
                ));
            }
            plan.invocations.push(CandidateInvocation {
                include_target_defaults: false,
                binding: InvocationBinding::Action {
                    action_ref: reference.clone(),
                },
                refs: references(
                    action.get("default_execution_permission_set_refs"),
                    &file,
                    "/default_execution_permission_set_refs",
                )?,
                file: file.clone(),
                path: "/default_execution_permission_set_refs".into(),
                target_path: "/ref".into(),
            });
            if let Some(workflow) = action.get("workflow_file").and_then(Value::as_str) {
                let path = resolve_workflow_path(&root.join("actions"), workflow)?;
                let graph_file = format!("actions/{workflow}");
                let content = std::fs::read_to_string(path).map_err(|_| {
                    super::diagnostics::metadata_error(
                        &file,
                        "/workflow_file",
                        "Workflow graph file is unavailable",
                    )
                })?;
                plan.read_tasks(&parse(&content, &graph_file)?, &reference, &graph_file, "")?;
            }
        }
        let mut dispatch_refs = std::collections::BTreeSet::new();
        for folder in ["rules", "queues"] {
            for (name, content) in files(&root.join(folder))? {
                let file = format!("{folder}/{name}");
                let value = parse(&content, &file)?;
                let reference =
                    component_ref(&value, "ref", pack_ref, &file, "/ref", folder == "queues")?;
                if !dispatch_refs.insert((folder, reference.clone())) {
                    return Err(super::diagnostics::metadata_error(
                        &file,
                        "/ref",
                        "Duplicate dispatch component definition",
                    ));
                }
                let action_key = if folder == "queues" {
                    "dispatch_action"
                } else {
                    "action_ref"
                };
                let target = value.get(action_key).or_else(|| value.get("action"));
                let action = target.and_then(Value::as_str).ok_or_else(|| {
                    super::diagnostics::metadata_error(
                        &file,
                        &format!("/{action_key}"),
                        "Dispatch action ref is required",
                    )
                })?;
                let action = qualify(action, pack_ref);
                RefValidator::validate_component_ref(&action).map_err(|error| {
                    super::diagnostics::located(error, &file, &format!("/{action_key}"))
                })?;
                let binding = if folder == "queues" {
                    InvocationBinding::Queue {
                        queue_ref: reference,
                        action_ref: action,
                    }
                } else {
                    InvocationBinding::Rule {
                        rule_ref: reference,
                        action_ref: action,
                    }
                };
                plan.invocations.push(CandidateInvocation {
                    include_target_defaults: false,
                    binding,
                    refs: references(
                        value
                            .get("permission_set_refs")
                            .or_else(|| value.get("permission_set_ref")),
                        &file,
                        "/permission_set_refs",
                    )?,
                    file,
                    path: "/permission_set_refs".into(),
                    target_path: format!("/{action_key}"),
                });
            }
        }
        Ok(plan)
    }

    fn read_tasks(&mut self, value: &Value, workflow: &str, file: &str, base: &str) -> Result<()> {
        let tasks: Vec<(String, &Value)> = match value.get("tasks") {
            None => Vec::new(),
            Some(Value::Array(tasks)) => tasks
                .iter()
                .enumerate()
                .map(|(index, task)| (format!("{base}/tasks/{index}"), task))
                .collect(),
            Some(Value::Object(tasks)) => tasks
                .iter()
                .map(|(name, task)| {
                    (
                        format!("{base}/tasks{}", super::diagnostics::pointer(name)),
                        task,
                    )
                })
                .collect(),
            _ => {
                return Err(super::diagnostics::metadata_error(
                    file,
                    &format!("{base}/tasks"),
                    "Workflow tasks must be an array or object",
                ))
            }
        };
        for (path, task) in tasks {
            if let Some(action) = task.get("action").or_else(|| task.get("action_ref")) {
                let action = action.as_str().ok_or_else(|| {
                    super::diagnostics::metadata_error(
                        file,
                        &format!("{path}/action"),
                        "Workflow action ref must be a string",
                    )
                })?;
                let targets = task_action_targets(
                    action,
                    task.get("allowed_action_refs"),
                    &self.pack_ref,
                    file,
                    &path,
                )?;
                let target_path = if action.contains("{{") {
                    format!("{path}/allowed_action_refs")
                } else {
                    format!("{path}/action")
                };
                let selection = task
                    .get("permission_set_refs")
                    .or_else(|| task.get("permission_set_ref"));
                let (refs, include_target_defaults) =
                    task_references(selection, file, &format!("{path}/permission_set_refs"))?;
                for action in targets {
                    self.invocations.push(CandidateInvocation {
                        include_target_defaults,
                        binding: InvocationBinding::WorkflowTask {
                            workflow_ref: workflow.into(),
                            task_path: path.clone(),
                            action_ref: action,
                        },
                        refs: refs.clone(),
                        file: file.into(),
                        path: format!("{path}/permission_set_refs"),
                        target_path: target_path.clone(),
                    });
                }
            }
            self.read_tasks(task, workflow, file, &path)?;
        }
        Ok(())
    }

    /// `executables` must come from candidate graph admission and immutable
    /// release repositories. Ref keys alone are not ownership evidence.
    pub fn compile(
        &self,
        owner: PackAuthorityOwner,
        release: &PackReleasePin,
        executables: &BTreeMap<String, ExecutionExecutableSnapshot>,
        approvals: &[ExternalApproval],
        externally_admitted_sets: &BTreeMap<String, AdmittedPermissionSet>,
    ) -> Result<RuntimePolicy> {
        self.compile_bound(
            owner,
            release,
            executables,
            approvals,
            externally_admitted_sets,
            None,
        )
    }

    fn compile_bound(
        &self,
        owner: PackAuthorityOwner,
        release: &PackReleasePin,
        executables: &BTreeMap<String, ExecutionExecutableSnapshot>,
        approvals: &[ExternalApproval],
        externally_admitted_sets: &BTreeMap<String, AdmittedPermissionSet>,
        bound_executables: Option<(
            &BTreeMap<String, ExecutionExecutableSnapshot>,
            Option<&RuntimePolicy>,
        )>,
    ) -> Result<RuntimePolicy> {
        if owner.pack_ref != self.pack_ref {
            return Err(Error::validation(
                "Candidate policy owner does not match the declaring pack",
            ));
        }
        let mut sets = externally_admitted_sets.clone();
        for (reference, (definition, file)) in &self.definitions {
            if sets.contains_key(reference) {
                return Err(super::diagnostics::metadata_error(
                    file,
                    "/ref",
                    "Candidate policy cannot shadow an externally admitted permission set",
                ));
            }
            let set = definition
                .compile(&owner, approvals)
                .map_err(|error| super::diagnostics::located(error, file, "/grants"))?;
            sets.insert(reference.clone(), set);
        }
        let mut invocations = Vec::new();
        for candidate in &self.invocations {
            let binding_key = serde_json::to_string(&candidate.binding)?;
            let executable = bound_executables
                .and_then(|(targets, _)| targets.get(&binding_key))
                .or_else(|| executables.get(candidate.binding.action_ref()))
                .ok_or_else(|| {
                    super::diagnostics::metadata_error(
                        &candidate.file,
                        &candidate.target_path,
                        "Invocation action has no admitted executable snapshot",
                    )
                })?;
            let selected = candidate.maximum_selection(executable);
            let mut candidate_sets = sets.clone();
            if let Some((_, Some(retained))) = bound_executables {
                if let Some(previous) = retained
                    .invocations
                    .iter()
                    .find(|entry| entry.binding == candidate.binding)
                {
                    candidate_sets.extend(previous.permission_sets.clone());
                }
            }
            let invocation = InvocationPolicy::from_executable(
                candidate.binding.clone(),
                executable,
                selected.as_deref(),
                &candidate_sets,
            )
            .map_err(|error| {
                super::diagnostics::located(error, &candidate.file, &candidate.path)
            })?;
            invocations.push(invocation);
        }
        let policy = RuntimePolicy {
            concrete_targets: BTreeMap::new(),
            owner,
            release_id: release.id,
            release_digest: release.digest.clone(),
            invocations,
        };
        policy.validate()?;
        Ok(policy)
    }
}

fn files(path: &Path) -> Result<Vec<(String, String)>> {
    if path.exists() {
        read_yaml_files(path)
    } else {
        Ok(Vec::new())
    }
}

fn parse(content: &str, file: &str) -> Result<Value> {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(content)
        .map_err(|error| super::diagnostics::yaml_error(file, &error))?;
    serde_json::to_value(value).map_err(Into::into)
}

fn qualify(reference: &str, pack: &str) -> String {
    if reference.contains('.') {
        reference.into()
    } else {
        format!("{pack}.{reference}")
    }
}

/// Literal finite action ceiling shared by candidate compilation and immutable
/// source verification. No template or prefix can serve as a target proof.
pub(crate) fn task_action_targets(
    action: &str,
    allowed: Option<&Value>,
    pack: &str,
    file: &str,
    path: &str,
) -> Result<Vec<String>> {
    let dynamic = action.contains("{{");
    let targets = match allowed {
        Some(Value::Array(values)) if !values.is_empty() && values.len() <= 256 => values
            .iter()
            .map(|value| {
                let reference = value.as_str().ok_or_else(|| {
                    super::diagnostics::metadata_error(
                        file,
                        &format!("{path}/allowed_action_refs"),
                        "Allowed action refs must be literal strings",
                    )
                })?;
                RefValidator::validate_component_ref(reference).map_err(|error| {
                    super::diagnostics::located(error, file, &format!("{path}/allowed_action_refs"))
                })?;
                Ok(reference.to_string())
            })
            .collect::<Result<std::collections::BTreeSet<_>>>()?
            .into_iter()
            .collect::<Vec<_>>(),
        None if !dynamic => Vec::new(),
        _ => {
            return Err(super::diagnostics::metadata_error(
                file,
                &format!("{path}/allowed_action_refs"),
                "Dynamic actions require 1 to 256 literal allowed_action_refs",
            ))
        }
    };
    if dynamic {
        return Ok(targets);
    }
    let action = qualify(action, pack);
    RefValidator::validate_component_ref(&action)
        .map_err(|error| super::diagnostics::located(error, file, &format!("{path}/action")))?;
    if allowed.is_some() && !targets.contains(&action) {
        return Err(super::diagnostics::metadata_error(
            file,
            &format!("{path}/action"),
            "Literal action is outside allowed_action_refs",
        ));
    }
    Ok(vec![action])
}

fn component_ref(
    value: &Value,
    field: &str,
    pack: &str,
    file: &str,
    path: &str,
    queue: bool,
) -> Result<String> {
    let reference = value.get(field).and_then(Value::as_str).ok_or_else(|| {
        super::diagnostics::metadata_error(file, path, "Component ref is required")
    })?;
    let reference = qualify(reference, pack);
    if reference.split('.').next() != Some(pack) {
        return Err(super::diagnostics::metadata_error(
            file,
            path,
            "Declared component belongs to another pack",
        ));
    }
    (if queue {
        RefValidator::validate_work_queue_ref(&reference)
    } else {
        RefValidator::validate_component_ref(&reference)
    })
    .map_err(|error| super::diagnostics::located(error, file, path))?;
    Ok(reference)
}

fn references(value: Option<&Value>, file: &str, path: &str) -> Result<Option<Vec<String>>> {
    let refs = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value.as_str().map(String::from).ok_or_else(|| {
                    super::diagnostics::metadata_error(
                        file,
                        path,
                        "Permission refs must be strings",
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?,
        _ => {
            return Err(super::diagnostics::metadata_error(
                file,
                path,
                "Permission refs must be a string or array",
            ))
        }
    };
    for reference in &refs {
        if reference != "standard" {
            RefValidator::validate_component_ref(reference)
                .map_err(|error| super::diagnostics::located(error, file, path))?;
        }
    }
    Ok(Some(refs))
}

fn task_references(
    value: Option<&Value>,
    file: &str,
    path: &str,
) -> Result<(Option<Vec<String>>, bool)> {
    let values = match value {
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value.as_str().map(String::from).ok_or_else(|| {
                    super::diagnostics::metadata_error(
                        file,
                        path,
                        "Permission refs must be strings",
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?,
        _ => return references(value, file, path).map(|refs| (refs, false)),
    };
    let dynamic = values.iter().any(|value| value.contains("{{"));
    if !dynamic {
        return references(value, file, path).map(|refs| (refs, false));
    }
    let literal = values
        .into_iter()
        .filter(|value| !value.contains("{{"))
        .map(Value::String)
        .collect::<Vec<_>>();
    // A rendered task can reduce its action's admitted default maximum plus
    // literal selections declared by this task. Templates never manufacture an
    // unbounded set of runtime grants.
    Ok((references(Some(&Value::Array(literal)), file, path)?, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dynamic_actions_have_exact_finite_located_target_bindings() {
        let plan = CandidateRuntimePolicyPlan::component("example", serde_json::json!({
            "ref": "example.flow", "definition": {"tasks": {"group/name": {
                "action": "{{ parameters.target }}", "allowed_action_refs": ["example.one", "foreign.two", "example.one"],
                "permission_set_refs": ["{{ parameters.permissions }}", "example.extra"]
            }}}
        }), "workflows", Vec::new()).unwrap();
        assert_eq!(plan.invocations.len(), 3);
        for (entry, expected) in plan.invocations[1..]
            .iter()
            .zip(["example.one", "foreign.two"])
        {
            assert!(
                matches!(&entry.binding, InvocationBinding::WorkflowTask { task_path, action_ref, .. } if task_path == "/tasks/group~1name" && action_ref == expected)
            );
            assert!(entry.include_target_defaults);
            assert_eq!(
                entry.refs.as_deref(),
                Some(["example.extra".into()].as_slice())
            );
        }
        assert!(plan.replaces(&InvocationBinding::WorkflowTask {
            workflow_ref: "example.flow".into(),
            task_path: "/tasks/deleted".into(),
            action_ref: "example.removed".into()
        }));
        assert!(!plan.replaces(&InvocationBinding::Action {
            action_ref: "example.other".into()
        }));
    }

    #[test]
    fn dynamic_targets_reject_missing_empty_nonliteral_and_prefix_ceilings() {
        for allowed in [
            Value::Null,
            serde_json::json!([]),
            serde_json::json!([42]),
            serde_json::json!(["example.*"]),
            serde_json::json!(["{{ parameters.target }}"]),
        ] {
            let error = task_action_targets(
                "{{ parameters.target }}",
                Some(&allowed),
                "example",
                "actions/workflows/flow.yaml",
                "/tasks/0",
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("actions/workflows/flow.yaml"), "{error}");
            assert!(error.contains("/tasks/0/allowed_action_refs"), "{error}");
        }
        assert!(task_action_targets(
            "{{ parameters.target }}",
            None,
            "example",
            "workflow.yaml",
            "/tasks/0"
        )
        .is_err());
        assert!(task_action_targets(
            "example.other",
            Some(&serde_json::json!(["example.allowed"])),
            "example",
            "workflow.yaml",
            "/tasks/0"
        )
        .is_err());
        assert!(task_action_targets(
            "{{ parameters.target }}",
            Some(&serde_json::json!(vec!["example.target"; 257])),
            "example",
            "workflow.yaml",
            "/tasks/0"
        )
        .is_err());
    }

    #[test]
    fn dynamic_ceiling_survives_yaml_parser_and_snapshot_round_trip() {
        let yaml = "version: '1.0'\ntasks:\n  - name: choose\n    action: '{{ parameters.target }}'\n    allowed_action_refs: [example.one, foreign.two]\n";
        let parsed: crate::workflow::parser::WorkflowDefinition =
            serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(
            parsed.tasks[0].allowed_action_refs,
            vec!["example.one", "foreign.two"]
        );
        let snapshot = serde_json::to_value(&parsed).unwrap();
        let restored: crate::workflow::parser::WorkflowDefinition =
            serde_json::from_value(snapshot).unwrap();
        assert_eq!(
            restored.tasks[0].allowed_action_refs,
            parsed.tasks[0].allowed_action_refs
        );
        assert_eq!(
            crate::action_visibility::collect_workflow_action_refs(&restored),
            vec!["example.one", "foreign.two"]
        );
    }

    #[test]
    fn dynamic_source_proof_uses_canonical_escaped_names_and_denies_numeric_ambiguity() {
        use crate::repositories::workflow_policy::task_at_authored_path;
        let graph = serde_json::json!({"tasks":[{"name":"group/name", "tasks":[{
            "name":"choose", "action":"{{ parameters.target }}", "allowed_action_refs":["example.one"]
        }]}]});
        let task = task_at_authored_path(&graph, "/tasks/group~1name/tasks/choose").unwrap();
        assert_eq!(
            task_action_targets(
                task["action"].as_str().unwrap(),
                task.get("allowed_action_refs"),
                "example",
                "workflow.yaml",
                "/tasks/group~1name/tasks/choose"
            )
            .unwrap(),
            vec!["example.one"]
        );
        let ambiguous = serde_json::json!({"tasks":[{"name":"other"},{"name":"0","action":"{{ parameters.target }}","allowed_action_refs":["example.one"]}]});
        assert!(task_at_authored_path(&ambiguous, "/tasks/0").is_none());
    }
    #[test]
    fn selection_preserves_omission_and_explicit_empty() {
        assert_eq!(
            references(None, "action.yaml", "/permission_set_refs").unwrap(),
            None
        );
        assert_eq!(
            references(
                Some(&serde_json::json!([])),
                "action.yaml",
                "/permission_set_refs"
            )
            .unwrap(),
            Some(Vec::new())
        );
        assert!(references(
            Some(&serde_json::json!([42])),
            "action.yaml",
            "/permission_set_refs"
        )
        .is_err());
        assert!(references(
            Some(&serde_json::json!(["{{ parameters.permissions }}"])),
            "action.yaml",
            "/permission_set_refs"
        )
        .is_err());
    }

    #[test]
    fn rendered_task_selections_use_a_bounded_default_maximum_and_literal_additions() {
        let (refs, dynamic) = task_references(
            Some(&serde_json::json!([
                "{{ parameters.selected }}",
                "example.extra"
            ])),
            "workflow.yaml",
            "/tasks/0/permission_set_refs",
        )
        .unwrap();
        assert!(dynamic);
        assert_eq!(refs, Some(vec!["example.extra".into()]));
        let (refs, dynamic) = task_references(
            Some(&serde_json::json!([])),
            "workflow.yaml",
            "/tasks/0/permission_set_refs",
        )
        .unwrap();
        assert!(!dynamic);
        assert_eq!(refs, Some(Vec::new()));
    }

    #[test]
    fn exact_platform_definitions_are_not_reinterpreted_as_local_pack_policy() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("permission_sets")).unwrap();
        let file = root.path().join("permission_sets/admin.yaml");
        std::fs::write(
            &file,
            include_str!("../platform_catalog/permission_sets/admin.yaml"),
        )
        .unwrap();
        let plan = CandidateRuntimePolicyPlan::read(root.path(), "core").unwrap();
        assert!(plan.definitions.is_empty());
        let mut changed = parse(
            include_str!("../platform_catalog/permission_sets/admin.yaml"),
            "admin.yaml",
        )
        .unwrap();
        changed["description"] = Value::String("Not the platform definition".into());
        std::fs::write(file, serde_yaml_ng::to_string(&changed).unwrap()).unwrap();
        let altered = CandidateRuntimePolicyPlan::read(root.path(), "core").unwrap();
        assert!(altered.definitions.contains_key("core.admin"));
    }
    #[test]
    fn unused_external_definitions_still_require_approval() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("permission_sets")).unwrap();
        std::fs::write(root.path().join("permission_sets/external.yaml"), "ref: example.external\nscope: external\ngrants:\n  - resource: events\n    actions: [create]\n").unwrap();
        let plan = CandidateRuntimePolicyPlan::read(root.path(), "example").unwrap();
        let owner = PackAuthorityOwner {
            pack_id: 1,
            pack_ref: "example".into(),
            sponsor_identity_id: 2,
        };
        let release = PackReleasePin {
            id: 3,
            digest: "a".repeat(64),
            content_path: "/owned".into(),
        };
        assert!(plan
            .compile(owner, &release, &BTreeMap::new(), &[], &BTreeMap::new())
            .is_err());
    }
    #[test]
    fn task_paths_preserve_nested_and_escaped_named_identity() {
        let mut plan = CandidateRuntimePolicyPlan {
            pack_ref: "example".into(),
            definitions: BTreeMap::new(),
            invocations: Vec::new(),
        };
        plan.read_tasks(&serde_json::json!({"tasks": {"group/name": {"tasks": [{"action": "later", "permission_set_refs": []}]}}}), "example.workflow", "actions/workflows/main.yaml", "").unwrap();
        assert_eq!(plan.invocations.len(), 1);
        assert!(
            matches!(&plan.invocations[0].binding, InvocationBinding::WorkflowTask { task_path, action_ref, .. }
            if task_path == "/tasks/group~1name/tasks/0" && action_ref == "example.later")
        );
        assert_eq!(plan.invocations[0].refs, Some(Vec::new()));
    }
}
