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
    constraints.pack_refs.is_none()
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
    constraints.owner.is_none()
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

        let projection = VisibilityGrantFilter {
            ids: constraints.ids.clone().unwrap_or_default(),
            refs: constraints.refs.clone().unwrap_or_default(),
            pack_refs: constraints.pack_refs.clone().unwrap_or_default(),
        };
        if projection.ids.is_empty()
            && projection.refs.is_empty()
            && projection.pack_refs.is_empty()
        {
            continue;
        }
        scope.grants.push(projection);
    }

    scope
}

pub(crate) fn pack_ref_from_ref(value: &str) -> Option<&str> {
    value.split_once('.').map(|(pack_ref, _)| pack_ref)
}

/// Evaluate an in-memory row against a visibility read scope. Used for
/// single-row redaction/authorization after a row is already loaded.
pub(crate) fn scope_allows_resource_ref(
    scope: &VisibilityReadScope,
    id: Option<i64>,
    resource_ref: Option<&str>,
) -> bool {
    if scope.unconstrained {
        return true;
    }
    if scope.grants.is_empty() {
        return false;
    }

    let pack_ref = resource_ref.and_then(pack_ref_from_ref);
    scope.grants.iter().any(|grant| {
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
    }
}
