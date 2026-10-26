//! Shared resource policy contracts. Resource values never appear in group keys.
use crate::{models::PolicyMethod, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceParticipant {
    pub action: String,
    pub resource_parameter: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceConcurrency {
    pub namespace: String,
    pub limit: i32,
    pub method: PolicyMethod,
    pub participants: Vec<ResourceParticipant>,
}

impl ResourceConcurrency {
    pub fn validate(&self) -> Result<()> {
        if self.namespace.trim().is_empty() || self.namespace.len() > 255 || self.limit < 1 {
            return Err(Error::validation(
                "Resource concurrency requires a namespace and positive limit",
            ));
        }
        if self.participants.is_empty() {
            return Err(Error::validation(
                "Resource concurrency requires participants",
            ));
        }
        let mut actions = HashSet::new();
        for participant in &self.participants {
            if crate::schema::RefValidator::validate_component_ref(&participant.action).is_err()
                || participant.action.trim() != participant.action
                || participant
                    .resource_parameter
                    .split('.')
                    .any(|part| part.trim().is_empty())
                || !actions.insert(&participant.action)
            {
                return Err(Error::validation("Resource participants require unique qualified action refs and nonempty dotted parameter paths"));
            }
        }
        Ok(())
    }
}

/// Uses the same object-only dotted paths as standard parameter concurrency.
pub fn parameter_value<'a>(config: Option<&'a Value>, path: &str) -> Option<&'a Value> {
    let mut value = config?;
    for part in path.split('.') {
        value = value.as_object()?.get(part)?;
    }
    Some(value)
}

/// JSON scalar serialization preserves type and string bytes. Objects, arrays,
/// null, empty strings and fractional numbers are not resource identifiers.
pub fn resource_digest(config: Option<&Value>, path: &str) -> Result<String> {
    let value = parameter_value(config, path)
        .ok_or_else(|| Error::validation("Missing resource identifier"))?;
    let valid = match value {
        Value::String(text) => !text.trim().is_empty(),
        Value::Number(number) => number.is_i64() || number.is_u64(),
        Value::Bool(_) => true,
        _ => false,
    };
    if !valid {
        return Err(Error::validation(
            "Resource identifier must be a nonempty string, integer, or boolean",
        ));
    }
    Ok(Sha256::digest(serde_json::to_vec(value)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn equivalent_paths_share_identity_without_disclosing_values() {
        let a = resource_digest(Some(&json!({"org_id":"secret-org"})), "org_id").unwrap();
        let b = resource_digest(
            Some(&json!({"target":{"organization_id":"secret-org"}})),
            "target.organization_id",
        )
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(!a.contains("secret-org"));
        assert_ne!(
            resource_digest(Some(&json!({"id":1})), "id").unwrap(),
            resource_digest(Some(&json!({"id":"1"})), "id").unwrap()
        );
    }

    #[test]
    fn invalid_identifiers_fail_closed() {
        for value in [
            Value::Null,
            json!([]),
            json!({}),
            json!(""),
            json!("  "),
            json!(1.5),
        ] {
            assert!(resource_digest(Some(&json!({"id":value})), "id").is_err());
        }
        assert!(resource_digest(None, "id").is_err());
        assert!(resource_digest(Some(&json!({})), "id").is_err());
    }
}
