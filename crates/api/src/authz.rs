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
    metadata_cache::MetadataCache,
    models::{OwnerType, PermissionSet},
    mq::{IdentityAuthorizationChangedPayload, PermissionSetChangedPayload},
    rbac::{Action, AuthorizationContext, Grant, GrantConstraints, Resource},
    repositories::{
        identity::{IdentityRepository, IdentityRoleAssignmentRepository, PermissionSetRepository},
        FindById,
    },
};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    OnceLock,
};
use std::time::Duration;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct AuthorizationCheck {
    pub resource: Resource,
    pub action: Action,
    pub context: AuthorizationContext,
}

/// A per-request snapshot of the requesting identity's attributes and
/// effective grants. Load once via [`AuthorizationService::load_snapshot`]
/// and reuse across multiple authorization checks (and visibility-scope
/// derivations) for the same request instead of re-querying identity/grants
/// for each check.
#[derive(Debug, Clone)]
pub struct AuthorizationSnapshot {
    pub identity_id: i64,
    pub identity_attributes: HashMap<String, serde_json::Value>,
    pub grants: Vec<Grant>,
}

#[derive(Clone)]
pub struct AuthorizationService {
    db: PgPool,
}

const AUTHZ_CACHE_TTL: Duration = Duration::from_secs(5);
const AUTHZ_CACHE_MAX_ENTRIES: usize = 4096;
const AUTHZ_CACHE_ENABLED_ENV: &str = "ATTUNE_AUTHZ_CACHE_ENABLED";
const AUTHZ_ROLE_CACHE_ENABLED_ENV: &str = "ATTUNE_AUTHZ_ROLE_CACHE_ENABLED";
const AUTHZ_GRANTS_CACHE_ENABLED_ENV: &str = "ATTUNE_AUTHZ_GRANTS_CACHE_ENABLED";
const AUTHZ_PERMISSION_SET_CACHE_ENABLED_ENV: &str = "ATTUNE_AUTHZ_PERMISSION_SET_CACHE_ENABLED";
const AUTHZ_IDENTITY_CACHE_ENABLED_ENV: &str = "ATTUNE_AUTHZ_IDENTITY_CACHE_ENABLED";
const AUTHZ_CACHE_SHADOW_SAMPLE_RATE_ENV: &str = "ATTUNE_AUTHZ_CACHE_SHADOW_SAMPLE_RATE";

fn parse_env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .as_deref()
        .map(str::trim)
        .map(|value| !matches!(value, "0" | "false" | "FALSE" | "False" | "off" | "OFF"))
        .unwrap_or(default)
}

fn authz_cache_enabled() -> bool {
    static ENABLED: OnceLock<AtomicBool> = OnceLock::new();
    let enabled = ENABLED.get_or_init(|| {
        let parsed = parse_env_bool(AUTHZ_CACHE_ENABLED_ENV, true);
        AtomicBool::new(parsed)
    });
    enabled.load(Ordering::Relaxed)
}

fn role_cache_enabled() -> bool {
    authz_cache_enabled() && parse_env_bool(AUTHZ_ROLE_CACHE_ENABLED_ENV, true)
}

fn grants_cache_enabled() -> bool {
    authz_cache_enabled() && parse_env_bool(AUTHZ_GRANTS_CACHE_ENABLED_ENV, true)
}

fn permission_set_cache_enabled() -> bool {
    authz_cache_enabled() && parse_env_bool(AUTHZ_PERMISSION_SET_CACHE_ENABLED_ENV, true)
}

fn identity_cache_enabled() -> bool {
    authz_cache_enabled() && parse_env_bool(AUTHZ_IDENTITY_CACHE_ENABLED_ENV, true)
}

fn cache_shadow_sample_rate() -> f64 {
    static RATE: OnceLock<f64> = OnceLock::new();
    *RATE.get_or_init(|| {
        std::env::var(AUTHZ_CACHE_SHADOW_SAMPLE_RATE_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .map(|rate| rate.clamp(0.0, 1.0))
            .unwrap_or(0.0)
    })
}

fn should_shadow_read() -> bool {
    let rate = cache_shadow_sample_rate();
    if rate <= 0.0 {
        return false;
    }
    if rate >= 1.0 {
        return true;
    }
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let mut n = COUNTER.fetch_add(1, Ordering::Relaxed);
    n = n.wrapping_mul(6364136223846793005).wrapping_add(1);
    (n as f64 / u64::MAX as f64) < rate
}

fn role_names_cache() -> &'static MetadataCache<String, Vec<String>> {
    static CACHE: OnceLock<MetadataCache<String, Vec<String>>> = OnceLock::new();
    CACHE.get_or_init(|| MetadataCache::new(AUTHZ_CACHE_TTL, AUTHZ_CACHE_MAX_ENTRIES))
}

