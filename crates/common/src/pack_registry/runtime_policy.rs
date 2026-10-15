//! Candidate runtime policy compilation. This does not bypass deployment checks
//! or grant runtime access before the production admission consumers are converted.

use super::loader::{read_yaml_files, resolve_workflow_path};
use crate::{
    models::{ExecutionExecutableSnapshot, PackReleasePin},
    runtime_authority::{
        AdmittedPermissionSet, ExternalApproval, InvocationBinding, InvocationPolicy,
        PackAuthorityOwner, PermissionSetDefinition, RuntimePolicy,
    },
    schema::RefValidator,
    Error, Result,
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

struct CandidateInvocation {
    binding: InvocationBinding,
    refs: Option<Vec<String>>,
    file: String,
    path: String,
}

/// Full authored declarations, with source locations retained through compilation.
pub struct CandidateRuntimePolicyPlan {
    pack_ref: String,
    definitions: BTreeMap<String, (PermissionSetDefinition, String)>,
    invocations: Vec<CandidateInvocation>,
}

impl CandidateRuntimePolicyPlan {
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
                if action.contains("{{") {
                    return Err(super::diagnostics::metadata_error(
                        file,
                        &format!("{path}/action"),
                        "Dynamic action selection requires an explicit admitted target set",
                    ));
                }
                let action = qualify(action, &self.pack_ref);
                RefValidator::validate_component_ref(&action).map_err(|error| {
                    super::diagnostics::located(error, file, &format!("{path}/action"))
                })?;
                self.invocations.push(CandidateInvocation {
                    binding: InvocationBinding::WorkflowTask {
                        workflow_ref: workflow.into(),
                        task_path: path.clone(),
                        action_ref: action,
                    },
                    refs: references(
                        task.get("permission_set_refs")
                            .or_else(|| task.get("permission_set_ref")),
                        file,
                        &format!("{path}/permission_set_refs"),
                    )?,
                    file: file.into(),
                    path: format!("{path}/permission_set_refs"),
                });
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
            let executable = executables
                .get(candidate.binding.action_ref())
                .ok_or_else(|| {
                    super::diagnostics::metadata_error(
                        &candidate.file,
                        &candidate.path,
                        "Invocation action has no admitted executable snapshot",
                    )
                })?;
            let invocation = InvocationPolicy::from_executable(
                candidate.binding.clone(),
                executable,
                candidate.refs.as_deref(),
                &sets,
            )
            .map_err(|error| {
                super::diagnostics::located(error, &candidate.file, &candidate.path)
            })?;
            invocations.push(invocation);
        }
        let policy = RuntimePolicy {
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

#[cfg(test)]
mod tests {
    use super::*;
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
