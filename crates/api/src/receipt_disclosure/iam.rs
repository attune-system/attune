//! Bind the current Access snapshot to repository-held IAM rows, not a cached
//! permit carried across later source/target waits.
use crate::middleware::{ApiError, ApiResult};
use attune_common::{
    models::Id,
    rbac::{AuthorizationContext, Grant},
    repositories::identity::{IdentityRepository, PermissionSetRepository},
};
use std::collections::HashMap;

pub(super) struct HeldIam {
    identity: Id,
    attributes: HashMap<String, serde_json::Value>,
    grants: Vec<Grant>,
}

impl HeldIam {
    pub(super) async fn retain(conn: &mut sqlx::PgConnection, identity: Id) -> ApiResult<Self> {
        let current = IdentityRepository::find_by_id_for_share(&mut *conn, identity)
            .await?
            .filter(|identity| !identity.frozen)
            .ok_or_else(|| ApiError::Unauthorized("Identity unavailable".into()))?;
        let mut sets =
            PermissionSetRepository::find_by_identity_for_share(&mut *conn, identity).await?;
        sets.extend(
            PermissionSetRepository::find_by_identity_roles_for_share(&mut *conn, identity).await?,
        );
        let grants = attune_common::delegation::grants_from_sets(sets)?;
        Ok(Self {
            identity,
            attributes: match current.attributes {
                serde_json::Value::Object(values) => values.into_iter().collect(),
                _ => HashMap::new(),
            },
            grants,
        })
    }
    pub(super) fn matches(&self, caller: &AuthorizationContext, grants: &[Grant]) -> bool {
        // Equality of the original grant values, ignoring only set order and
        // duplicates. Never combine actions, constraints or concrete targets.
        self.identity == caller.identity_id
            && self.attributes == caller.identity_attributes
            && self.grants.iter().all(|grant| grants.contains(grant))
            && grants.iter().all(|grant| self.grants.contains(grant))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grant(actions: serde_json::Value, id: Id) -> Grant {
        serde_json::from_value(
            serde_json::json!({"resource":"keys","actions":actions,"constraints":{"ids":[id]}}),
        )
        .unwrap()
    }
    #[test]
    fn held_current_iam_ignores_only_grant_order_and_identical_duplicates() {
        let read = grant(serde_json::json!(["read"]), 11);
        let decrypt = grant(serde_json::json!(["decrypt"]), 11);
        let held = HeldIam {
            identity: 1,
            attributes: HashMap::new(),
            grants: vec![read.clone(), decrypt.clone()],
        };
        assert!(held.matches(
            &AuthorizationContext::new(1),
            &[decrypt, read.clone(), read]
        ));
    }
    #[test]
    fn held_current_iam_does_not_union_separate_actions_or_targets() {
        let held = HeldIam {
            identity: 1,
            attributes: HashMap::new(),
            grants: vec![grant(serde_json::json!(["read", "decrypt"]), 11)],
        };
        assert!(!held.matches(
            &AuthorizationContext::new(1),
            &[
                grant(serde_json::json!(["read"]), 11),
                grant(serde_json::json!(["decrypt"]), 11)
            ]
        ));
        assert!(!held.matches(
            &AuthorizationContext::new(1),
            &[grant(serde_json::json!(["read", "decrypt"]), 12)]
        ));
    }
    #[test]
    fn held_current_iam_never_rebinds_another_identity_or_attributes() {
        let held = HeldIam {
            identity: 1,
            attributes: HashMap::new(),
            grants: vec![],
        };
        assert!(!held.matches(&AuthorizationContext::new(2), &[]));
        let mut changed = AuthorizationContext::new(1);
        changed
            .identity_attributes
            .insert("role".into(), serde_json::json!("changed"));
        assert!(!held.matches(&changed, &[]));
    }
}
