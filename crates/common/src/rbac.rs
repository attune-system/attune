//! Role-based access control (RBAC) model and evaluator.
//!
//! Permission sets store `grants` as a JSON array of [`Grant`].
//! This module defines the canonical grant schema and matching logic.

use crate::models::{ArtifactVisibility, Id, OwnerType};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Packs,
    Actions,
    Policies,
    Queues,
    QueueItems,
    Rules,
    Triggers,
    Executions,
    Events,
    Enforcements,
    Inquiries,
    Keys,
    Caches,
    Artifacts,
    Runtimes,
    Workers,
    Dashboards,
    Retention,
    Identities,
    Permissions,
    AuditLog,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Read,
    Create,
    Install,
    Configure,
    Update,
    Delete,
    Execute,
    Cancel,
    Respond,
    Manage,
    Decrypt,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnerConstraint {
    #[serde(rename = "self")]
    SelfOnly,
    Any,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionScopeConstraint {
    #[serde(rename = "self")]
    SelfOnly,
    Descendants,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GrantConstraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_refs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<OwnerConstraint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_types: Option<Vec<OwnerType>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_refs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Vec<ArtifactVisibility>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_scope: Option<ExecutionScopeConstraint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<HashMap<String, JsonValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Grant {
    pub resource: Resource,
    pub actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<GrantConstraints>,
}

pub fn validate_cache_grant_constraints(
    constraints: &GrantConstraints,
) -> std::result::Result<(), String> {
    let owner_types = constraints.owner_types.as_deref();
    if owner_types.is_some_and(|values| values.is_empty()) {
        return Err("Cache owner_types cannot be empty".to_string());
    }

    if let Some(owner_refs) = constraints.owner_refs.as_deref() {
        if owner_refs.is_empty() || owner_refs.iter().any(|value| value.trim().is_empty()) {
            return Err("Cache owner_refs must contain non-empty references".to_string());
        }
        let owner_types = owner_types.ok_or_else(|| {
            "Cache owner_refs require exactly one matching owner_type".to_string()
        })?;
        if owner_types.len() != 1
            || !matches!(
                owner_types[0],
                OwnerType::Pack | OwnerType::Action | OwnerType::Sensor
            )
        {
            return Err(
                "Cache owner_refs require exactly one pack, action, or sensor owner_type"
                    .to_string(),
            );
        }
    }

    if let Some(namespace_refs) = constraints.refs.as_deref() {
        if namespace_refs.is_empty()
            || namespace_refs
                .iter()
                .any(|value| !is_valid_cache_namespace_ref(value))
        {
            return Err("Cache namespace refs must match ^[a-z0-9][a-z0-9._-]{0,127}$".to_string());
        }

        let owner_types = owner_types
            .ok_or_else(|| "Cache namespace refs require exactly one owner_type".to_string())?;
        if owner_types.len() != 1 {
            return Err("Cache namespace refs require exactly one owner_type".to_string());
        }
        if matches!(
            owner_types[0],
            OwnerType::Pack | OwnerType::Action | OwnerType::Sensor
        ) && constraints.owner_refs.is_none()
        {
            return Err(
                "Cache namespace refs for pack, action, or sensor owners require owner_refs"
                    .to_string(),
            );
        }
    }

    Ok(())
}

fn is_valid_cache_namespace_ref(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(byte) if byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

#[derive(Debug, Clone)]
pub struct AuthorizationContext {
    pub identity_id: Id,
    pub identity_attributes: HashMap<String, JsonValue>,
    pub target_id: Option<Id>,
    pub target_ref: Option<String>,
    pub pack_ref: Option<String>,
    pub owner_identity_id: Option<Id>,
    pub owner_type: Option<OwnerType>,
    pub owner_ref: Option<String>,
    pub visibility: Option<ArtifactVisibility>,
    pub encrypted: Option<bool>,
    pub execution_owner_identity_id: Option<Id>,
    pub execution_ancestor_identity_ids: Vec<Id>,
}

impl AuthorizationContext {
    pub fn new(identity_id: Id) -> Self {
        Self {
            identity_id,
            identity_attributes: HashMap::new(),
            target_id: None,
            target_ref: None,
            pack_ref: None,
            owner_identity_id: None,
            owner_type: None,
            owner_ref: None,
            visibility: None,
            encrypted: None,
            execution_owner_identity_id: None,
            execution_ancestor_identity_ids: Vec::new(),
        }
    }
}

impl Grant {
    pub fn allows(&self, resource: Resource, action: Action, ctx: &AuthorizationContext) -> bool {
        self.resource == resource && self.actions.contains(&action) && self.constraints_match(ctx)
    }

    fn constraints_match(&self, ctx: &AuthorizationContext) -> bool {
        let Some(constraints) = &self.constraints else {
            return true;
        };

        if let Some(pack_refs) = &constraints.pack_refs {
            let Some(pack_ref) = &ctx.pack_ref else {
                return false;
            };
            if !pack_refs.contains(pack_ref) {
                return false;
            }
        }

        if let Some(owner) = constraints.owner {
            let owner_match = match owner {
                OwnerConstraint::SelfOnly => ctx.owner_identity_id == Some(ctx.identity_id),
                OwnerConstraint::Any => true,
                OwnerConstraint::None => ctx.owner_identity_id.is_none(),
            };
            if !owner_match {
                return false;
            }
        }

        if let Some(owner_types) = &constraints.owner_types {
            let Some(owner_type) = ctx.owner_type else {
                return false;
            };
            if !owner_types.contains(&owner_type) {
                return false;
            }
        }

        if let Some(owner_refs) = &constraints.owner_refs {
            let Some(owner_ref) = &ctx.owner_ref else {
                return false;
            };
            if !owner_refs.contains(owner_ref) {
                return false;
            }
        }

        if let Some(visibility) = &constraints.visibility {
            let Some(target_visibility) = ctx.visibility else {
                return false;
            };
            if !visibility.contains(&target_visibility) {
                return false;
            }
        }

        if let Some(execution_scope) = constraints.execution_scope {
            let execution_match = match execution_scope {
                ExecutionScopeConstraint::SelfOnly => {
                    ctx.execution_owner_identity_id == Some(ctx.identity_id)
                }
                ExecutionScopeConstraint::Descendants => {
                    ctx.execution_owner_identity_id == Some(ctx.identity_id)
                        || ctx
                            .execution_ancestor_identity_ids
                            .contains(&ctx.identity_id)
                }
                ExecutionScopeConstraint::Any => true,
            };
            if !execution_match {
                return false;
            }
        }

        if let Some(refs) = &constraints.refs {
            let Some(target_ref) = &ctx.target_ref else {
                return false;
            };
            if !refs.contains(target_ref) {
                return false;
            }
        }

        if let Some(ids) = &constraints.ids {
            let Some(target_id) = ctx.target_id else {
                return false;
            };
            if !ids.contains(&target_id) {
                return false;
            }
        }

        if let Some(encrypted) = constraints.encrypted {
            let Some(target_encrypted) = ctx.encrypted else {
                return false;
            };
            if encrypted != target_encrypted {
                return false;
            }
        }

        if let Some(attributes) = &constraints.attributes {
            for (key, expected_value) in attributes {
                let Some(actual_value) = ctx.identity_attributes.get(key) else {
                    return false;
                };
                if actual_value != expected_value {
                    return false;
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grant_without_constraints_allows() {
        let grant = Grant {
            resource: Resource::Actions,
            actions: vec![Action::Read],
            constraints: None,
        };
        let ctx = AuthorizationContext::new(42);
        assert!(grant.allows(Resource::Actions, Action::Read, &ctx));
        assert!(!grant.allows(Resource::Actions, Action::Create, &ctx));
    }

    #[test]
    fn key_constraint_owner_type_and_encrypted() {
        let grant = Grant {
            resource: Resource::Keys,
            actions: vec![Action::Read],
            constraints: Some(GrantConstraints {
                owner_types: Some(vec![OwnerType::System]),
                encrypted: Some(false),
                ..Default::default()
            }),
        };

        let mut ctx = AuthorizationContext::new(1);
        ctx.owner_type = Some(OwnerType::System);
        ctx.encrypted = Some(false);
        assert!(grant.allows(Resource::Keys, Action::Read, &ctx));

        ctx.encrypted = Some(true);
        assert!(!grant.allows(Resource::Keys, Action::Read, &ctx));
    }

    #[test]
    fn attributes_constraint_requires_exact_value_match() {
        let grant = Grant {
            resource: Resource::Packs,
            actions: vec![Action::Read],
            constraints: Some(GrantConstraints {
                attributes: Some(HashMap::from([("team".to_string(), json!("platform"))])),
                ..Default::default()
            }),
        };

        let mut ctx = AuthorizationContext::new(1);
        ctx.identity_attributes
            .insert("team".to_string(), json!("platform"));
        assert!(grant.allows(Resource::Packs, Action::Read, &ctx));

        ctx.identity_attributes
            .insert("team".to_string(), json!("infra"));
        assert!(!grant.allows(Resource::Packs, Action::Read, &ctx));
    }

    #[test]
    fn cache_grant_matches_owner_type_owner_ref_and_namespace_ref() {
        // Cache grants scope reads/writes by owner type + owner ref, and a
        // specific namespace via the `refs` constraint (the namespace name is
        // the authorization target ref). Owner-only grants (no `refs`) cover
        // all namespaces in that owner.
        let grant = Grant {
            resource: Resource::Caches,
            actions: vec![Action::Read],
            constraints: Some(GrantConstraints {
                owner_types: Some(vec![OwnerType::Pack]),
                owner_refs: Some(vec!["salesforce".to_string()]),
                refs: Some(vec!["users".to_string()]),
                ..Default::default()
            }),
        };

        let mut ctx = AuthorizationContext::new(1);
        ctx.owner_type = Some(OwnerType::Pack);
        ctx.owner_ref = Some("salesforce".to_string());
        ctx.target_ref = Some("users".to_string());
        assert!(grant.allows(Resource::Caches, Action::Read, &ctx));

        // A different namespace in the same owner is not covered by a
        // namespace-scoped grant.
        ctx.target_ref = Some("locations".to_string());
        assert!(!grant.allows(Resource::Caches, Action::Read, &ctx));

        // A different owner ref is never covered.
        ctx.target_ref = Some("users".to_string());
        ctx.owner_ref = Some("other_pack".to_string());
        assert!(!grant.allows(Resource::Caches, Action::Read, &ctx));

        // Writes require a write action; a read grant never authorizes update.
        ctx.owner_ref = Some("salesforce".to_string());
        assert!(!grant.allows(Resource::Caches, Action::Update, &ctx));
    }

    #[test]
    fn owner_ref_constraint_requires_exact_value_match() {
        let grant = Grant {
            resource: Resource::Artifacts,
            actions: vec![Action::Read],
            constraints: Some(GrantConstraints {
                owner_types: Some(vec![OwnerType::Pack]),
                owner_refs: Some(vec!["python_example".to_string()]),
                ..Default::default()
            }),
        };

        let mut ctx = AuthorizationContext::new(1);
        ctx.owner_type = Some(OwnerType::Pack);
        ctx.owner_ref = Some("python_example".to_string());
        assert!(grant.allows(Resource::Artifacts, Action::Read, &ctx));

        ctx.owner_ref = Some("other_pack".to_string());
        assert!(!grant.allows(Resource::Artifacts, Action::Read, &ctx));

        ctx.owner_ref = None;
        assert!(!grant.allows(Resource::Artifacts, Action::Read, &ctx));
    }
}
