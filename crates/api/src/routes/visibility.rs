//! Shared row-level visibility helpers for scoped-identity read filtering.
//!
//! These helpers translate an identity's effective RBAC grants into a
//! [`VisibilityReadScope`] projection that repository search filters can turn
//! into SQL predicates (see the `push_visibility_scope_predicate` family in
//! `attune_common::repositories::event`). They are resource-agnostic so they
//! can be reused across route modules (events, enforcements, rules, ...).

use attune_common::rbac::{Action as RbacAction, Grant, GrantConstraints, Resource};
use attune_common::repositories::event::{VisibilityGrantFilter, VisibilityReadScope};

use crate::auth::{jwt::TokenType, middleware::AuthenticatedUser};

pub(crate) async fn component_read_filter(
    state: &std::sync::Arc<crate::state::AppState>,
    user: &AuthenticatedUser,
) -> Result<
    Option<attune_common::repositories::iam_ownership::ComponentReadFilter>,
    crate::middleware::ApiError,
> {
    Ok(state
        .authorization_service()
        .load_snapshot(user)
        .await?
        .map(
            |snapshot| attune_common::repositories::iam_ownership::ComponentReadFilter {
                identity: snapshot.identity_id,
                attributes: snapshot.identity_attributes,
                grants: snapshot.grants,
            },
        ))
}

#[derive(Default)]
pub(crate) struct VisibilityOwners {
    rules: std::collections::HashMap<(i64, String), Option<i64>>,
    triggers: std::collections::HashMap<(i64, String), (Option<i64>, bool)>,
    enforcement_triggers: std::collections::HashMap<(i64, String), (i64, Option<i64>)>,
}

impl VisibilityOwners {
    pub(crate) async fn load(
        db: &sqlx::PgPool,
        rule_scope: &VisibilityReadScope,
        trigger_scope: &VisibilityReadScope,
        rule_ids: &[i64],
        trigger_ids: &[i64],
        enforcement_ids: &[i64],
    ) -> Result<Self, crate::middleware::ApiError> {
        use attune_common::repositories::{
            iam_ownership::IamOwnershipRepository as Owners, runtime_target::ComponentTarget,
        };
        let mut result = Self::default();
        if rule_scope
            .grants
            .iter()
            .any(|grant| grant.managing_pack_id.is_some())
        {
            let ids = rule_ids
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            for batch in ids.chunks(64) {
                for (id, reference, pack) in
                    Owners::component_refs_and_packs(db, ComponentTarget::Rule, batch).await?
                {
                    result.rules.insert((id, reference), pack);
                }
            }
        }
        if !trigger_scope.unconstrained
            && (trigger_scope.include_public
                || trigger_scope
                    .grants
                    .iter()
                    .any(|grant| grant.managing_pack_id.is_some()))
        {
            let ids = trigger_ids
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            for batch in ids.chunks(64) {
                for (id, reference, pack, public) in
                    Owners::trigger_visibility_owners(db, batch).await?
                {
                    result.triggers.insert((id, reference), (pack, public));
                }
            }
            for batch in enforcement_ids.chunks(64).filter(|_| {
                trigger_scope
                    .grants
                    .iter()
                    .any(|grant| grant.managing_pack_id.is_some())
            }) {
                for (enforcement, id, reference, pack) in
                    Owners::enforcement_trigger_owners(db, batch).await?
                {
                    result
                        .enforcement_triggers
                        .insert((enforcement, reference), (id, pack));
                }
            }
        }
        Ok(result)
    }

    pub(crate) fn rule_pack(&self, id: Option<i64>, reference: Option<&str>) -> Option<i64> {
        self.rules.get(&(id?, reference?.into())).copied().flatten()
    }
    pub(crate) fn trigger_pack(&self, id: Option<i64>, reference: &str) -> Option<i64> {
        self.triggers
            .get(&(id?, reference.into()))
            .and_then(|(pack, _)| *pack)
    }
    pub(crate) fn trigger_public(&self, id: Option<i64>, reference: &str) -> bool {
        id.and_then(|id| self.triggers.get(&(id, reference.into())))
            .is_some_and(|(_, public)| *public)
    }
    pub(crate) fn enforcement_trigger(
        &self,
        enforcement: i64,
        reference: &str,
    ) -> (Option<i64>, Option<i64>) {
        self.enforcement_triggers
            .get(&(enforcement, reference.into()))
            .map(|(id, pack)| (Some(*id), *pack))
            .unwrap_or_default()
    }
}

