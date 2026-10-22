//! Role-based access control (RBAC) model and evaluator.
//!
//! Permission sets store `grants` as a JSON array of [`Grant`].
//! This module defines the canonical grant schema and matching logic.

use crate::models::{ArtifactVisibility, Id, OwnerType};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = RbacResource)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = RbacAction)]
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
    Use,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = RbacOwnerConstraint)]
pub enum OwnerConstraint {
    #[serde(rename = "self")]
    SelfOnly,
    Any,
    None,
}

/// A namespace claim requires authoritative absence, not an ownerless pack row.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = RbacPackStateConstraint)]
pub enum PackStateConstraint {
    Uninstalled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = RbacPackOwnerConstraint)]
pub enum PackOwnerConstraint {
    #[serde(rename = "self")]
    SelfOnly,
}

/// Human ownership of component metadata, not runtime lineage or secret ownership.
pub fn supports_pack_owner(resource: Resource) -> bool {
    matches!(
        resource,
        Resource::Packs
            | Resource::Actions
            | Resource::Policies
            | Resource::Queues
            | Resource::Rules
            | Resource::Triggers
            | Resource::Runtimes
            | Resource::Dashboards
    )
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = RbacExecutionScopeConstraint)]
pub enum ExecutionScopeConstraint {
    #[serde(rename = "self")]
    SelfOnly,
    Descendants,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = RbacGrantConstraints)]
pub struct GrantConstraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_owner: Option<PackOwnerConstraint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_state: Option<PackStateConstraint>,
    /// Server-normalized structural ownership. Authored selectors cannot set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managing_pack_id: Option<Id>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = RbacGrant)]
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
    /// Server-resolved, current role membership for the actual managing pack.
    /// Never populated from JWT roles or on execution/sensor target resolution.
    pub pack_owner_role_member: bool,
    /// Persisted primary owner of the structurally managing pack, never the
    /// target's runtime owner, an associated pack, or a ref-derived namespace.
    pub pack_owner_identity_id: Option<Id>,
    /// Set only after resolving namespace absence and platform reservations.
    pub pack_state: Option<PackStateConstraint>,
    /// Set only from repository-owned target resolution, never a ref prefix.
    pub managing_pack_id: Option<Id>,
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
            pack_owner_role_member: false,
            pack_owner_identity_id: None,
            pack_state: None,
            managing_pack_id: None,
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
        if constraints.pack_owner.is_some()
            && (!supports_pack_owner(self.resource)
                || ctx.managing_pack_id.is_none()
                || !(ctx.pack_owner_identity_id == Some(ctx.identity_id)
                    || ctx.pack_owner_role_member))
        {
            return false;
        }
        if let Some(state) = constraints.pack_state {
            if self.resource != Resource::Packs
                || ctx.pack_state != Some(state)
                || ctx
                    .target_ref
                    .as_deref()
                    .is_none_or(crate::platform_catalog::reserved_pack_namespace)
                || ctx.target_id.is_some()
                || ctx.managing_pack_id.is_some()
                || ctx.owner_identity_id.is_some()
            {
                return false;
            }
        }
        if let Some(pack_id) = constraints.managing_pack_id {
            if ctx.managing_pack_id != Some(pack_id)
                || !crate::runtime_authority::pack_owned_resource(self.resource)
                || matches!(
                    self.resource,
                    Resource::Keys | Resource::Artifacts | Resource::Caches
                ) && !matches!(
                    ctx.owner_type,
                    Some(OwnerType::Pack | OwnerType::Action | OwnerType::Sensor)
                )
            {
                return false;
            }
        }

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
                OwnerConstraint::SelfOnly => {
                    ctx.owner_identity_id == Some(ctx.identity_id)
                        || self.resource == Resource::Packs
                            && ctx.pack_owner_role_member
                            && ctx.target_id.is_some()
                            && ctx.target_id == ctx.managing_pack_id
                }
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

/// Prove that every requested resource/action is contained by a held grant.
/// Owner-relative constraints use the same identity on both sides.
pub fn can_delegate_grants(
    held: &[Grant],
    requested: &[Grant],
    context: &AuthorizationContext,
) -> bool {
    requested.iter().all(|requested| {
        requested.actions.iter().all(|action| {
            held.iter().any(|held| {
                held.resource == requested.resource
                    && held.actions.contains(action)
                    && delegation_constraints_cover(
                        held.constraints.as_ref(),
                        requested.constraints.as_ref(),
                        context,
                    )
            })
        })
    })
}