fn access_grants_cache() -> &'static MetadataCache<String, Vec<Grant>> {
    static CACHE: OnceLock<MetadataCache<String, Vec<Grant>>> = OnceLock::new();
    CACHE.get_or_init(|| MetadataCache::new(AUTHZ_CACHE_TTL, AUTHZ_CACHE_MAX_ENTRIES))
}

fn permission_sets_by_refs_cache() -> &'static MetadataCache<String, Vec<PermissionSet>> {
    static CACHE: OnceLock<MetadataCache<String, Vec<PermissionSet>>> = OnceLock::new();
    CACHE.get_or_init(|| MetadataCache::new(AUTHZ_CACHE_TTL, AUTHZ_CACHE_MAX_ENTRIES))
}

fn identity_attributes_cache() -> &'static MetadataCache<String, HashMap<String, serde_json::Value>>
{
    static CACHE: OnceLock<MetadataCache<String, HashMap<String, serde_json::Value>>> =
        OnceLock::new();
    CACHE.get_or_init(|| MetadataCache::new(AUTHZ_CACHE_TTL, AUTHZ_CACHE_MAX_ENTRIES))
}

fn identity_key(identity_id: i64) -> String {
    identity_id.to_string()
}

fn refs_key(refs: &[String]) -> String {
    let mut normalized: Vec<String> = refs.iter().map(|value| value.trim().to_string()).collect();
    normalized.sort();
    normalized.join("|")
}

impl AuthorizationService {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub async fn invalidate_identity_authz_cache(identity_id: i64) {
        if !authz_cache_enabled() {
            return;
        }
        let key = identity_key(identity_id);
        if role_cache_enabled() {
            let _ = role_names_cache().invalidate_key(&key).await;
        }
        if grants_cache_enabled() {
            let _ = access_grants_cache().invalidate_key(&key).await;
        }
        if identity_cache_enabled() {
            let _ = identity_attributes_cache().invalidate_key(&key).await;
        }
    }

    pub async fn invalidate_permission_set_caches() {
        if !authz_cache_enabled() {
            return;
        }
        if permission_set_cache_enabled() {
            permission_sets_by_refs_cache().invalidate_all().await;
        }
        if grants_cache_enabled() {
            access_grants_cache().invalidate_all().await;
        }
    }

    pub async fn handle_permission_set_metadata_change(_payload: PermissionSetChangedPayload) {
        Self::invalidate_permission_set_caches().await;
    }

    pub async fn handle_identity_authorization_metadata_change(
        payload: IdentityAuthorizationChangedPayload,
    ) {
        Self::invalidate_identity_authz_cache(payload.identity_id).await;
    }

    pub async fn authorize(
        &self,
        user: &AuthenticatedUser,
        check: AuthorizationCheck,
    ) -> Result<(), ApiError> {
        let snapshot = self.load_snapshot(user).await?;
        self.authorize_with_snapshot(user, snapshot.as_ref(), check)
    }

    /// Loads the requesting identity's attributes and effective grants once so
    /// callers that need multiple authorization checks (or visibility scopes)
    /// for the same request can reuse them via [`Self::authorize_with_snapshot`]
    /// instead of re-querying identity/grants per check.
    ///
    /// Returns `None` for token types that are not subject to identity-based
    /// RBAC (e.g. sensor/refresh tokens), matching [`Self::authorize`]'s bypass
    /// behavior for those token types.
    pub async fn load_snapshot(
        &self,
        user: &AuthenticatedUser,
    ) -> Result<Option<AuthorizationSnapshot>, ApiError> {
        // Sensor and Refresh tokens have dedicated scope checks elsewhere and
        // are not subject to identity-based RBAC.
        //
        // Access tokens use identity/role assignments. Execution tokens are
        // constrained to permission set refs embedded by the worker at token
        // mint time; they never inherit the triggering identity's full RBAC.
        match user.claims.token_type {
            TokenType::Access | TokenType::Execution => {}
            _ => return Ok(None),
        }

        let identity_id = user.identity_id().map_err(|_| {
            ApiError::Unauthorized("Invalid authentication subject in token".to_string())
        })?;

        let identity_attributes = self.load_identity_attributes_cached(identity_id).await?;
        let grants = self.load_grants_for_token(user, identity_id).await?;

        Ok(Some(AuthorizationSnapshot {
            identity_id,
            identity_attributes,
            grants,
        }))
    }