/// Scoped-identity tokens whose reads must be filtered by RBAC grants. Other
/// token types (sensor, worker, ...) preserve their existing behavior.
pub(crate) fn is_scoped_identity_token(user: &AuthenticatedUser) -> bool {
    matches!(
        user.claims.token_type,
        TokenType::Access | TokenType::Execution
    )
}

/// A constraint set that imposes no scoping at all (equivalent to an
/// unconstrained grant).
pub(crate) fn constraints_are_effectively_unscoped(constraints: &GrantConstraints) -> bool {
    constraints.pack_owner.is_none()
        && constraints.pack_state.is_none()
        && constraints.managing_pack_id.is_none()
        && constraints.pack_refs.is_none()
        && constraints.owner.is_none()
        && constraints.owner_types.is_none()
        && constraints.owner_refs.is_none()
        && constraints.visibility.is_none()
        && constraints.execution_scope.is_none()
        && constraints.refs.is_none()
        && constraints.ids.is_none()
        && constraints.encrypted.is_none()
        && constraints.attributes.is_none()
}

/// Whether a constraint set only uses fields the visibility projection can
/// represent (id/ref/pack_ref allowlists). Grants that additionally constrain
/// on unsupported dimensions are ignored for projection purposes.
pub(crate) fn constraints_supported_for_visibility_projection(
    constraints: &GrantConstraints,
) -> bool {
    constraints.pack_owner.is_none()
        && constraints.pack_state.is_none()
        && constraints.owner.is_none()
        && constraints.owner_types.is_none()
        && constraints.owner_refs.is_none()
        && constraints.visibility.is_none()
        && constraints.execution_scope.is_none()
        && constraints.encrypted.is_none()
        && constraints.attributes.is_none()
}

/// Whether the caller holds any grant for `resource`/`action`.
pub(crate) fn resource_action_grant_exists(
    grants: &[Grant],
    resource: Resource,
    action: RbacAction,
) -> bool {
    grants
        .iter()
        .any(|grant| grant.resource == resource && grant.actions.contains(&action))
}

/// Whether the caller holds an unconstrained (global) grant for
/// `resource`/`action`. This is the admin-level override path.
pub(crate) fn has_unconstrained_resource_action(
    grants: &[Grant],
    resource: Resource,
    action: RbacAction,
) -> bool {
    grants.iter().any(|grant| {
        grant.resource == resource
            && grant.actions.contains(&action)
            && match grant.constraints.as_ref() {
                None => true,
                Some(constraints) => constraints_are_effectively_unscoped(constraints),
            }
    })
}

/// Build the visibility read scope for `resource`/`action` from the caller's
/// effective grants. An unconstrained grant yields `unconstrained = true`
/// (full access); scoped grants yield id/ref/pack_ref allowlists; no matching
/// grant yields an empty scope (deny).
pub(crate) fn build_visibility_read_scope(
    grants: &[Grant],
    resource: Resource,
    action: RbacAction,
    include_public: bool,
) -> VisibilityReadScope {
    let mut scope = VisibilityReadScope {
        include_public,
        ..Default::default()
    };
    for grant in grants {
        if grant.resource != resource || !grant.actions.contains(&action) {
            continue;
        }

        let Some(constraints) = grant.constraints.as_ref() else {
            scope.unconstrained = true;
            scope.grants.clear();
            break;
        };
        if constraints_are_effectively_unscoped(constraints) {
            scope.unconstrained = true;
            scope.grants.clear();
            break;
        }
        if !constraints_supported_for_visibility_projection(constraints) {
            continue;
        }
        if constraints.ids.as_ref().is_some_and(Vec::is_empty)
            || constraints.refs.as_ref().is_some_and(Vec::is_empty)
            || constraints.pack_refs.as_ref().is_some_and(Vec::is_empty)
        {
            continue;
        }

        let projection = VisibilityGrantFilter {
            managing_pack_id: constraints.managing_pack_id,
            ids: constraints.ids.clone().unwrap_or_default(),
            refs: constraints.refs.clone().unwrap_or_default(),
            pack_refs: constraints.pack_refs.clone().unwrap_or_default(),
        };
        if projection.managing_pack_id.is_none()
            && projection.ids.is_empty()
            && projection.refs.is_empty()
            && projection.pack_refs.is_empty()
        {
            continue;
        }
        scope.grants.push(projection);
    }

    scope
}

