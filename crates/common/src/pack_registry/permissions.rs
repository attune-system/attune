//! Permission-bearing metadata admission against pre-mutation deployment authority.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;
use sqlx::PgConnection;

use crate::{
    delegation::{named_refs, DelegationAuthority},
    rbac::{validate_cache_grant_constraints, Action, AuthorizationContext, Grant, Resource},
    repositories::{action::ActionRepository, identity::PermissionSetRepository},
    Error, Result,
};

use super::loader::{read_yaml_files, resolve_workflow_path};

#[derive(Default)]
pub struct PackPermissionPlan {
    pack_ref: String,
    definition_sources: BTreeMap<String, String>,
    definitions: BTreeMap<String, Vec<Grant>>,
    action_defaults: BTreeMap<String, Vec<String>>,
    uses: Vec<PermissionUse>,
}

struct PermissionUse {
    source_file: String,
    metadata_path: String,
    refs: Option<Vec<String>>,
    action: Option<String>,
    cache_only: bool,
    signing_only: bool,
    input_only: bool,
}

impl PackPermissionPlan {
    pub fn read(pack_dir: &Path, pack_ref: &str) -> Result<Self> {
        let mut plan = Self {
            pack_ref: pack_ref.into(),
            ..Self::default()
        };
        for (name, content) in files(&pack_dir.join("permission_sets"))? {
            let file = format!("permission_sets/{name}");
            let data = parse(&content, &file)?;
            let reference = data["ref"]
                .as_str()
                .ok_or_else(|| Error::validation("Permission set definition requires ref"))?
                .to_string();
            let definition: crate::runtime_authority::PermissionSetDefinition =
                serde_json::from_value(data.clone()).map_err(|_| {
                    super::diagnostics::metadata_error(
                        &file,
                        "/grants",
                        "Invalid canonical permission set definition",
                    )
                })?;
            let grants = definition.registrar_grants()?;
            // Exact catalog definitions are platform-owned, not republished
            // pack-local IAM declarations. Keep the bootstrap exception exact.
            if !(pack_ref == "core"
                && crate::platform_catalog::is_legacy_definition(
                    crate::platform_catalog::ManagedComponentKind::PermissionSet,
                    &data,
                )?)
            {
                definition
                    .validate_publication(pack_ref)
                    .map_err(|error| super::diagnostics::located(error, &file, "/grants"))?;
            }
            validate_grants(&grants)
                .map_err(|error| super::diagnostics::located(error, &file, "/grants"))?;
            plan.definition_sources
                .insert(reference.clone(), file.clone());
            if plan.definitions.insert(reference, grants).is_some() {
                return Err(super::diagnostics::metadata_error(
                    &file,
                    "/ref",
                    "Duplicate permission set definition",
                ));
            }
        }
        let actions_dir = pack_dir.join("actions");
        for (name, content) in files(&actions_dir)? {
            let file = format!("actions/{name}");
            let data = parse(&content, &file)?;
            let refs = literal_refs(data.get("default_execution_permission_set_refs"), false)
                .map_err(|error| {
                    super::diagnostics::located(
                        error,
                        &file,
                        "/default_execution_permission_set_refs",
                    )
                })?
                .unwrap_or_default();
            if let Some(reference) = data["ref"].as_str() {
                plan.action_defaults
                    .insert(qualify(reference, pack_ref), refs.clone());
            }
            plan.uses.push(PermissionUse {
                source_file: file.clone(),
                metadata_path: "/default_execution_permission_set_refs".into(),
                refs: Some(refs),
                action: None,
                cache_only: false,
                signing_only: false,
                input_only: false,
            });
            if let Some(workflow_file) = data["workflow_file"].as_str() {
                let path = resolve_workflow_path(&actions_dir, workflow_file)?;
                if path.exists() {
                    plan.read_tasks(
                        &parse(
                            &std::fs::read_to_string(path)
                                .map_err(|error| Error::io(error.to_string()))?,
                            &format!("actions/{workflow_file}"),
                        )?,
                        pack_ref,
                        &format!("actions/{workflow_file}"),
                        "",
                    )?;
                }
            }
        }
        for folder in ["rules", "queues"] {
            for (name, content) in files(&pack_dir.join(folder))? {
                let file = format!("{folder}/{name}");
                let data = parse(&content, &file)?;
                if folder == "rules" {
                    if let Some(refs) = literal_refs(data.get("input_permission_set_refs"), false)
                        .map_err(|error| {
                        super::diagnostics::located(error, &file, "/input_permission_set_refs")
                    })? {
                        if refs.iter().any(|reference| reference == "standard") {
                            return Err(super::diagnostics::metadata_error(
                                &file,
                                "/input_permission_set_refs",
                                "Rule input reads require explicit named permission sets",
                            ));
                        }
                        plan.uses.push(PermissionUse {
                            source_file: file.clone(),
                            metadata_path: "/input_permission_set_refs".into(),
                            refs: Some(refs),
                            action: None,
                            cache_only: false,
                            signing_only: false,
                            input_only: true,
                        });
                    }
                }
                let refs = literal_refs(
                    data.get("permission_set_refs")
                        .or_else(|| data.get("permission_set_ref")),
                    false,
                )
                .map_err(|error| {
                    super::diagnostics::located(error, &file, "/permission_set_refs")
                })?;
                let action = data
                    .get("action_ref")
                    .or_else(|| data.get("dispatch_action"))
                    .or_else(|| data.get("action"))
                    .and_then(Value::as_str)
                    .map(|reference| qualify(reference, pack_ref));
                plan.uses.push(PermissionUse {
                    source_file: file.clone(),
                    metadata_path: "/permission_set_refs".into(),
                    refs,
                    action,
                    cache_only: false,
                    signing_only: false,
                    input_only: false,
                });
            }
        }
        for (name, content) in files(&pack_dir.join("sensors"))? {
            let file = format!("sensors/{name}");
            let data = parse(&content, &file)?;
            let refs = literal_refs(
                data.get("config")
                    .and_then(|config| config.get("cache_permission_set_refs")),
                false,
            )?;
            if refs.is_some() {
                plan.uses.push(PermissionUse {
                    source_file: file.clone(),
                    metadata_path: "/config/cache_permission_set_refs".into(),
                    refs,
                    action: None,
                    cache_only: true,
                    signing_only: false,
                    input_only: false,
                });
            }
            let refs = literal_refs(
                data.get("config")
                    .and_then(|config| config.get("signing_permission_set_refs")),
                false,
            )?;
            if let Some(refs) = refs {
                if refs.iter().any(|reference| reference == "standard") {
                    return Err(super::diagnostics::metadata_error(
                        &file,
                        "/config/signing_permission_set_refs",
                        "Sensor signing requires explicit named permission sets",
                    ));
                }
                plan.uses.push(PermissionUse {
                    source_file: file.clone(),
                    metadata_path: "/config/signing_permission_set_refs".into(),
                    refs: Some(refs),
                    action: None,
                    cache_only: false,
                    signing_only: true,
                    input_only: false,
                });
            }
        }
        for (name, content) in files(&pack_dir.join("sensors"))? {
            let file = format!("sensors/{name}");
            let data = parse(&content, &file)?;
            if let Some(refs) = literal_refs(
                data.get("config")
                    .and_then(|config| config.get("input_permission_set_refs")),
                false,
            )? {
                if refs.iter().any(|reference| reference == "standard") {
                    return Err(super::diagnostics::metadata_error(
                        &file,
                        "/config/input_permission_set_refs",
                        "Sensor input access requires explicit named permission sets",
                    ));
                }
                plan.uses.push(PermissionUse {
                    source_file: file.clone(),
                    metadata_path: "/config/input_permission_set_refs".into(),
                    refs: Some(refs),
                    action: None,
                    cache_only: false,
                    signing_only: false,
                    input_only: true,
                });
            }
        }
        Ok(plan)
    }

