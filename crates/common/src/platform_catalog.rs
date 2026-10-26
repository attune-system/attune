//! Built-in metadata. Namespace is independent of lifecycle ownership.

use serde_json::Value;

use crate::{Error, Result};

pub const COMPATIBILITY_EPOCH: i32 = 1;
pub const CATALOG_REVISION: i32 = 3;

/// Platform component namespaces are reserved even without an installed pack.
pub fn reserved_pack_namespace(reference: &str) -> bool {
    reference == "core"
}

/// The component tables managed by pack installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ManagedComponentKind {
    SchemaType,
    Runtime,
    PermissionSet,
    Trigger,
    Action,
    Sensor,
    Rule,
    Policy,
    WorkQueue,
    Workflow,
    Dashboard,
    Cache,
}

impl ManagedComponentKind {
    pub(crate) fn table(self) -> &'static str {
        match self {
            Self::SchemaType => "schema_type",
            Self::Runtime => "runtime",
            Self::PermissionSet => "permission_set",
            Self::Trigger => "trigger",
            Self::Action => "action",
            Self::Sensor => "sensor",
            Self::Rule => "rule",
            Self::Policy => "policy",
            Self::WorkQueue => "work_queue",
            Self::Workflow => "workflow_definition",
            Self::Dashboard => "dashboard",
            Self::Cache => "cache_namespace",
        }
    }
}

/// Bundled definitions are compiled into the application, never read from an
/// installed pack. Keep the legacy YAML snapshot until bundled-core removal.
pub(crate) fn definitions() -> Result<Vec<(ManagedComponentKind, Value)>> {
    use ManagedComponentKind::{PermissionSet, Runtime, Trigger};
    const SOURCES: &[(ManagedComponentKind, &str)] = &[
        (
            Runtime,
            include_str!("platform_catalog/runtimes/shell.yaml"),
        ),
        (
            Runtime,
            include_str!("platform_catalog/runtimes/python.yaml"),
        ),
        (
            Runtime,
            include_str!("platform_catalog/runtimes/nodejs.yaml"),
        ),
        (
            Runtime,
            include_str!("platform_catalog/runtimes/native.yaml"),
        ),
        (Runtime, include_str!("platform_catalog/runtimes/java.yaml")),
        (Runtime, include_str!("platform_catalog/runtimes/ruby.yaml")),
        (Runtime, include_str!("platform_catalog/runtimes/perl.yaml")),
        (Runtime, include_str!("platform_catalog/runtimes/go.yaml")),
        (Runtime, include_str!("platform_catalog/runtimes/r.yaml")),
        (
            PermissionSet,
            include_str!("platform_catalog/permission_sets/admin.yaml"),
        ),
        (
            PermissionSet,
            include_str!("platform_catalog/permission_sets/editor.yaml"),
        ),
        (
            PermissionSet,
            include_str!("platform_catalog/permission_sets/executor.yaml"),
        ),
        (
            PermissionSet,
            include_str!("platform_catalog/permission_sets/viewer.yaml"),
        ),
        (
            Trigger,
            include_str!("platform_catalog/triggers/alert.yaml"),
        ),
        (
            Trigger,
            include_str!("platform_catalog/triggers/queue_started.yaml"),
        ),
        (
            Trigger,
            include_str!("platform_catalog/triggers/queue_empty.yaml"),
        ),
    ];
    SOURCES
        .iter()
        .map(|(kind, yaml)| {
            serde_yaml_ng::from_str(yaml)
                .map(|value| (*kind, value))
                .map_err(|error| Error::internal(format!("Invalid built-in catalog: {error}")))
        })
        .collect()
}