fn delegation_constraints_cover(
    held: Option<&GrantConstraints>,
    requested: Option<&GrantConstraints>,
    context: &AuthorizationContext,
) -> bool {
    let Some(held) = held else {
        return true;
    };
    let empty = GrantConstraints::default();
    let requested = requested.unwrap_or(&empty);
    let attributes_match = held.attributes.as_ref().is_none_or(|attributes| {
        attributes
            .iter()
            .all(|(name, value)| context.identity_attributes.get(name) == Some(value))
    });
    let owner = match held.owner {
        None | Some(OwnerConstraint::Any) => true,
        Some(owner) => requested.owner == Some(owner),
    };
    let execution_scope = match held.execution_scope {
        None | Some(ExecutionScopeConstraint::Any) => true,
        Some(ExecutionScopeConstraint::Descendants) => matches!(
            requested.execution_scope,
            Some(ExecutionScopeConstraint::SelfOnly | ExecutionScopeConstraint::Descendants)
        ),
        Some(ExecutionScopeConstraint::SelfOnly) => {
            requested.execution_scope == Some(ExecutionScopeConstraint::SelfOnly)
        }
    };
    attributes_match
        && held
            .pack_owner
            .is_none_or(|owner| requested.pack_owner == Some(owner))
        && held
            .pack_state
            .is_none_or(|state| requested.pack_state == Some(state))
        && held
            .managing_pack_id
            .is_none_or(|pack| requested.managing_pack_id == Some(pack))
        && owner
        && execution_scope
        && selected_scope_is_subset(held.pack_refs.as_deref(), requested.pack_refs.as_deref())
        && selected_scope_is_subset(held.refs.as_deref(), requested.refs.as_deref())
        && selected_scope_is_subset(held.ids.as_deref(), requested.ids.as_deref())
        && selected_scope_is_subset(
            held.owner_types.as_deref(),
            requested.owner_types.as_deref(),
        )
        && selected_scope_is_subset(held.owner_refs.as_deref(), requested.owner_refs.as_deref())
        && selected_scope_is_subset(held.visibility.as_deref(), requested.visibility.as_deref())
        && held
            .encrypted
            .is_none_or(|value| requested.encrypted == Some(value))
}

