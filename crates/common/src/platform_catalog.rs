//! Built-in metadata. Namespace is independent of lifecycle ownership.

use serde_json::Value;

use crate::{Error, Result};

pub const COMPATIBILITY_EPOCH: i32 = 1;
pub const CATALOG_REVISION: i32 = 1;

/// The component tables managed by pack installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ManagedComponentKind {
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
    fn catalog_matches_bundled_legacy_metadata() {
        let definitions = definitions().unwrap();
        assert_eq!(definitions.len(), 16);
        for (kind, definition) in definitions {
            let component_ref = definition["ref"].as_str().unwrap();
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