pub(crate) fn is_legacy_definition(kind: ManagedComponentKind, definition: &Value) -> Result<bool> {
    Ok(definitions()?
        .iter()
        .any(|(candidate_kind, candidate)| *candidate_kind == kind && candidate == definition))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_includes_global_disclosure_and_event_publication() {
        use crate::rbac::{Action, AuthorizationContext, Grant, Resource};
        let definition = definitions()
            .unwrap()
            .into_iter()
            .find(|(_, definition)| definition["ref"] == "core.admin")
            .unwrap()
            .1;
        let grants: Vec<Grant> = serde_json::from_value(definition["grants"].clone()).unwrap();
        let context = AuthorizationContext::new(42);
        for (resource, action) in [
            (Resource::Packs, Action::Decrypt),
            (Resource::Queues, Action::Decrypt),
            (Resource::QueueItems, Action::Decrypt),
            (Resource::Rules, Action::Decrypt),
            (Resource::Triggers, Action::Decrypt),
            (Resource::Artifacts, Action::Decrypt),
            (Resource::Inquiries, Action::Decrypt),
            (Resource::Events, Action::Create),
        ] {
            assert!(
                grants
                    .iter()
                    .any(|grant| grant.allows(resource, action, &context)),
                "admin missing global {resource:?}:{action:?}"
            );
        }
    }

    #[test]
    fn admin_covers_exactly_every_supported_global_permission() {
        use crate::rbac::{Grant, Resource};
        let definition = definitions()
            .unwrap()
            .into_iter()
            .find(|(_, definition)| definition["ref"] == "core.admin")
            .unwrap()
            .1;
        let grants: Vec<Grant> = serde_json::from_value(definition["grants"].clone()).unwrap();
        assert_eq!(grants.len(), Resource::ALL.len());
        for resource in Resource::ALL {
            let matches: Vec<_> = grants
                .iter()
                .filter(|grant| grant.resource == *resource)
                .collect();
            assert_eq!(matches.len(), 1, "admin must define {resource:?} once");
            let grant = matches[0];
            assert!(grant.constraints.is_none(), "admin grants must be global");
            assert_eq!(grant.actions.len(), resource.supported_actions().len());
            for action in resource.supported_actions() {
                assert!(
                    grant.actions.contains(action),
                    "admin missing {resource:?}:{action:?}"
                );
            }
        }
    }

    #[test]
    fn editor_can_author_event_packs_without_administrative_grants() {
        use crate::rbac::{Action, Grant, Resource};
        let definition = definitions()
            .unwrap()
            .into_iter()
            .find(|(_, definition)| definition["ref"] == "core.editor")
            .unwrap()
            .1;
        let grants: Vec<Grant> = serde_json::from_value(definition["grants"].clone()).unwrap();
        for (resource, action) in [
            (Resource::Packs, Action::Read),
            (Resource::Actions, Action::Create),
            (Resource::Actions, Action::Update),
            (Resource::Actions, Action::Execute),
            (Resource::Rules, Action::Create),
            (Resource::Rules, Action::Update),
            (Resource::Triggers, Action::Read),
            (Resource::Triggers, Action::Create),
            (Resource::Triggers, Action::Update),
            (Resource::Events, Action::Read),
            (Resource::Events, Action::Create),
        ] {
            assert!(
                grants.iter().any(|grant| {
                    grant.resource == resource
                        && grant.actions.contains(&action)
                        && grant.constraints.is_none()
                }),
                "editor missing {resource:?}:{action:?}"
            );
        }
        assert!(grants.iter().all(|grant| {
            !matches!(grant.resource, Resource::Permissions | Resource::Identities)
                && !grant.actions.contains(&Action::Manage)
                && !(grant.resource == Resource::Events && grant.actions.contains(&Action::Decrypt))
        }));
        let fresh = crate::repositories::pack::PackRepository::deployment_context_for_pack(
            7, "new_pack", None,
        );
        assert!(grants
            .iter()
            .any(|grant| grant.allows(Resource::Packs, Action::Install, &fresh)));
        let mut existing = fresh.clone();
        existing.pack_state = None;
        existing.target_id = Some(8);
        existing.managing_pack_id = Some(8);
        for owner in [None, Some(7), Some(9)] {
            existing.owner_identity_id = owner;
            assert_eq!(
                grants.iter().any(|grant| grant.allows(
                    Resource::Packs,
                    Action::Install,
                    &existing
                )),
                owner == Some(7)
            );
            assert!(grants.iter().any(|grant| grant.allows(
                Resource::Packs,
                Action::Read,
                &existing
            )));
            assert_eq!(
                grants.iter().any(|grant| grant.allows(
                    Resource::Packs,
                    Action::Configure,
                    &existing
                )),
                owner == Some(7)
            );
        }
        let reserved =
            crate::repositories::pack::PackRepository::deployment_context_for_pack(7, "core", None);
        assert!(!grants.iter().any(|grant| grant.allows(
            Resource::Packs,
            Action::Create,
            &reserved
        )));
        assert!(!grants.iter().any(|grant| grant.allows(
            Resource::Packs,
            Action::Install,
            &reserved
        )));
    }

    #[test]
    fn catalog_matches_bundled_legacy_metadata() {
        let definitions = definitions().unwrap();
        assert_eq!(definitions.len(), 16);
        for (kind, definition) in definitions {
            let component_ref = definition["ref"].as_str().unwrap();
            assert!(reserved_pack_namespace(
                component_ref.split('.').next().unwrap()
            ));
            let directory = match kind {
                ManagedComponentKind::Runtime => "runtimes",
                ManagedComponentKind::PermissionSet => "permission_sets",
                ManagedComponentKind::Trigger => "triggers",
                _ => unreachable!(),
            };
            let name = component_ref.strip_prefix("core.").unwrap();
            let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../packs/core/{directory}/{name}.yaml"));
            let legacy: Value =
                serde_yaml_ng::from_str(&std::fs::read_to_string(source).unwrap()).unwrap();
            assert_eq!(definition, legacy, "{component_ref}");
            assert!(is_legacy_definition(kind, &legacy).unwrap());
        }
    }
}
