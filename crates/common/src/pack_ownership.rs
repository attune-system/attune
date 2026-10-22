//! Canonical ownership declarations from pack.yaml. Names match identity roles
//! exactly: no trimming, case folding, role assignment, or permission issuance.

use serde::{Deserialize, Deserializer, Serialize};

pub fn validate_owner_roles(roles: Vec<String>) -> crate::Result<Vec<String>> {
    if roles.len() > 64 {
        return Err(crate::Error::validation(
            "owner_roles must contain at most 64 role names",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for role in roles {
        if role.trim().is_empty()
            || role.chars().count() > 255
            || role.chars().any(char::is_control)
        {
            return Err(crate::Error::validation("owner_roles requires nonempty role names of at most 255 characters, without control characters"));
        }
        if seen.insert(role.clone()) {
            result.push(role);
        }
    }
    Ok(result)
}

/// Absence preserves the previous declaration. An explicit [] clears it.
/// Null, scalars and non-string list entries are rejected.
pub fn deserialize_owner_roles<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    let value = serde_json::Value::deserialize(deserializer)?;
    let roles = serde_json::from_value::<Vec<String>>(value).map_err(D::Error::custom)?;
    validate_owner_roles(roles)
        .map(Some)
        .map_err(D::Error::custom)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PackOwnershipMetadata {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_owner_roles"
    )]
    #[schema(required = false, nullable = false, max_items = 64)]
    pub owner_roles: Option<Vec<String>>,
}

impl PackOwnershipMetadata {
    pub fn read(path: &std::path::Path) -> crate::Result<Self> {
        let content = std::fs::read_to_string(path.join("pack.yaml"))
            .map_err(|error| crate::Error::io(error.to_string()))?;
        serde_yaml_ng::from_str(&content).map_err(|error| {
            crate::Error::validation(format!("Invalid pack ownership metadata: {error}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_roles_are_exact_bounded_deduplicated_and_presence_sensitive() {
        assert!(serde_yaml_ng::from_str::<PackOwnershipMetadata>("owner_roles: [123]").is_err());
        let parse = |value| serde_json::from_value::<PackOwnershipMetadata>(value);
        assert_eq!(parse(serde_json::json!({})).unwrap().owner_roles, None);
        assert_eq!(
            parse(serde_json::json!({"owner_roles": []}))
                .unwrap()
                .owner_roles,
            Some(vec![])
        );
        assert_eq!(
            parse(serde_json::json!({"owner_roles": ["Team", "team", "Team", " team "]}))
                .unwrap()
                .owner_roles,
            Some(vec!["Team".into(), "team".into(), " team ".into()])
        );
        for value in [
            serde_json::Value::Null,
            serde_json::json!("team"),
            serde_json::json!([1]),
            serde_json::json!([""]),
            serde_json::json!(["  "]),
            serde_json::json!(["bad\nrole"]),
            serde_json::json!(["x".repeat(256)]),
            serde_json::json!(vec!["team"; 65]),
        ] {
            assert!(parse(serde_json::json!({"owner_roles": value})).is_err());
        }
    }
}