pub(crate) fn structural_visibility_scope(
    snapshot: &crate::authz::AuthorizationSnapshot,
    resource: Resource,
    action: RbacAction,
) -> VisibilityReadScope {
    // Attribute predicates are evaluated once against the request snapshot,
    // before translating the remaining target predicates into SQL.
    let grants = snapshot
        .grants
        .iter()
        .filter_map(|grant| {
            let constraints = grant.constraints.as_ref()?;
            constraints.managing_pack_id?;
            if constraints.attributes.as_ref().is_some_and(|attributes| {
                attributes
                    .iter()
                    .any(|(key, value)| snapshot.identity_attributes.get(key) != Some(value))
            }) {
                return None;
            }
            let mut grant = grant.clone();
            grant.constraints.as_mut()?.attributes = None;
            Some(grant)
        })
        .collect::<Vec<_>>();
    build_visibility_read_scope(&grants, resource, action, false)
}

pub(crate) fn snapshot_visibility_scope(
    snapshot: &crate::authz::AuthorizationSnapshot,
    resource: Resource,
    action: RbacAction,
) -> VisibilityReadScope {
    let grants = snapshot
        .grants
        .iter()
        .filter_map(|grant| {
            let mut grant = grant.clone();
            if let Some(scope) = grant.constraints.as_mut() {
                if scope.attributes.as_ref().is_some_and(|expected| {
                    expected
                        .iter()
                        .any(|(key, value)| snapshot.identity_attributes.get(key) != Some(value))
                }) {
                    return None;
                }
                scope.attributes = None;
                match scope.owner {
                    Some(attune_common::rbac::OwnerConstraint::SelfOnly) => return None,
                    _ => scope.owner = None,
                }
                match scope.execution_scope {
                    Some(
                        attune_common::rbac::ExecutionScopeConstraint::SelfOnly
                        | attune_common::rbac::ExecutionScopeConstraint::Descendants,
                    ) => return None,
                    _ => scope.execution_scope = None,
                }
            }
            Some(grant)
        })
        .collect::<Vec<_>>();
    build_visibility_read_scope(&grants, resource, action, false)
}

pub(crate) fn pack_ref_from_ref(value: &str) -> Option<&str> {
    value.split_once('.').map(|(pack_ref, _)| pack_ref)
}

/// Evaluate an in-memory row against a visibility read scope. Used for
/// single-row redaction/authorization after a row is already loaded.
#[cfg(test)]
fn scope_allows_resource_ref(
    scope: &VisibilityReadScope,
    id: Option<i64>,
    resource_ref: Option<&str>,
) -> bool {
    scope_allows_owned_resource_ref(scope, id, resource_ref, None)
}

pub(crate) fn scope_allows_owned_resource_ref(
    scope: &VisibilityReadScope,
    id: Option<i64>,
    resource_ref: Option<&str>,
    managing_pack_id: Option<i64>,
) -> bool {
    if scope.unconstrained {
        return true;
    }
    if scope.grants.is_empty() {
        return false;
    }

    let pack_ref = resource_ref.and_then(pack_ref_from_ref);
    scope.grants.iter().any(|grant| {
        // This legacy ref-only projection has no structural ownership proof.
        if grant
            .managing_pack_id
            .is_some_and(|pack| managing_pack_id != Some(pack))
        {
            return false;
        }
        if !grant.ids.is_empty() && !id.is_some_and(|value| grant.ids.contains(&value)) {
            return false;
        }
        if !grant.refs.is_empty()
            && !resource_ref
                .is_some_and(|value| grant.refs.iter().any(|candidate| candidate == value))
        {
            return false;
        }
        if !grant.pack_refs.is_empty()
            && !pack_ref.is_some_and(|value| {
                grant
                    .pack_refs
                    .iter()
                    .any(|candidate| candidate.as_str() == value)
            })
        {
            return false;
        }
        true
    })
}