    /// Evaluates `check` against a snapshot previously loaded via
    /// [`Self::load_snapshot`]. A `None` snapshot means the caller's token
    /// type is not subject to identity-based RBAC, so the check passes (same
    /// bypass behavior as [`Self::authorize`]).
    pub fn authorize_with_snapshot(
        &self,
        user: &AuthenticatedUser,
        snapshot: Option<&AuthorizationSnapshot>,
        mut check: AuthorizationCheck,
    ) -> Result<(), ApiError> {
        let Some(snapshot) = snapshot else {
            return Ok(());
        };

        check.context.identity_id = snapshot.identity_id;
        check.context.identity_attributes = snapshot.identity_attributes.clone();

        let allowed = Self::is_allowed(
            &snapshot.grants,
            check.resource,
            check.action,
            &check.context,
        );

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
        self.can_delegate_permission_sets_with_snapshot(user, None, permission_set_refs)
            .await
    }

    /// Same as [`Self::can_delegate_permission_sets`] but reuses a pre-loaded
    /// snapshot (identity attributes + effective grants) instead of
    /// re-fetching them, when one is available for the current request.
    pub async fn can_delegate_permission_sets_with_snapshot(
        &self,
        user: &AuthenticatedUser,
        snapshot: Option<&AuthorizationSnapshot>,
        permission_set_refs: &[String],
    ) -> Result<bool, ApiError> {
        let permission_set_refs = named_execution_permission_set_refs(permission_set_refs);
        if permission_set_refs.is_empty() {
            return Ok(true);
        }

        let identity_id = user.identity_id().map_err(|_| {
            ApiError::Unauthorized("Invalid authentication subject in token".to_string())
        })?;

        let (identity_attributes, current_grants) = match snapshot {
            Some(snapshot) => (
                snapshot.identity_attributes.clone(),
                snapshot.grants.clone(),
            ),
            None => {
                let identity_attributes = self.load_identity_attributes_cached(identity_id).await?;
                let current_grants = self.load_grants_for_token(user, identity_id).await?;
                (identity_attributes, current_grants)
            }
        };
        let mut ctx = AuthorizationContext::new(identity_id);
        ctx.identity_attributes = identity_attributes;

        let requested_sets = self
            .find_permission_sets_by_refs_cached(&permission_set_refs)
            .await?;
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

    async fn load_identity_attributes_cached(
        &self,
        identity_id: i64,
    ) -> Result<HashMap<String, serde_json::Value>, ApiError> {
        if identity_cache_enabled() {
            let key = identity_key(identity_id);
            if let Some(attributes) = identity_attributes_cache().get(&key).await {
                debug!(
                    entity = "authz_identity_attributes",
                    operation = "load_identity_attributes",
                    cache_hit = true,
                    identity_id
                );
                if should_shadow_read() {
                    let fresh = self.load_identity_attributes_uncached(identity_id).await?;
                    if attributes != fresh {
                        warn!(
                            entity = "authz_identity_attributes",
                            operation = "shadow_compare",
                            identity_id,
                            "cache/db mismatch detected for cached identity attributes"
                        );
                    }
                }
                return Ok(attributes);
            }
        }

        let attributes = self.load_identity_attributes_uncached(identity_id).await?;
        if identity_cache_enabled() {
            let key = identity_key(identity_id);
            identity_attributes_cache()
                .insert(key, attributes.clone())
                .await;
            debug!(
                entity = "authz_identity_attributes",
                operation = "load_identity_attributes",
                cache_hit = false,
                identity_id
            );
        }

        Ok(attributes)
    }

    async fn load_identity_attributes_uncached(
        &self,
        identity_id: i64,
    ) -> Result<HashMap<String, serde_json::Value>, ApiError> {
        let identity = IdentityRepository::find_by_id(&self.db, identity_id)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("Identity not found".to_string()))?;
        Ok(match identity.attributes {
            serde_json::Value::Object(map) => map.into_iter().collect(),
            _ => Default::default(),
        })
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
        if grants_cache_enabled() {
            let key = identity_key(identity_id);
            if let Some(grants) = access_grants_cache().get(&key).await {
                debug!(
                    entity = "authz_access_grants",
                    operation = "load_effective_grants",
                    cache_hit = true,
                    identity_id
                );
                if should_shadow_read() {
                    let fresh = self.load_effective_grants_uncached(identity_id).await?;
                    if grants != fresh {
                        warn!(
                            entity = "authz_access_grants",
                            operation = "shadow_compare",
                            identity_id,
                            "cache/db mismatch detected for cached grants"
                        );
                    }
                }
                return Ok(grants);
            }
        }

        let grants = self.load_effective_grants_uncached(identity_id).await?;
        if grants_cache_enabled() {
            let key = identity_key(identity_id);
            access_grants_cache().insert(key, grants.clone()).await;
            debug!(
                entity = "authz_access_grants",
                operation = "load_effective_grants",
                cache_hit = false,
                identity_id
            );
        }

        Ok(grants)
    }