    fn read_tasks(
        &mut self,
        workflow: &Value,
        pack_ref: &str,
        file: &str,
        base: &str,
    ) -> Result<()> {
        let tasks: Vec<(String, &Value)> = match workflow.get("tasks") {
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
            _ => Vec::new(),
        };
        for (path, task) in tasks {
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
                source_file: file.into(),
                metadata_path: format!("{path}/permission_set_refs"),
                refs,
                action,
                cache_only: false,
                signing_only: false,
                input_only: false,
            });
            self.read_tasks(task, pack_ref, file, &path)?;
        }
        Ok(())
    }

    pub async fn validate(
        &self,
        conn: &mut PgConnection,
        authority: &DelegationAuthority,
    ) -> Result<()> {
        let context = crate::repositories::pack::PackRepository::deployment_context_with_roles(
            &mut *conn,
            authority.identity_id,
            &self.pack_ref,
        )
        .await?;
        let deployment_authorized = self.deployment_authorized(authority, context);
        self.validate_metadata(conn, authority, deployment_authorized)
            .await
    }

    pub(crate) async fn validate_deployment(
        &self,
        conn: &mut PgConnection,
        authority: &DelegationAuthority,
        pack_id: crate::models::Id,
        deployment: &crate::repositories::pack::AuthorizedPackDeployment,
    ) -> Result<()> {
        deployment
            .validate(conn, authority.identity_id, pack_id, &self.pack_ref)
            .await?;
        self.validate_metadata(conn, authority, true).await
    }

    async fn validate_metadata(
        &self,
        conn: &mut PgConnection,
        authority: &DelegationAuthority,
        deployment_authorized: bool,
    ) -> Result<()> {
        for (reference, grants) in &self.definitions {
            if !deployment_authorized
                && !authority.manages_permissions()
                && !authority.covers(grants)
            {
                let index = grants
                    .iter()
                    .position(|grant| !authority.covers(std::slice::from_ref(grant)))
                    .unwrap_or(0);
                return Err(super::diagnostics::located(
                    Error::PermissionDenied(
                        "Pack permission definitions exceed the registering identity's authority"
                            .into(),
                    ),
                    &self.definition_sources[reference],
                    &format!("/grants/{index}"),
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
            let refs = named_refs(refs.map(Vec::as_slice).unwrap_or(&[])).map_err(|error| {
                super::diagnostics::located(error, &usage.source_file, &usage.metadata_path)
            })?;
            references.extend(
                refs.iter()
                    .filter(|reference| !self.definitions.contains_key(*reference))
                    .cloned(),
            );
            resolved.push((
                refs,
                usage.cache_only,
                usage.signing_only,
                usage.input_only,
                usage,
            ));
        }
        let references: Vec<_> = references.into_iter().collect();
        let sets = PermissionSetRepository::find_by_refs_for_share(&mut *conn, &references).await?;
        for (refs, _, _, _, usage) in &resolved {
            if refs.iter().any(|reference| {
                references.contains(reference) && !sets.iter().any(|set| &set.r#ref == reference)
            }) {
                return Err(super::diagnostics::located(Error::PermissionDenied("Requested execution permission sets are unavailable to this deployment identity. Check the submitted refs and registering authority.".into()), &usage.source_file, &usage.metadata_path));
            }
        }
        let mut grants_by_ref = self.definitions.clone();
        for set in sets {
            grants_by_ref.insert(set.r#ref, serde_json::from_value::<Vec<Grant>>(set.grants)?);
        }
        for (refs, cache_only, signing_only, input_only, usage) in resolved {
            if signing_only
                && refs.iter().any(|reference| {
                    !grants_by_ref[reference].iter().any(|grant| {
                        grant.resource == Resource::Keys
                            && grant.actions.contains(&crate::rbac::Action::Use)
                    })
                })
            {
                return Err(Error::validation(
                    "Sensor signing permission sets must grant Key use",
                ));
            }
            let mut grants: Vec<Grant> = refs
                .iter()
                .flat_map(|reference| grants_by_ref[reference].iter())
                .filter(|grant| !cache_only || grant.resource == Resource::Caches)
                .filter(|grant| {
                    !signing_only
                        || (grant.resource == Resource::Keys
                            && grant.actions.contains(&crate::rbac::Action::Use))
                })
                .cloned()
                .collect();
            if input_only {
                grants.retain(|grant| {
                    matches!(grant.resource, Resource::Keys | Resource::Packs)
                        && grant.actions.iter().any(|action| {
                            matches!(
                                action,
                                crate::rbac::Action::Read | crate::rbac::Action::Decrypt
                            )
                        })
                });
                for grant in &mut grants {
                    grant.actions.retain(|action| {
                        matches!(
                            action,
                            crate::rbac::Action::Read | crate::rbac::Action::Decrypt
                        )
                    });
                }
            }
            if signing_only {
                for grant in &mut grants {
                    grant.actions = vec![crate::rbac::Action::Use];
                }
            }
            if !deployment_authorized {
                authority.require_grants(&grants).map_err(|error| {
                    super::diagnostics::located(error, &usage.source_file, &usage.metadata_path)
                })?;
            }
        }
        Ok(())
    }

    fn deployment_authorized(
        &self,
        authority: &DelegationAuthority,
        context: AuthorizationContext,
    ) -> bool {
        context.target_ref.as_deref() == Some(self.pack_ref.as_str())
            && context.pack_ref.as_deref() == Some(self.pack_ref.as_str())
            && authority.allows(Resource::Packs, Action::Install, context)
    }
}

fn files(directory: &Path) -> Result<Vec<(String, String)>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    read_yaml_files(directory)
}

fn parse(content: &str, file: &str) -> Result<Value> {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(content)
        .map_err(|error| super::diagnostics::yaml_error(file, &error))?;
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
            if scope.pack_owner.is_some() && !crate::rbac::supports_pack_owner(grant.resource) {
                return Err(Error::validation(
                    "Pack owner constraints require pack-managed component metadata or packs",
                ));
            }
            if scope.pack_state.is_some() && grant.resource != Resource::Packs {
                return Err(Error::validation(
                    "Pack state constraints only apply to packs grants",
                ));
            }
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

#[cfg(test)]
mod tests {
    use super::PackPermissionPlan;

    #[test]
    fn deployment_authority_is_bound_to_the_target_pack_not_personal_grant_coverage() {
        use crate::{
            delegation::DelegationAuthority,
            rbac::{Action, Grant, GrantConstraints, Resource},
        };
        let root = tempfile::tempdir().unwrap();
        let plan = PackPermissionPlan::read(root.path(), "team_tools").unwrap();
        let broader = [Grant {
            resource: Resource::Permissions,
            actions: vec![Action::Manage],
            constraints: None,
        }];
        for (resource, action, constraints, expected) in [
            (Resource::Packs, Action::Install, None, true),
            (
                Resource::Packs,
                Action::Install,
                Some(GrantConstraints {
                    refs: Some(vec!["team_tools".into()]),
                    ..Default::default()
                }),
                true,
            ),
            (
                Resource::Packs,
                Action::Install,
                Some(GrantConstraints {
                    pack_refs: Some(vec!["team_tools".into()]),
                    ..Default::default()
                }),
                true,
            ),
            (
                Resource::Packs,
                Action::Install,
                Some(GrantConstraints {
                    pack_refs: Some(vec!["another_pack".into()]),
                    ..Default::default()
                }),
                false,
            ),
            (Resource::Packs, Action::Read, None, false),
            (Resource::Packs, Action::Configure, None, false),
            (Resource::Actions, Action::Create, None, false),
        ] {
            let authority = DelegationAuthority::new(
                1,
                Default::default(),
                vec![Grant {
                    resource,
                    actions: vec![action],
                    constraints,
                }],
            );
            assert!(!authority.covers(&broader));
            let context = crate::repositories::pack::PackRepository::deployment_context_for_pack(
                authority.identity_id,
                &plan.pack_ref,
                None,
            );
            assert_eq!(plan.deployment_authorized(&authority, context), expected);
        }
    }

    #[test]
    fn sensor_signing_requires_named_permission_sets() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("sensors")).unwrap();
        let sensor = root.path().join("sensors/signer.yaml");
        std::fs::write(
            &sensor,
            "ref: example.signer\nconfig:\n  signing_permission_set_refs: [standard]\n",
        )
        .unwrap();
        assert!(PackPermissionPlan::read(root.path(), "example").is_err());
        std::fs::write(
            &sensor,
            "ref: example.signer\nconfig:\n  signing_permission_set_refs: [example.signer_use]\n",
        )
        .unwrap();
        let plan = PackPermissionPlan::read(root.path(), "example").unwrap();
        assert_eq!(plan.uses.len(), 1);
        assert!(plan.uses[0].signing_only);
        assert_eq!(plan.uses[0].refs.as_ref().unwrap(), &["example.signer_use"]);
    }
}