pub(crate) fn action_name(action: RbacAction) -> &'static str {
    match action {
        RbacAction::Read => "read",
        RbacAction::Create => "create",
        RbacAction::Install => "install",
        RbacAction::Configure => "configure",
        RbacAction::Update => "update",
        RbacAction::Delete => "delete",
        RbacAction::Execute => "execute",
        RbacAction::Cancel => "cancel",
        RbacAction::Respond => "respond",
        RbacAction::Manage => "manage",
        RbacAction::Decrypt => "decrypt",
        RbacAction::Use => "use",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn global_redaction_budget_does_not_acquire_an_ownership_connection() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(1))
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let ids = (1..=1000).collect::<Vec<_>>();
        let scope = VisibilityReadScope {
            unconstrained: true,
            ..Default::default()
        };
        VisibilityOwners::load(&pool, &scope, &scope, &ids, &ids, &ids)
            .await
            .unwrap();
        pool.close().await;
    }

    #[test]
    fn redaction_proof_requires_exact_actual_component_identity_and_ref() {
        let scope = VisibilityReadScope {
            grants: vec![VisibilityGrantFilter {
                managing_pack_id: Some(17),
                ..Default::default()
            }],
            ..Default::default()
        };
        let owners = VisibilityOwners {
            rules: std::collections::HashMap::from([
                ((8, "local.rule".into()), Some(17)),
                ((9, "local.foreign".into()), Some(18)),
            ]),
            ..Default::default()
        };
        assert!(scope_allows_owned_resource_ref(
            &scope,
            Some(8),
            Some("local.rule"),
            owners.rule_pack(Some(8), Some("local.rule"))
        ));
        for (id, reference) in [(8, "reused.rule"), (9, "local.foreign"), (10, "local.rule")] {
            assert!(!scope_allows_owned_resource_ref(
                &scope,
                Some(id),
                Some(reference),
                owners.rule_pack(Some(id), Some(reference))
            ));
        }
    }

    #[test]
    fn public_trigger_fallback_does_not_treat_missing_private_or_reused_refs_as_public() {
        let owners = VisibilityOwners {
            triggers: std::collections::HashMap::from([
                ((8, "local.public".into()), (Some(17), true)),
                ((9, "local.private".into()), (Some(17), false)),
            ]),
            ..Default::default()
        };
        assert!(owners.trigger_public(Some(8), "local.public"));
        assert!(!owners.trigger_public(Some(9), "local.private"));
        assert!(!owners.trigger_public(Some(10), "local.public"));
        assert!(!owners.trigger_public(None, "local.public"));
    }

    #[test]
    fn structural_local_scope_is_not_an_unrestricted_or_ref_only_permission() {
        let grants: Vec<Grant> = serde_json::from_value(json!([{
            "resource": "rules", "actions": ["read"],
            "constraints": {"managing_pack_id": 17}
        }]))
        .unwrap();
        assert!(!has_unconstrained_resource_action(
            &grants,
            Resource::Rules,
            RbacAction::Read
        ));
        let scope = build_visibility_read_scope(&grants, Resource::Rules, RbacAction::Read, false);
        assert!(!scope.unconstrained);
        assert_eq!(scope.grants.len(), 1);
        assert_eq!(scope.grants[0].managing_pack_id, Some(17));
        assert!(!scope_allows_resource_ref(
            &scope,
            Some(8),
            Some("local.rule")
        ));
    }

    #[test]
    fn local_projection_preserves_narrower_selectors() {
        let grants: Vec<Grant> = serde_json::from_value(json!([{
            "resource": "rules", "actions": ["read"],
            "constraints": {"managing_pack_id": 17, "refs": ["local.one"], "ids": [8]}
        }]))
        .unwrap();
        let scope = build_visibility_read_scope(&grants, Resource::Rules, RbacAction::Read, false);
        assert_eq!(scope.grants[0].refs, vec!["local.one"]);
        assert_eq!(scope.grants[0].ids, vec![8]);
    }
}