fn selected_scope_is_subset<T: PartialEq>(held: Option<&[T]>, requested: Option<&[T]>) -> bool {
    match held {
        None => true,
        Some(held) => requested.is_some_and(|requested| {
            !requested.is_empty() && requested.iter().all(|value| held.contains(value))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_role_proof_counts_only_for_pack_ownership_not_target_ownership() {
        let mut context = AuthorizationContext::new(7);
        context.managing_pack_id = Some(19);
        context.target_id = Some(19);
        context.owner_identity_id = Some(99);
        context.pack_owner_identity_id = Some(99);
        context.pack_owner_role_member = true;
        let pack = Grant {
            resource: Resource::Packs,
            actions: vec![Action::Configure],
            constraints: Some(GrantConstraints {
                owner: Some(OwnerConstraint::SelfOnly),
                ..Default::default()
            }),
        };
        assert!(pack.allows(Resource::Packs, Action::Configure, &context));
        let rule = Grant {
            resource: Resource::Rules,
            ..pack.clone()
        };
        assert!(!rule.allows(Resource::Rules, Action::Configure, &context));
        let managed = Grant {
            resource: Resource::Rules,
            constraints: Some(GrantConstraints {
                pack_owner: Some(PackOwnerConstraint::SelfOnly),
                ..Default::default()
            }),
            ..pack.clone()
        };
        assert!(managed.allows(Resource::Rules, Action::Configure, &context));
        context.managing_pack_id = None;
        assert!(!managed.allows(Resource::Rules, Action::Configure, &context));
        assert!(!pack.allows(Resource::Packs, Action::Configure, &context));
    }

    #[test]
    fn pack_owner_is_separate_from_target_owner_and_requires_structural_context() {
        let grant: Grant = serde_json::from_value(serde_json::json!({
            "resource": "rules", "actions": ["read", "update"], "constraints": {"pack_owner": "self"}
        })).unwrap();
        let mut context = AuthorizationContext::new(7);
        context.target_ref = Some("owned_namespace.rule".into());
        context.owner_identity_id = Some(7);
        assert!(!grant.allows(Resource::Rules, Action::Read, &context));
        context.managing_pack_id = Some(19);
        for owner in [None, Some(8)] {
            context.pack_owner_identity_id = owner;
            assert!(!grant.allows(Resource::Rules, Action::Read, &context));
        }
        context.pack_owner_identity_id = Some(7);
        context.owner_identity_id = Some(99);
        assert!(grant.allows(Resource::Rules, Action::Read, &context));
        let target_owned = Grant {
            constraints: Some(GrantConstraints {
                owner: Some(OwnerConstraint::SelfOnly),
                ..Default::default()
            }),
            ..grant.clone()
        };
        assert!(!target_owned.allows(Resource::Rules, Action::Read, &context));
        context.owner_identity_id = Some(7);
        context.pack_owner_identity_id = Some(8);
        assert!(target_owned.allows(Resource::Rules, Action::Read, &context));
        assert!(!grant.allows(Resource::Rules, Action::Read, &context));
        let global = Grant {
            constraints: None,
            ..grant.clone()
        };
        context.pack_owner_identity_id = None;
        assert!([grant.clone(), global].iter().any(|grant| grant.allows(
            Resource::Rules,
            Action::Read,
            &context
        )));
        let secret = Grant {
            resource: Resource::Keys,
            ..grant.clone()
        };
        context.pack_owner_identity_id = Some(7);
        assert!(!secret.allows(Resource::Keys, Action::Read, &context));
        assert!(crate::runtime_authority::validate_stable_grant(&grant).is_err());
        assert_eq!(
            serde_json::to_value(&grant).unwrap()["constraints"]["pack_owner"],
            "self"
        );
        for owner in ["any", "none", "primary"] {
            assert!(serde_json::from_value::<GrantConstraints>(
                serde_json::json!({"pack_owner": owner})
            )
            .is_err());
        }
    }

    #[test]
    fn delegation_preserves_pack_owner_even_when_the_current_pack_is_owned() {
        let owned = Grant {
            resource: Resource::Actions,
            actions: vec![Action::Update],
            constraints: Some(GrantConstraints {
                pack_owner: Some(PackOwnerConstraint::SelfOnly),
                ..Default::default()
            }),
        };
        let global = Grant {
            constraints: None,
            ..owned.clone()
        };
        let narrow = Grant {
            constraints: Some(GrantConstraints {
                refs: Some(vec!["tools.run".into()]),
                ..owned.constraints.clone().unwrap()
            }),
            ..owned.clone()
        };
        let mut context = AuthorizationContext::new(7);
        context.managing_pack_id = Some(19);
        context.pack_owner_identity_id = Some(7);
        assert!(!can_delegate_grants(
            std::slice::from_ref(&owned),
            std::slice::from_ref(&global),
            &context
        ));
        assert!(can_delegate_grants(
            std::slice::from_ref(&global),
            std::slice::from_ref(&owned),
            &context
        ));
        assert!(can_delegate_grants(
            std::slice::from_ref(&owned),
            std::slice::from_ref(&narrow),
            &context
        ));
        assert!(!can_delegate_grants(
            std::slice::from_ref(&narrow),
            std::slice::from_ref(&owned),
            &context
        ));
    }

    fn claim_grant() -> Grant {
        serde_json::from_value(serde_json::json!({
            "resource": "packs", "actions": ["install"],
            "constraints": {"pack_state": "uninstalled"}
        }))
        .unwrap()
    }

    #[test]
    fn uninstalled_requires_authoritative_absence_and_is_not_ownerless() {
        let grant = claim_grant();
        let mut context = AuthorizationContext::new(7);
        context.target_ref = Some("tools".into());
        assert!(!grant.allows(Resource::Packs, Action::Install, &context));
        context.pack_state = Some(PackStateConstraint::Uninstalled);
        assert!(grant.allows(Resource::Packs, Action::Install, &context));
        context.target_id = Some(8);
        assert!(!grant.allows(Resource::Packs, Action::Install, &context));
        context.target_id = None;
        context.owner_identity_id = Some(7);
        assert!(!grant.allows(Resource::Packs, Action::Install, &context));
        let mut invalid_resource = grant.clone();
        invalid_resource.resource = Resource::Actions;
        context.owner_identity_id = None;
        assert!(!invalid_resource.allows(Resource::Actions, Action::Install, &context));
        assert!(crate::runtime_authority::validate_stable_grant(&invalid_resource).is_err());
        context.target_ref = Some("core".into());
        assert!(!grant.allows(Resource::Packs, Action::Install, &context));
        assert_eq!(
            serde_json::to_value(grant).unwrap()["constraints"]["pack_state"],
            "uninstalled"
        );
        assert!(serde_json::from_value::<GrantConstraints>(
            serde_json::json!({"pack_state": "unclaimed"})
        )
        .is_err());
    }

    #[test]
    fn delegation_cannot_drop_pack_state_even_when_context_is_uninstalled() {
        let claim = claim_grant();
        let broad = Grant {
            constraints: None,
            ..claim.clone()
        };
        let narrow = Grant {
            constraints: Some(GrantConstraints {
                refs: Some(vec!["tools".into()]),
                ..claim.constraints.clone().unwrap()
            }),
            ..claim.clone()
        };
        let context = crate::repositories::pack::PackRepository::deployment_context_for_pack(
            7, "tools", None,
        );
        assert!(!can_delegate_grants(
            std::slice::from_ref(&claim),
            std::slice::from_ref(&broad),
            &context
        ));
        assert!(can_delegate_grants(
            std::slice::from_ref(&broad),
            std::slice::from_ref(&claim),
            &context
        ));
        assert!(can_delegate_grants(
            std::slice::from_ref(&claim),
            std::slice::from_ref(&narrow),
            &context
        ));
        assert!(!can_delegate_grants(
            std::slice::from_ref(&narrow),
            std::slice::from_ref(&claim),
            &context
        ));
    }
    use serde_json::json;

    #[test]
    fn structural_iam_containment_cannot_drop_or_change_managing_pack() {
        let held = Grant {
            resource: Resource::Actions,
            actions: vec![Action::Execute],
            constraints: Some(GrantConstraints {
                managing_pack_id: Some(17),
                ..Default::default()
            }),
        };
        let context = AuthorizationContext::new(42);
        let mut requested = held.clone();
        requested.constraints.as_mut().unwrap().ids = Some(vec![8]);
        assert!(can_delegate_grants(
            std::slice::from_ref(&held),
            &[requested.clone()],
            &context
        ));
        for pack in [None, Some(18)] {
            requested.constraints.as_mut().unwrap().managing_pack_id = pack;
            assert!(!can_delegate_grants(
                std::slice::from_ref(&held),
                &[requested.clone()],
                &context
            ));
        }
    }

    #[test]
    fn structural_context_does_not_enable_global_administration() {
        let mut context = AuthorizationContext::new(42);
        context.managing_pack_id = Some(17);
        for resource in [
            Resource::Workers,
            Resource::Identities,
            Resource::Permissions,
            Resource::Retention,
            Resource::AuditLog,
        ] {
            let grant = Grant {
                resource,
                actions: vec![Action::Manage],
                constraints: Some(GrantConstraints {
                    managing_pack_id: Some(17),
                    ..Default::default()
                }),
            };
            assert!(!grant.allows(resource, Action::Manage, &context));
        }
    }

    #[test]
    fn delegation_cannot_discard_requested_scope_constraints() {
        let held: Vec<Grant> = serde_json::from_value(
            json!([{ "resource":"keys", "actions":["read"], "constraints":{"owner":"none"}}]),
        )
        .unwrap();
        let broad: Vec<Grant> =
            serde_json::from_value(json!([{ "resource":"keys", "actions":["read"]}])).unwrap();
        let context = AuthorizationContext::new(42);
        assert!(held[0].allows(Resource::Keys, Action::Read, &context));
        assert!(!can_delegate_grants(&held, &broad, &context));
        assert!(can_delegate_grants(&held, &held, &context));
    }

    #[test]
    fn delegation_allows_narrowing_and_separate_action_grants() {
        let held: Vec<Grant> = serde_json::from_value(json!([
            {"resource":"actions", "actions":["read"], "constraints":{"pack_refs":["a","b"]}},
            {"resource":"actions", "actions":["execute"], "constraints":{"pack_refs":["a","b"]}}
        ]))
        .unwrap();
        let narrow: Vec<Grant> = serde_json::from_value(json!([{ "resource":"actions", "actions":["read","execute"], "constraints":{"pack_refs":["a"]}}])).unwrap();
        let broad: Vec<Grant> =
            serde_json::from_value(json!([{ "resource":"actions", "actions":["read","execute"]}]))
                .unwrap();
        assert!(can_delegate_grants(
            &held,
            &narrow,
            &AuthorizationContext::new(42)
        ));
        assert!(!can_delegate_grants(
            &held,
            &broad,
            &AuthorizationContext::new(42)
        ));
    }

    #[test]
    fn delegation_checks_current_identity_attributes() {
        let held: Vec<Grant> = serde_json::from_value(json!([{ "resource":"actions", "actions":["execute"], "constraints":{"attributes":{"department":"ops"}}}])).unwrap();
        let requested: Vec<Grant> =
            serde_json::from_value(json!([{ "resource":"actions", "actions":["execute"]}]))
                .unwrap();
        let mut context = AuthorizationContext::new(42);
        assert!(!can_delegate_grants(&held, &requested, &context));
        context
            .identity_attributes
            .insert("department".into(), json!("ops"));
        assert!(can_delegate_grants(&held, &requested, &context));
    }

    #[test]
    fn grant_deserialization_rejects_misspelled_security_fields() {
        for value in [
            json!({"resource": "actions", "actions": ["execute"], "constraint": {"refs": ["deploy.release"]}}),
            json!({"resource": "actions", "actions": ["execute"], "constraints": {"ref": ["deploy.release"]}}),
        ] {
            assert!(serde_json::from_value::<Grant>(value).is_err());
        }
        let grant: Grant = serde_json::from_value(json!({
            "resource": "actions", "actions": ["execute"],
            "constraints": {"refs": ["deploy.release"]}
        }))
        .unwrap();
        let mut context = AuthorizationContext::new(42);
        context.target_ref = Some("deploy.other".to_string());
        assert!(!grant.allows(Resource::Actions, Action::Execute, &context));
    }

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
