//! RBAC authorization service for API handlers.
//!
//! This module evaluates grants assigned to user identities via
//! `permission_set` and `permission_assignment`.

use crate::{
    auth::{jwt::TokenType, middleware::AuthenticatedUser},
    middleware::ApiError,
};
use attune_common::{
    audit::{
        event_type, AuditCategory, AuditEventBuilder, AuditOutcome, AuditRepository,
        PendingAuditEvent,
    },
    auth::jwt::STANDARD_EXECUTION_ACCESS_REF,
    models::OwnerType,
    rbac::{Action, AuthorizationContext, Grant, GrantConstraints, Resource},
    repositories::{
        identity::{IdentityRepository, IdentityRoleAssignmentRepository, PermissionSetRepository},
        FindById,
    },
};
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct AuthorizationCheck {
    pub resource: Resource,
    pub action: Action,
    pub context: AuthorizationContext,
}

#[derive(Clone)]
pub struct AuthorizationService {
    db: PgPool,
}

impl AuthorizationService {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub async fn authorize(
        &self,
        user: &AuthenticatedUser,
        mut check: AuthorizationCheck,
    ) -> Result<(), ApiError> {
        // Sensor and Refresh tokens have dedicated scope checks elsewhere and
        // are not subject to identity-based RBAC.
        //
        // Access tokens use identity/role assignments. Execution tokens are
        // constrained to permission set refs embedded by the worker at token
        // mint time; they never inherit the triggering identity's full RBAC.
        match user.claims.token_type {
            TokenType::Access | TokenType::Execution => {}
            _ => return Ok(()),
        }

        let identity_id = user.identity_id().map_err(|_| {
            ApiError::Unauthorized("Invalid authentication subject in token".to_string())
        })?;

        // Ensure identity exists and load identity attributes used by attribute constraints.
        let identity = IdentityRepository::find_by_id(&self.db, identity_id)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("Identity not found".to_string()))?;

        check.context.identity_id = identity_id;
        check.context.identity_attributes = match identity.attributes {
            serde_json::Value::Object(map) => map.into_iter().collect(),
            _ => Default::default(),
        };

        let grants = self.load_grants_for_token(user, identity_id).await?;

        let allowed = Self::is_allowed(&grants, check.resource, check.action, &check.context);

        if !allowed {
            self.emit_rbac_denied(user, &check);
            return Err(ApiError::Forbidden(format!(
                "Insufficient permissions: {}:{}",
                resource_name(check.resource),
                action_name(check.action)
            )));
        }

