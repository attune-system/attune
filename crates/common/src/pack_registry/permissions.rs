//! Permission-bearing metadata admission against the pre-mutation actor authority.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;
use sqlx::PgConnection;

use crate::{
    delegation::{named_refs, DelegationAuthority},
    rbac::{validate_cache_grant_constraints, Grant, Resource},
    repositories::{action::ActionRepository, identity::PermissionSetRepository},
    Error, Result,
};

use super::loader::{read_yaml_files, resolve_workflow_path};

#[derive(Default)]
pub struct PackPermissionPlan {
    definitions: BTreeMap<String, Vec<Grant>>,
    action_defaults: BTreeMap<String, Vec<String>>,
    uses: Vec<PermissionUse>,
}

struct PermissionUse {
    refs: Option<Vec<String>>,
    action: Option<String>,
    cache_only: bool,
}

impl PackPermissionPlan {
    pub fn read(pack_dir: &Path, pack_ref: &str) -> Result<Self> {
        let mut plan = Self::default();
        for (_, content) in files(&pack_dir.join("permission_sets"))? {
            let data = parse(&content)?;
            let reference = data["ref"]
                .as_str()
                .ok_or_else(|| Error::validation("Permission set definition requires ref"))?
                .to_string();
            let grants: Vec<Grant> = serde_json::from_value(
                data.get("grants")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new())),
            )?;
            validate_grants(&grants)?;
            if plan.definitions.insert(reference, grants).is_some() {
                return Err(Error::validation("Duplicate permission set definition"));
            }
        }
        let actions_dir = pack_dir.join("actions");
        for (_, content) in files(&actions_dir)? {
            let data = parse(&content)?;
            let refs = literal_refs(data.get("default_execution_permission_set_refs"), false)?
                .unwrap_or_default();
            if let Some(reference) = data["ref"].as_str() {
                plan.action_defaults
                    .insert(qualify(reference, pack_ref), refs.clone());
            }
            plan.uses.push(PermissionUse {
                refs: Some(refs),
                action: None,
                cache_only: false,
            });
            if let Some(workflow_file) = data["workflow_file"].as_str() {
                let path = resolve_workflow_path(&actions_dir, workflow_file)?;
                if path.exists() {
                    plan.read_tasks(
                        &parse(
                            &std::fs::read_to_string(path)
                                .map_err(|error| Error::io(error.to_string()))?,
                        )?,
                        pack_ref,
                    )?;
                }
            }
        }
        for folder in ["rules", "queues"] {
            for (_, content) in files(&pack_dir.join(folder))? {
                let data = parse(&content)?;
                let refs = literal_refs(
                    data.get("permission_set_refs")
                        .or_else(|| data.get("permission_set_ref")),
                    false,
                )?;
                let action = data
                    .get("action_ref")
                    .or_else(|| data.get("dispatch_action"))
                    .or_else(|| data.get("action"))
                    .and_then(Value::as_str)
                    .map(|reference| qualify(reference, pack_ref));
                plan.uses.push(PermissionUse {
                    refs,
                    action,
                    cache_only: false,
                });
            }
        }
        for (_, content) in files(&pack_dir.join("sensors"))? {
            let data = parse(&content)?;
            let refs = literal_refs(
                data.get("config")
                    .and_then(|config| config.get("cache_permission_set_refs")),
                false,
            )?;
            if refs.is_some() {
                plan.uses.push(PermissionUse {
                    refs,
                    action: None,
                    cache_only: true,
                });
            }
        }
        Ok(plan)
    }

    fn read_tasks(&mut self, workflow: &Value, pack_ref: &str) -> Result<()> {
        let tasks: Vec<&Value> = match workflow.get("tasks") {
            Some(Value::Array(tasks)) => tasks.iter().collect(),
            Some(Value::Object(tasks)) => tasks.values().collect(),
            _ => Vec::new(),
        };
        for task in tasks {
            let refs = literal_refs(
                task.get("permission_set_refs")
                    .or_else(|| task.get("permission_set_ref")),
                true,
            )?;
            let action = task
                .get("action")
                .or_else(|| task.get("action_ref"))
                .and_then(Value::as_str)
                .filter(|reference| !reference.contains("{{"))
                .map(|reference| qualify(reference, pack_ref));
            self.uses.push(PermissionUse {
                refs,
                action,
                cache_only: false,
            });
            self.read_tasks(task, pack_ref)?;
        }
        Ok(())
    }

    pub async fn validate(
        &self,
        conn: &mut PgConnection,
        authority: &DelegationAuthority,
    ) -> Result<()> {
        for grants in self.definitions.values() {
            if !authority.manages_permissions() && !authority.covers(grants) {
                return Err(Error::PermissionDenied(
                    "Pack permission definitions exceed the registering identity's authority"
                        .into(),
                ));
            }
        }
        let external_actions: Vec<String> = self
            .uses
            .iter()
            .filter(|usage| usage.refs.is_none())
            .filter_map(|usage| usage.action.as_ref())
            .filter(|reference| !self.action_defaults.contains_key(*reference))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let actions =
            ActionRepository::find_by_refs_for_share(&mut *conn, &external_actions).await?;
        let external_defaults: BTreeMap<_, _> = actions
            .into_iter()
            .map(|action| (action.r#ref, action.default_execution_permission_set_refs))
            .collect();
        let mut resolved = Vec::new();
        let mut references = BTreeSet::new();
        for usage in &self.uses {
            let refs = usage.refs.as_ref().or_else(|| {
                usage.action.as_ref().and_then(|action| {
                    self.action_defaults
                        .get(action)
                        .or_else(|| external_defaults.get(action))
                })
            });
            let refs = named_refs(refs.map(Vec::as_slice).unwrap_or(&[]))?;
            references.extend(
                refs.iter()
                    .filter(|reference| !self.definitions.contains_key(*reference))
                    .cloned(),
            );
            resolved.push((refs, usage.cache_only));
        }
        let references: Vec<_> = references.into_iter().collect();
        let sets = PermissionSetRepository::find_by_refs_for_share(&mut *conn, &references).await?;
        if sets.len() != references.len() {
            return Err(Error::PermissionDenied(
                "Pack references unavailable execution permission sets".into(),
            ));
        }
        let mut grants_by_ref = self.definitions.clone();
        for set in sets {
            grants_by_ref.insert(set.r#ref, serde_json::from_value::<Vec<Grant>>(set.grants)?);
        }
        for (refs, cache_only) in resolved {
            let grants: Vec<Grant> = refs
                .iter()
                .flat_map(|reference| grants_by_ref[reference].iter())
                .filter(|grant| !cache_only || grant.resource == Resource::Caches)
                .cloned()
                .collect();
            authority.require_grants(&grants)?;
        }
        Ok(())
    }
}

fn files(directory: &Path) -> Result<Vec<(String, String)>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    read_yaml_files(directory)
}

fn parse(content: &str) -> Result<Value> {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(content).map_err(|error| {
        Error::validation(format!("Invalid permission-bearing pack metadata: {error}"))
    })?;
    serde_json::to_value(value).map_err(Into::into)
}

fn qualify(reference: &str, pack_ref: &str) -> String {
    if reference.contains('.') {
        reference.to_string()
    } else {
        format!("{pack_ref}.{reference}")
    }
}

fn literal_refs(value: Option<&Value>, allow_expressions: bool) -> Result<Option<Vec<String>>> {
    let mut refs = Vec::new();
    match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(reference)) => refs.push(reference.clone()),
        Some(Value::Array(values)) => {
            for value in values {
                refs.push(
                    value
                        .as_str()
                        .ok_or_else(|| Error::validation("Permission set refs must be strings"))?
                        .to_string(),
                );
            }
        }
        _ => {
            return Err(Error::validation(
                "Permission set refs must be strings or an array of strings",
            ))
        }
    }
    for reference in &refs {
        if reference.trim().is_empty() {
            return Err(Error::validation("Permission set refs cannot be empty"));
        }
        if reference.contains("{{") && !allow_expressions {
            return Err(Error::validation(
                "Execution permission references cannot be expressions in this component",
            ));
        }
    }
    // Rendered task expressions are checked at child creation and token use.
    refs.retain(|reference| !reference.contains("{{"));
    Ok(Some(refs))
}

fn validate_grants(grants: &[Grant]) -> Result<()> {
    for grant in grants {
        if grant.actions.is_empty() {
            return Err(Error::validation("Permission grants require actions"));
        }
        if let Some(scope) = &grant.constraints {
            if scope.ids.is_some() {
                return Err(Error::validation(
                    "Permission grants use metadata refs, not database IDs",
                ));
            }
            if scope.pack_refs.is_some() && scope.refs.is_some() {
                return Err(Error::validation(
                    "Permission grants cannot combine pack and component refs",
                ));
            }
            for values in [&scope.pack_refs, &scope.refs, &scope.owner_refs] {
                if values.as_ref().is_some_and(|values| {
                    values.is_empty() || values.iter().any(|value| value.trim().is_empty())
                }) {
                    return Err(Error::validation("Permission scope refs must be nonempty"));
                }
            }
            if grant.resource == Resource::Caches {
                validate_cache_grant_constraints(scope).map_err(Error::validation)?;
            }
        }
    }
    Ok(())
}