    async fn load_effective_grants_uncached(
        &self,
        identity_id: i64,
    ) -> Result<Vec<Grant>, ApiError> {
        let mut permission_sets =
            PermissionSetRepository::find_by_identity(&self.db, identity_id).await?;
        let roles = self.find_role_names_by_identity_cached(identity_id).await?;
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
                let permission_sets = self.find_permission_sets_by_refs_cached(&refs).await?;
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

    async fn find_role_names_by_identity_cached(
        &self,
        identity_id: i64,
    ) -> Result<Vec<String>, ApiError> {
        if role_cache_enabled() {
            let key = identity_key(identity_id);
            if let Some(roles) = role_names_cache().get(&key).await {
                debug!(
                    entity = "identity_role_names",
                    operation = "find_by_identity",
                    cache_hit = true,
                    identity_id
                );
                if should_shadow_read() {
                    let fresh = IdentityRoleAssignmentRepository::find_role_names_by_identity(
                        &self.db,
                        identity_id,
                    )
                    .await?;
                    let mut cached = roles.clone();
                    cached.sort();
                    let mut fresh_sorted = fresh;
                    fresh_sorted.sort();
                    if cached != fresh_sorted {
                        warn!(
                            entity = "identity_role_names",
                            operation = "shadow_compare",
                            identity_id,
                            "cache/db mismatch detected for cached role names"
                        );
                    }
                }
                return Ok(roles);
            }
        }

        let roles =
            IdentityRoleAssignmentRepository::find_role_names_by_identity(&self.db, identity_id)
                .await?;
        if role_cache_enabled() {
            let key = identity_key(identity_id);
            role_names_cache().insert(key, roles.clone()).await;
            debug!(
                entity = "identity_role_names",
                operation = "find_by_identity",
                cache_hit = false,
                identity_id
            );
        }
        Ok(roles)
    }

    async fn find_permission_sets_by_refs_cached(
        &self,
        refs: &[String],
    ) -> Result<Vec<PermissionSet>, ApiError> {
        if refs.is_empty() {
            return Ok(Vec::new());
        }

        let key = refs_key(refs);
        if permission_set_cache_enabled() {
            if let Some(permission_sets) = permission_sets_by_refs_cache().get(&key).await {
                debug!(
                    entity = "permission_sets_by_refs",
                    operation = "find_by_refs",
                    cache_hit = true,
                    refs_count = refs.len()
                );
                if should_shadow_read() {
                    let fresh = PermissionSetRepository::find_by_refs(&self.db, refs).await?;
                    let mut cached_ids: Vec<i64> =
                        permission_sets.iter().map(|set| set.id).collect();
                    let mut fresh_ids: Vec<i64> = fresh.iter().map(|set| set.id).collect();
                    cached_ids.sort_unstable();
                    fresh_ids.sort_unstable();
                    if cached_ids != fresh_ids {
                        warn!(
                            entity = "permission_sets_by_refs",
                            operation = "shadow_compare",
                            refs_count = refs.len(),
                            "cache/db mismatch detected for permission set refs lookup"
                        );
                    }
                }
                return Ok(permission_sets);
            }
        }

        let permission_sets = PermissionSetRepository::find_by_refs(&self.db, refs).await?;
        if permission_set_cache_enabled() {
            permission_sets_by_refs_cache()
                .insert(key, permission_sets.clone())
                .await;
            debug!(
                entity = "permission_sets_by_refs",
                operation = "find_by_refs",
                cache_hit = false,
                refs_count = refs.len()
            );
        }
        Ok(permission_sets)
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
        Resource::Policies => "policies",
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
        Resource::Dashboards => "dashboards",
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
    fn refs_key_is_order_insensitive() {
        let a = vec!["core.writer".to_string(), "core.reader".to_string()];
        let b = vec!["core.reader".to_string(), "core.writer".to_string()];
        assert_eq!(refs_key(&a), refs_key(&b));
    }

    #[test]
    fn refs_key_trims_inputs() {
        let refs = vec![
            " core.reader ".to_string(),
            "core.writer".to_string(),
            "core.reader".to_string(),
        ];
        assert_eq!(refs_key(&refs), "core.reader|core.reader|core.writer");
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