        Ok(())
    }

    fn emit_rbac_denied(&self, user: &AuthenticatedUser, check: &AuthorizationCheck) {
        let pool = self.db.clone();
        let event = build_rbac_denied_event(user, check);

        tokio::spawn(async move {
            if let Err(err) = AuditRepository::insert(&pool, event).await {
                tracing::error!(error = %err, "failed to persist RBAC denial audit event");
            }
        });
    }

    pub async fn effective_grants(&self, user: &AuthenticatedUser) -> Result<Vec<Grant>, ApiError> {
        match user.claims.token_type {
            TokenType::Access | TokenType::Execution => {}
            _ => return Ok(Vec::new()),
        }

        let identity_id = user.identity_id().map_err(|_| {
            ApiError::Unauthorized("Invalid authentication subject in token".to_string())
        })?;
        self.load_grants_for_token(user, identity_id).await
    }

    /// Returns true when the current token's effective grants are at least
    /// sufficient to grant every resource/action pair in the named permission
    /// sets to a child execution token.
    pub async fn can_delegate_permission_sets(
        &self,
        user: &AuthenticatedUser,
        permission_set_refs: &[String],
    ) -> Result<bool, ApiError> {
        let permission_set_refs = named_execution_permission_set_refs(permission_set_refs);
        if permission_set_refs.is_empty() {
            return Ok(true);
        }

        let identity_id = user.identity_id().map_err(|_| {
            ApiError::Unauthorized("Invalid authentication subject in token".to_string())
        })?;
        let identity = IdentityRepository::find_by_id(&self.db, identity_id)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("Identity not found".to_string()))?;
        let mut ctx = AuthorizationContext::new(identity_id);
        ctx.identity_attributes = match identity.attributes {
            serde_json::Value::Object(map) => map.into_iter().collect(),
            _ => Default::default(),
        };

        let current_grants = self.load_grants_for_token(user, identity_id).await?;
        let requested_sets =
            PermissionSetRepository::find_by_refs(&self.db, &permission_set_refs).await?;
        if requested_sets.len() != permission_set_refs.len() {
            return Ok(false);
        }
        let requested_grants = Self::grants_from_permission_sets(requested_sets)?;

        Ok(requested_grants.iter().all(|grant| {
            grant
                .actions
                .iter()
                .all(|action| Self::is_allowed(&current_grants, grant.resource, *action, &ctx))
        }))
    }

    pub fn is_allowed(
        grants: &[Grant],
        resource: Resource,
        action: Action,
        context: &AuthorizationContext,
    ) -> bool {
        grants.iter().any(|g| g.allows(resource, action, context))
    }

    async fn load_effective_grants(&self, identity_id: i64) -> Result<Vec<Grant>, ApiError> {
        let mut permission_sets =
            PermissionSetRepository::find_by_identity(&self.db, identity_id).await?;
        let roles =
            IdentityRoleAssignmentRepository::find_role_names_by_identity(&self.db, identity_id)
                .await?;
        let role_permission_sets = PermissionSetRepository::find_by_roles(&self.db, &roles).await?;
        permission_sets.extend(role_permission_sets);

        let mut seen_permission_sets = std::collections::HashSet::new();
        permission_sets.retain(|permission_set| seen_permission_sets.insert(permission_set.id));

        let mut grants = Vec::new();
        for permission_set in permission_sets {
            let set_grants: Vec<Grant> =
                serde_json::from_value(permission_set.grants).map_err(|e| {
                    ApiError::InternalServerError(format!(
                        "Invalid grant schema in permission set '{}': {}",
                        permission_set.r#ref, e
                    ))
                })?;
            grants.extend(set_grants);
        }

        Ok(grants)
    }

    async fn load_grants_for_token(
        &self,
        user: &AuthenticatedUser,
        identity_id: i64,
    ) -> Result<Vec<Grant>, ApiError> {
        match user.claims.token_type {
            TokenType::Access => self.load_effective_grants(identity_id).await,
            TokenType::Execution => {
                let refs = execution_permission_set_refs(user);
                let permission_sets =
                    PermissionSetRepository::find_by_refs(&self.db, &refs).await?;
                if permission_sets.len() != refs.len() {
                    let found: std::collections::HashSet<_> = permission_sets
                        .iter()
                        .map(|set| set.r#ref.as_str())
                        .collect();
                    let missing: Vec<_> = refs
                        .iter()
                        .filter(|r| !found.contains(r.as_str()))
                        .cloned()
                        .collect();
                    return Err(ApiError::Forbidden(format!(
                        "Execution token references unavailable permission sets: {}",
                        missing.join(", ")
                    )));
                }
                let mut grants = Self::grants_from_permission_sets(permission_sets)?;
                grants.extend(execution_standard_access_grants(user));
                Ok(grants)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn grants_from_permission_sets(
        permission_sets: Vec<attune_common::models::PermissionSet>,
    ) -> Result<Vec<Grant>, ApiError> {
        let mut grants = Vec::new();
        for permission_set in permission_sets {
            let set_grants: Vec<Grant> =
                serde_json::from_value(permission_set.grants).map_err(|e| {
                    ApiError::InternalServerError(format!(
                        "Invalid grant schema in permission set '{}': {}",
                        permission_set.r#ref, e
                    ))
                })?;
            grants.extend(set_grants);
        }
        Ok(grants)
    }
}

pub fn execution_permission_set_refs(user: &AuthenticatedUser) -> Vec<String> {
    named_execution_permission_set_refs(&execution_access_refs(user))
}

pub fn execution_has_standard_access(user: &AuthenticatedUser) -> bool {
    user.claims.token_type == TokenType::Execution
        && execution_access_refs(user)
            .iter()
            .any(|value| value == STANDARD_EXECUTION_ACCESS_REF)
}

pub fn execution_standard_pack_refs(user: &AuthenticatedUser) -> Vec<String> {
    metadata_string_array(user, "standard_access_pack_refs")
}

pub fn execution_standard_owner_refs(user: &AuthenticatedUser) -> Vec<String> {
    let mut refs = execution_standard_pack_refs(user);
    refs.extend(metadata_string_array(user, "standard_access_action_refs"));
    refs.sort();
    refs.dedup();
    refs
}

fn execution_access_refs(user: &AuthenticatedUser) -> Vec<String> {
    user.claims
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("permission_set_refs"))
        .and_then(|value| value.as_array())
        .map(|refs| {
            refs.iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn named_execution_permission_set_refs(refs: &[String]) -> Vec<String> {
    refs.iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty() && *value != STANDARD_EXECUTION_ACCESS_REF)
        .map(ToOwned::to_owned)
        .collect()
}

fn metadata_string_array(user: &AuthenticatedUser, key: &str) -> Vec<String> {
    user.claims
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(key))
        .and_then(|value| value.as_array())
        .map(|refs| {
            refs.iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn execution_standard_access_grants(user: &AuthenticatedUser) -> Vec<Grant> {
    if !execution_has_standard_access(user) {
        return Vec::new();
    }

    let owner_refs = execution_standard_owner_refs(user);
    if owner_refs.is_empty() {
        return Vec::new();
    }

    vec![
        Grant {
            resource: Resource::Keys,
            actions: vec![Action::Read, Action::Decrypt],
            constraints: Some(GrantConstraints {
                owner_types: Some(vec![OwnerType::Pack, OwnerType::Action]),
                owner_refs: Some(owner_refs.clone()),
                ..Default::default()
            }),
        },
        Grant {
            resource: Resource::Artifacts,
            actions: vec![Action::Read, Action::Create, Action::Update, Action::Delete],
            constraints: Some(GrantConstraints {
                owner_types: Some(vec![OwnerType::Pack, OwnerType::Action]),
                owner_refs: Some(owner_refs),
                ..Default::default()
            }),
        },
    ]
}

fn build_rbac_denied_event(
    user: &AuthenticatedUser,
    check: &AuthorizationCheck,
) -> PendingAuditEvent {
    let resource = resource_name(check.resource);
    let action = action_name(check.action);
    let ctx = &check.context;
    let mut builder = AuditEventBuilder::new(
        AuditCategory::Rbac,
        event_type::rbac::DENIED,
        AuditOutcome::Denied,
    )
    .actor_identity(ctx.identity_id)
    .actor_login(user.login().to_string())
    .actor_token_type(format!("{:?}", user.claims.token_type).to_lowercase())
    .resource(resource);
    if let Some(target_id) = ctx.target_id {
        builder = builder.resource_id(target_id);
    }
    if let Some(target_ref) = &ctx.target_ref {
        builder = builder.resource_ref(target_ref.clone());
    }
    builder
        .with_details(serde_json::json!({
            "resource": resource,
            "action": action,
            "target_id": ctx.target_id,
            "target_ref": ctx.target_ref,
            "pack_ref": ctx.pack_ref,
            "owner_identity_id": ctx.owner_identity_id,
            "owner_type": ctx.owner_type,
            "owner_ref": ctx.owner_ref,
            "visibility": ctx.visibility,
            "encrypted": ctx.encrypted,
            "reason": "grant_not_found_or_constraints_not_matched",
        }))
        .build()
}

fn resource_name(resource: Resource) -> &'static str {
    match resource {
        Resource::Packs => "packs",
        Resource::Actions => "actions",
        Resource::Queues => "queues",
        Resource::QueueItems => "queue_items",
        Resource::Rules => "rules",
        Resource::Triggers => "triggers",
        Resource::Executions => "executions",
        Resource::Events => "events",
        Resource::Enforcements => "enforcements",
        Resource::Inquiries => "inquiries",
        Resource::Keys => "keys",
        Resource::Artifacts => "artifacts",
        Resource::Runtimes => "runtimes",
        Resource::Workers => "workers",
        Resource::Retention => "retention",
        Resource::Identities => "identities",
        Resource::Permissions => "permissions",
        Resource::AuditLog => "audit_log",
    }
}

fn action_name(action: Action) -> &'static str {
    match action {
        Action::Read => "read",
        Action::Create => "create",
        Action::Install => "install",
        Action::Configure => "configure",
        Action::Update => "update",
        Action::Delete => "delete",
        Action::Execute => "execute",
        Action::Cancel => "cancel",
        Action::Respond => "respond",
        Action::Manage => "manage",
        Action::Decrypt => "decrypt",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::jwt::{Claims, TokenType};

    fn test_user() -> AuthenticatedUser {
        AuthenticatedUser {
            claims: Claims {
                sub: "42".to_string(),
                login: "auditor@example.test".to_string(),
                iat: 1,
                exp: 999_999,
                token_type: TokenType::Access,
                scope: None,
                metadata: None,
            },
        }
    }

    #[test]
    fn execution_permission_set_refs_read_from_token_metadata() {
        let user = AuthenticatedUser {
            claims: Claims {
                sub: "42".to_string(),
                login: "execution:123".to_string(),
                iat: 1,
                exp: 999_999,
                token_type: TokenType::Execution,
                scope: Some("execution".to_string()),
                metadata: Some(serde_json::json!({
                    "execution_id": 123,
                    "permission_set_refs": ["standard", "core.agent_reader", "", " core.agent_writer "],
                })),
            },
        };

        assert_eq!(
            execution_permission_set_refs(&user),
            vec![
                "core.agent_reader".to_string(),
                "core.agent_writer".to_string()
            ]
        );
    }

    #[test]
    fn execution_standard_access_grants_cover_action_and_pack_resources() {
        let user = AuthenticatedUser {
            claims: Claims {
                sub: "42".to_string(),
                login: "execution:123".to_string(),
                iat: 1,
                exp: 999_999,
                token_type: TokenType::Execution,
                scope: Some("execution".to_string()),
                metadata: Some(serde_json::json!({
                    "execution_id": 123,
                    "permission_set_refs": ["standard"],
                    "standard_access_pack_refs": ["salesforce", "workflow_pack"],
                    "standard_access_action_refs": ["salesforce.read_sobject", "workflow_pack.sync"],
                })),
            },
        };

        let grants = execution_standard_access_grants(&user);
        let mut pack_key_ctx = AuthorizationContext::new(42);
        pack_key_ctx.owner_type = Some(OwnerType::Pack);
        pack_key_ctx.owner_ref = Some("workflow_pack".to_string());
        pack_key_ctx.encrypted = Some(true);
        assert!(AuthorizationService::is_allowed(
            &grants,
            Resource::Keys,
            Action::Decrypt,
            &pack_key_ctx
        ));

        let mut action_artifact_ctx = AuthorizationContext::new(42);
        action_artifact_ctx.owner_type = Some(OwnerType::Action);
        action_artifact_ctx.owner_ref = Some("salesforce.read_sobject".to_string());
        assert!(AuthorizationService::is_allowed(
            &grants,
            Resource::Artifacts,
            Action::Create,
            &action_artifact_ctx
        ));

        let mut unrelated_ctx = AuthorizationContext::new(42);
        unrelated_ctx.owner_type = Some(OwnerType::Pack);
        unrelated_ctx.owner_ref = Some("unrelated".to_string());
        assert!(!AuthorizationService::is_allowed(
            &grants,
            Resource::Keys,
            Action::Read,
            &unrelated_ctx
        ));
    }

    #[test]
    fn rbac_denied_audit_event_contains_decision_context() {
        let mut ctx = AuthorizationContext::new(42);
        ctx.target_id = Some(7);
        ctx.target_ref = Some("secret.key".to_string());
        ctx.owner_identity_id = Some(99);
        ctx.encrypted = Some(true);

        let event = build_rbac_denied_event(
            &test_user(),
            &AuthorizationCheck {
                resource: Resource::Keys,
                action: Action::Decrypt,
                context: ctx,
            },
        );

        assert_eq!(event.category, AuditCategory::Rbac);
        assert_eq!(event.event_type, event_type::rbac::DENIED);
        assert_eq!(event.outcome, AuditOutcome::Denied);
        assert_eq!(event.actor_identity, Some(42));
        assert_eq!(event.resource_type.as_deref(), Some("keys"));
        assert_eq!(event.resource_id, Some(7));
        assert_eq!(event.resource_ref.as_deref(), Some("secret.key"));

        let details = event.details.expect("details");
        assert_eq!(details["resource"], "keys");
        assert_eq!(details["action"], "decrypt");
        assert_eq!(details["owner_identity_id"], 99);
        assert_eq!(
            details["reason"],
            "grant_not_found_or_constraints_not_matched"
        );
    }
}
