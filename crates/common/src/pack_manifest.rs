//! Shared normalization for canonical and legacy `pack.yaml` fields.

use serde_yaml_ng::{Mapping, Value};
use std::fmt;

const FIELD_ALIASES: &[(&str, &str)] = &[
    ("label", "name"),
    ("conf_schema", "config_schema"),
    ("meta", "metadata"),
    ("tags", "keywords"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackManifestConflict {
    pub canonical: &'static str,
    pub legacy: &'static str,
}

impl fmt::Display for PackManifestConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "canonical field '{}' and legacy field '{}' have different values; remove one or make them equal",
            self.canonical, self.legacy
        )
    }
}

#[derive(Debug, Clone)]
pub struct NormalizedPackManifest {
    pub mapping: Mapping,
    pub conflicts: Vec<PackManifestConflict>,
}

/// Normalize legacy aliases into canonical fields without overriding canonical values.
pub fn normalize_pack_manifest(mapping: &Mapping) -> NormalizedPackManifest {
    let mut normalized = mapping.clone();
    let mut conflicts = Vec::new();

    for &(canonical, legacy) in FIELD_ALIASES {
        let canonical_key = Value::String(canonical.to_string());
        let legacy_key = Value::String(legacy.to_string());
        match (mapping.get(&canonical_key), mapping.get(&legacy_key)) {
            (Some(canonical_value), Some(legacy_value)) if canonical_value != legacy_value => {
                conflicts.push(PackManifestConflict { canonical, legacy });
            }
            (None, Some(legacy_value)) => {
                normalized.insert(canonical_key, legacy_value.clone());
            }
            _ => {}
        }
    }

    NormalizedPackManifest {
        mapping: normalized,
        conflicts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_only_is_accepted() {
        let value: Value = serde_yaml_ng::from_str("label: Canonical\n").unwrap();
        let normalized = normalize_pack_manifest(value.as_mapping().unwrap());

        assert_eq!(
            normalized.mapping.get("label").and_then(Value::as_str),
            Some("Canonical")
        );
        assert!(normalized.conflicts.is_empty());
    }

    #[test]
    fn legacy_only_is_normalized_and_accepted() {
        let value: Value = serde_yaml_ng::from_str(
            "name: Legacy\nconfig_schema:\n  token:\n    type: string\nkeywords: [one]\n",
        )
        .unwrap();
        let normalized = normalize_pack_manifest(value.as_mapping().unwrap());

        assert_eq!(
            normalized.mapping.get("label").and_then(Value::as_str),
            Some("Legacy")
        );
        assert!(normalized.mapping.get("conf_schema").is_some());
        assert!(normalized.mapping.get("tags").is_some());
        assert!(normalized.conflicts.is_empty());
    }

    #[test]
    fn conflicting_mirrors_are_reported() {
        let value: Value = serde_yaml_ng::from_str("label: Canonical\nname: Legacy\n").unwrap();
        let normalized = normalize_pack_manifest(value.as_mapping().unwrap());

        assert_eq!(
            normalized.conflicts,
            vec![PackManifestConflict {
                canonical: "label",
                legacy: "name",
            }]
        );
    }

    #[test]
    fn equal_mirrors_are_accepted() {
        let value: Value = serde_yaml_ng::from_str("tags: [one]\nkeywords: [one]\n").unwrap();
        let normalized = normalize_pack_manifest(value.as_mapping().unwrap());

        assert!(normalized.conflicts.is_empty());
    }
}
