//! Owner-scoped cache API routes.
//!
//! All routes are protected with [`RequireAuth`]. Authentication is not
//! authorization: access/execution tokens use effective RBAC cache grants,
//! sensor tokens use only their signed cache authority, and every other token
//! type fails closed. Authorization is always evaluated before any namespace
//! existence or freshness lookup so an inaccessible namespace never leaks its
//! existence. All database access goes through [`attune_common::repositories`]
//! cache repositories with canonical owner IDs.

use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use hmac::{digest::KeyInit, Hmac, Mac};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use attune_common::{
    audit::{AuditCategory, AuditEventBuilder, AuditOutcome, PendingAuditEvent},
    models::{
        cache::{CacheGeneration, CacheNamespace},
        CacheGenerationState, Id, OwnerType,
    },
    rbac::{Action, AuthorizationContext, ExecutionScopeConstraint, Grant, Resource},
    repositories::{
        action::ActionRepository,
        cache::{
            CacheEntryInput, CacheEntryRepository, CacheGenerationRepository,
            CacheIngestRepository, CacheNamespaceFreshnessFilter, CacheNamespaceGrantFilter,
            CacheNamespacePolicy, CacheNamespaceReadVisibility, CacheNamespaceRepository,
            CacheOwnerScope, CreateCacheGenerationInput, CreateCacheGenerationResult,
            CreateCacheNamespaceInput, InsertCacheChunkResult, SealCacheGenerationInput,
            MAX_INGEST_CHUNK_BYTES, MAX_MULTI_LOOKUP_IDS, MAX_SCAN_PAGE_SIZE,
        },
        pack::PackRepository,
        retention::RetentionRepository,
        trigger::SensorRepository,
        FindById, FindByRef,
    },
    Error as CommonError,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

use crate::{
    auth::{
        jwt::TokenType,
        middleware::{AuthErrorResponse, AuthenticatedUser},
        RequireAuth,
    },
    authz::{AuthorizationCheck, AuthorizationService, AuthorizationSnapshot},
    dto::{
        cache::{
            CacheEntryResponse, CacheForbiddenResponse, CacheGenerationApiResponse,
            CacheGenerationListApiResponse, CacheGenerationListQuery, CacheGenerationListResponse,
            CacheGenerationResponse, CacheMultiLookupApiResponse, CacheMultiLookupRequest,
            CacheMultiLookupResponse, CacheNamespaceApiResponse, CacheNamespaceDeletionApiResponse,
            CacheNamespaceDeletionResponse, CacheNamespaceFreshness, CacheNamespaceListApiResponse,
            CacheNamespaceListQuery, CacheNamespaceListResponse, CacheNamespacePolicyBody,
            CacheNamespaceResponse, CacheOwnerBody, CacheOwnerQuery, CachePointLookupApiResponse,
            CachePointLookupRequest, CachePointLookupResponse, CacheScanPageApiResponse,
            CacheScanPageResponse, CacheScanQuery, CreateCacheGenerationRequest,
            CreateCacheNamespaceRequest, PromoteCacheGenerationRequest, SealCacheGenerationRequest,
            UpdateCacheNamespaceRequest, UploadCacheChunkRequest,
        },
        ApiResponse,
    },
    middleware::{error::ErrorResponse, ApiError},
    state::AppState,
};

/// Opaque cursor format version. Increment when the payload shape changes so
/// old cursors fail closed instead of being misread.
const CURSOR_VERSION: u8 = 1;
/// Default scan page size when the caller does not request one.
const DEFAULT_SCAN_PAGE_SIZE: i64 = 100;
const DEFAULT_METADATA_PAGE_SIZE: i64 = 100;
const MAX_METADATA_PAGE_SIZE: i64 = 500;
/// Serialized-byte budget for one scan page. Pages stop early and issue a
/// cursor once this budget is reached so response size stays bounded even when
/// individual records are large.
const MAX_SCAN_PAGE_BYTES: i64 = 2 * 1024 * 1024;
type HmacSha256 = Hmac<Sha256>;

/// Cache audit event types. Emitted as summaries only; never payloads or raw
/// external-ID lists.
mod cache_event {
    pub const NAMESPACE_CREATED: &str = "cache.namespace.created";
    pub const NAMESPACE_UPDATED: &str = "cache.namespace.updated";
    pub const NAMESPACE_TOMBSTONED: &str = "cache.namespace.tombstoned";
    pub const GENERATION_CREATED: &str = "cache.generation.created";
    pub const GENERATION_CHUNK_UPLOADED: &str = "cache.generation.chunk_uploaded";
    pub const GENERATION_SEALED: &str = "cache.generation.sealed";
    pub const GENERATION_PROMOTED: &str = "cache.generation.promoted";
    pub const GENERATION_ABANDONED: &str = "cache.generation.abandoned";
}

/// Error type for cache routes. Wraps [`ApiError`] for generic failures and
/// carries stable machine codes for cache-specific outcomes so clients can
/// distinguish `cache_not_populated`, `snapshot_expired`, `namespace_deleted`,
/// quota, conflict, and precondition conditions without existence leakage.
#[derive(Debug)]
pub enum CacheApiError {
    Api(ApiError),
    Coded {
        status: StatusCode,
        code: &'static str,
        message: String,
    },
}

impl CacheApiError {
    fn coded(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded {
            status,
            code,
            message: message.into(),
        }
    }

    fn not_populated() -> Self {
        Self::coded(
            StatusCode::CONFLICT,
            "cache_not_populated",
            "cache namespace has no active generation",
        )
    }

    fn snapshot_expired(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::CONFLICT, "snapshot_expired", message)
    }

    fn namespace_deleted() -> Self {
        Self::coded(
            StatusCode::CONFLICT,
            "namespace_deleted",
            "cache namespace has been deleted",
        )
    }

    fn quota(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::CONFLICT, "cache_quota_exceeded", message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::CONFLICT, "cache_conflict", message)
    }

    fn pack_managed_namespace(operation: &str) -> Self {
        Self::coded(
            StatusCode::CONFLICT,
            "pack_managed_namespace",
            format!(
                "pack-managed cache namespaces cannot be {operation} through the namespace API; update the managing pack definition instead"
            ),
        )
    }

    fn precondition(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::CONFLICT, "cache_precondition_failed", message)
    }

    fn stale(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::CONFLICT, "cache_stale", message)
    }

    fn cursor_invalid(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::BAD_REQUEST, "cache_cursor_invalid", message)
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::Api(ApiError::BadRequest(message.into()))
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::Api(ApiError::Forbidden(message.into()))
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::Api(ApiError::NotFound(message.into()))
    }
}

impl From<ApiError> for CacheApiError {
    fn from(err: ApiError) -> Self {
        Self::Api(err)
    }
}

impl From<CommonError> for CacheApiError {
    fn from(err: CommonError) -> Self {
        Self::Api(ApiError::from(err))
    }
}

impl From<sqlx::Error> for CacheApiError {
    fn from(err: sqlx::Error) -> Self {
        Self::Api(ApiError::from(err))
    }
}

impl IntoResponse for CacheApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(err) => err.into_response(),
            Self::Coded {
                status,
                code,
                message,
            } => {
                match code {
                    "snapshot_expired" => tracing::warn!(
                        component = "cache_api",
                        metric_set = "cache_api_outcomes",
                        cache_expired_snapshot_response_count = 1u64,
                        status = status.as_u16(),
                        "Cache API returned an expired snapshot response"
                    ),
                    code if code == "cache_quota_exceeded" || code.ends_with("_limit_exceeded") => {
                        tracing::warn!(
                            component = "cache_api",
                            metric_set = "cache_api_outcomes",
                            cache_quota_rejection_count = 1u64,
                            quota_code = code,
                            status = status.as_u16(),
                            "Cache API rejected an operation because of quota"
                        )
                    }
                    _ => {}
                }
                let body = crate::middleware::error::ErrorResponse::new(message).with_code(code);
                (status, Json(body)).into_response()
            }
        }
    }
}

type CacheResult<T> = Result<T, CacheApiError>;

/// Translates a repository write error into the appropriate cache machine code,
/// keeping quota, tombstone, precondition, and conflict conditions distinct
/// while never revealing inaccessible existence.
fn map_write_error(err: CommonError) -> CacheApiError {
    match &err {
        // Typed cache ingestion/lifecycle errors carry no raw external IDs and
        // get distinct, actionable machine codes.
        CommonError::CacheDuplicateExternalId => CacheApiError::coded(
            StatusCode::CONFLICT,
            "cache_duplicate_external_id",
            "cache ingest contains duplicate external identifiers",
        ),
        CommonError::CacheSnapshotExpired(message) => {
            CacheApiError::snapshot_expired(message.clone())
        }
        CommonError::CacheQuotaExceeded { code, message } => {
            CacheApiError::coded(StatusCode::CONFLICT, code, *message)
        }
        CommonError::AlreadyExists { .. } => CacheApiError::conflict(err.to_string()),
        CommonError::InvalidState(message) => {
            let lowered = message.to_ascii_lowercase();
            if lowered.contains("tombstoned") {
                CacheApiError::namespace_deleted()
            } else if lowered.contains("active generation changed")
                || lowered.contains("expected_")
                || lowered.contains("does not match")
            {
                CacheApiError::precondition(message.clone())
            } else {
                CacheApiError::conflict(message.clone())
            }
        }
        CommonError::Validation(message) => {
            let lowered = message.to_ascii_lowercase();
            if lowered.contains("quota") {
                CacheApiError::quota(message.clone())
            } else {
                CacheApiError::bad_request(message.clone())
            }
        }
        _ => CacheApiError::from(err),
    }
}

fn audit_write_error_reason(err: &CommonError) -> &'static str {
    match err {
        CommonError::CacheQuotaExceeded { .. } => "quota",
        CommonError::Validation(message) if message.to_ascii_lowercase().contains("quota") => {
            "quota"
        }
        CommonError::InvalidState(message)
            if message
                .to_ascii_lowercase()
                .contains("active generation changed")
                || message.to_ascii_lowercase().contains("expected_")
                || message.to_ascii_lowercase().contains("does not match") =>
        {
            "precondition"
        }
        CommonError::AlreadyExists { .. }
        | CommonError::InvalidState(_)
        | CommonError::CacheDuplicateExternalId => "conflict",
        CommonError::Validation(_) => "validation",
        _ => "internal",
    }
}

fn parse_json_with_required_field<T: DeserializeOwned>(
    body: &Bytes,
    required_field: &str,
    description: &str,
) -> CacheResult<T> {
    let value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|err| CacheApiError::bad_request(format!("invalid {description}: {err}")))?;
    if !value
        .as_object()
        .is_some_and(|object| object.contains_key(required_field))
    {
        return Err(CacheApiError::bad_request(format!(
            "{required_field} is required and must be an integer or explicit null"
        )));
    }
    serde_json::from_value(value)
        .map_err(|err| CacheApiError::bad_request(format!("invalid {description}: {err}")))
}

// ---------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------

/// Versioned, integrity-protected, opaque scan cursor. It is bound to the
/// namespace, generation, page shape, and an expiry that never exceeds the
/// generation's `readable_until` or the caller's token expiration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CacheCursor {
    v: u8,
    #[serde(rename = "n")]
    namespace_id: Id,
    #[serde(rename = "g")]
    generation_id: Id,
    #[serde(rename = "p")]
    page_size: i64,
    #[serde(rename = "f")]
    require_fresh: bool,
    #[serde(rename = "e")]
    last_external_id: String,
    #[serde(rename = "x")]
    expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CacheNamespaceCursor {
    v: u8,
    owner_type: Option<OwnerType>,
    owner_ref: Option<String>,
    namespace_filter: Option<String>,
    freshness: Option<CacheNamespaceFreshness>,
    page_size: i64,
    after_id: Id,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CacheGenerationCursor {
    v: u8,
    namespace_id: Id,
    page_size: i64,
    before_created: DateTime<Utc>,
    before_id: Id,
}

fn encode_signed_cursor<T: Serialize>(secret: &str, cursor: &T) -> Result<String, CacheApiError> {
    let payload = serde_json::to_vec(cursor).map_err(|err| {
        CacheApiError::Api(ApiError::InternalServerError(format!(
            "failed to encode cache cursor: {err}"
        )))
    })?;
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| {
        CacheApiError::Api(ApiError::InternalServerError(
            "invalid cache cursor signing key".to_string(),
        ))
    })?;
    mac.update(payload_b64.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{payload_b64}.{signature}"))
}

fn decode_signed_cursor<T: DeserializeOwned>(
    secret: &str,
    token: &str,
) -> Result<T, CacheApiError> {
    let invalid = || CacheApiError::cursor_invalid("cache cursor is invalid");
    let (payload_b64, signature_b64) = token.split_once('.').ok_or_else(invalid)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|_| invalid())?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| {
        CacheApiError::Api(ApiError::InternalServerError(
            "invalid cache cursor signing key".to_string(),
        ))
    })?;
    mac.update(payload_b64.as_bytes());
    mac.verify_slice(&signature).map_err(|_| invalid())?;
    let payload = URL_SAFE_NO_PAD.decode(payload_b64).map_err(|_| invalid())?;
    serde_json::from_slice(&payload).map_err(|_| invalid())
}

fn encode_cursor(secret: &str, cursor: &CacheCursor) -> Result<String, CacheApiError> {
    encode_signed_cursor(secret, cursor)
}

fn decode_cursor(secret: &str, token: &str) -> Result<CacheCursor, CacheApiError> {
    let cursor: CacheCursor = decode_signed_cursor(secret, token)?;
    if cursor.v != CURSOR_VERSION {
        return Err(CacheApiError::cursor_invalid("cache cursor is invalid"));
    }
    Ok(cursor)
}

fn decode_namespace_cursor(
    secret: &str,
    token: &str,
) -> Result<CacheNamespaceCursor, CacheApiError> {
    let cursor: CacheNamespaceCursor = decode_signed_cursor(secret, token)?;
    if cursor.v != CURSOR_VERSION {
        return Err(CacheApiError::cursor_invalid("cache cursor is invalid"));
    }
    Ok(cursor)
}

fn decode_generation_cursor(
    secret: &str,
    token: &str,
) -> Result<CacheGenerationCursor, CacheApiError> {
    let cursor: CacheGenerationCursor = decode_signed_cursor(secret, token)?;
    if cursor.v != CURSOR_VERSION {
        return Err(CacheApiError::cursor_invalid("cache cursor is invalid"));
    }
    Ok(cursor)
}

// ---------------------------------------------------------------------------
// Namespace name normalization
// ---------------------------------------------------------------------------

/// Normalizes a caller-supplied namespace to the documented lowercase ASCII
/// format `^[a-z0-9][a-z0-9._-]{0,127}$` at the API boundary.
fn normalize_namespace(raw: &str) -> CacheResult<String> {
    let normalized = raw.trim().to_ascii_lowercase();
    let mut bytes = normalized.bytes();
    let valid_first =
        matches!(bytes.next(), Some(b) if b.is_ascii_lowercase() || b.is_ascii_digit());
    let valid_rest = normalized
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'));
    if normalized.is_empty() || normalized.len() > 128 || !valid_first || !valid_rest {
        return Err(CacheApiError::bad_request(
            "cache namespace must match ^[a-z0-9][a-z0-9._-]{0,127}$",
        ));
    }
    Ok(normalized)
}

fn normalize_namespace_filter(raw: Option<&str>) -> CacheResult<Option<String>> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let normalized = raw.to_ascii_lowercase();
    if normalized.len() > 128
        || !normalized.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
    {
        return Err(CacheApiError::bad_request(
            "cache namespace filter may contain only lowercase letters, digits, '.', '_', and '-'",
        ));
    }
    Ok(Some(normalized))
}

fn repository_freshness(
    freshness: Option<CacheNamespaceFreshness>,
) -> Option<CacheNamespaceFreshnessFilter> {
    freshness.map(|freshness| match freshness {
        CacheNamespaceFreshness::Fresh => CacheNamespaceFreshnessFilter::Fresh,
        CacheNamespaceFreshness::Stale => CacheNamespaceFreshnessFilter::Stale,
        CacheNamespaceFreshness::Unpopulated => CacheNamespaceFreshnessFilter::Unpopulated,
    })
}

// ---------------------------------------------------------------------------
// Authorization
// ---------------------------------------------------------------------------

/// The effective cache authority for one request, gated by token type.
struct CacheAuthority {
    identity_id: i64,
    grants: Vec<Grant>,
    snapshot: Option<AuthorizationSnapshot>,
    authz: AuthorizationService,
}

impl CacheAuthority {
    fn authorize(
        &self,
        user: &AuthenticatedUser,
        action: Action,
        context: AuthorizationContext,
    ) -> CacheResult<()> {
        if let Some(snapshot) = self.snapshot.as_ref() {
            self.authz
                .authorize_with_snapshot(
                    user,
                    Some(snapshot),
                    AuthorizationCheck {
                        resource: Resource::Caches,
                        action,
                        context,
                    },
                )
                .map_err(CacheApiError::from)
        } else if cache_action_allowed(&self.grants, action, &context) {
            Ok(())
        } else {
            Err(CacheApiError::forbidden(format!(
                "Insufficient permissions: caches:{}",
                action_word(action)
            )))
        }
    }
}

/// Loads the effective cache grants for the current token, failing closed for
/// any token type that is not subject to cache authorization.
async fn load_cache_authority(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
) -> CacheResult<CacheAuthority> {
    let authz = state.authorization_service();
    match user.claims.token_type {
        TokenType::Access | TokenType::Execution => {
            let snapshot = authz.load_snapshot(user).await?.ok_or_else(|| {
                CacheApiError::Api(ApiError::Unauthorized(
                    "Invalid authentication subject in token".to_string(),
                ))
            })?;
            Ok(CacheAuthority {
                identity_id: snapshot.identity_id,
                grants: snapshot.grants.clone(),
                snapshot: Some(snapshot),
                authz,
            })
        }
        // Sensor tokens are not evaluated by identity RBAC. They may only use a
        // cache authority explicitly signed into the token; absent that, they
        // have no cache access.
        TokenType::Sensor => {
            let identity_id = user.identity_id().map_err(|_| {
                CacheApiError::Api(ApiError::Unauthorized("Invalid user identity".into()))
            })?;
            Ok(CacheAuthority {
                identity_id,
                grants: sensor_cache_grants(user),
                snapshot: None,
                authz,
            })
        }
        // Refresh and worker tokens are rejected from cache data routes.
        TokenType::Refresh | TokenType::Worker => Err(CacheApiError::forbidden(
            "cache access is not available for this token type",
        )),
    }
}

/// Reads a sensor token's signed cache authority. Only `Resource::Caches`
/// grants are honored; anything else is ignored. Returns empty (no access)
/// when the token carries no signed cache authority.
fn sensor_cache_grants(user: &AuthenticatedUser) -> Vec<Grant> {
    user.claims
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("cache_grants"))
        .and_then(|value| serde_json::from_value::<Vec<Grant>>(value.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|grant| grant.resource == Resource::Caches)
        .collect()
}

/// Builds the authorization context for a cache request from request-supplied
/// scope values (never from a database lookup) so authorization precedes any
/// existence check.
fn cache_context(
    identity_id: i64,
    owner_type: OwnerType,
    owner_ref: Option<&str>,
    owner_identity: Option<Id>,
    namespace: Option<&str>,
) -> AuthorizationContext {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.owner_type = Some(owner_type);
    ctx.owner_ref = owner_ref.map(ToOwned::to_owned);
    ctx.owner_identity_id = owner_identity;
    ctx.target_ref = namespace.map(ToOwned::to_owned);
    ctx
}

fn cache_action_allowed(grants: &[Grant], action: Action, ctx: &AuthorizationContext) -> bool {
    AuthorizationService::is_allowed(grants, Resource::Caches, action, ctx)
}

/// Authorizes a namespace-scoped cache action against the request scope. The
/// caller's own identity is used for the `identity` owner scope. Returns the
/// canonical owner scope only after authorization succeeds.
async fn authorize_namespace_action(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    action: Action,
    owner_type: OwnerType,
    owner_ref: Option<&str>,
    namespace: &str,
) -> CacheResult<(CacheAuthority, CacheOwnerScope)> {
    let authority = load_cache_authority(state, user).await?;
    let owner_ref = normalize_owner_selector(owner_type, owner_ref)?;
    let owner_identity = match owner_type {
        OwnerType::Identity => Some(authority.identity_id),
        _ => None,
    };
    let ctx = cache_context(
        authority.identity_id,
        owner_type,
        owner_ref.as_deref(),
        owner_identity,
        Some(namespace),
    );
    authority.authorize(user, action, ctx)?;

    let scope = resolve_owner_scope(
        &state.db,
        owner_type,
        owner_ref.as_deref(),
        authority.identity_id,
    )
    .await?;
    Ok((authority, scope))
}

fn normalize_owner_selector(
    owner_type: OwnerType,
    owner_ref: Option<&str>,
) -> CacheResult<Option<String>> {
    let owner_ref = owner_ref.map(str::trim).filter(|value| !value.is_empty());
    match owner_type {
        OwnerType::System | OwnerType::Identity => {
            if owner_ref.is_some() {
                return Err(CacheApiError::bad_request(
                    "owner_ref must be omitted for system and identity cache scopes",
                ));
            }
            Ok(None)
        }
        OwnerType::Pack => Ok(Some(require_owner_ref(owner_ref, "pack")?)),
        OwnerType::Action => Ok(Some(require_owner_ref(owner_ref, "action")?)),
        OwnerType::Sensor => Ok(Some(require_owner_ref(owner_ref, "sensor")?)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CacheNamespaceListScope {
    Any,
    Owner {
        owner_type: OwnerType,
        owner_ref: Option<String>,
    },
}

impl CacheNamespaceListScope {
    fn owner_type(&self) -> Option<OwnerType> {
        match self {
            Self::Any => None,
            Self::Owner { owner_type, .. } => Some(*owner_type),
        }
    }

    fn owner_ref(&self) -> Option<&str> {
        match self {
            Self::Any => None,
            Self::Owner { owner_ref, .. } => owner_ref.as_deref(),
        }
    }
}

fn normalize_namespace_list_scope(
    owner_type: Option<OwnerType>,
    owner_ref: Option<&str>,
) -> CacheResult<CacheNamespaceListScope> {
    match owner_type {
        Some(owner_type) => Ok(CacheNamespaceListScope::Owner {
            owner_type,
            owner_ref: normalize_owner_selector(owner_type, owner_ref)?,
        }),
        None if owner_ref.map(str::trim).is_none_or(str::is_empty) => {
            Ok(CacheNamespaceListScope::Any)
        }
        None => Err(CacheApiError::bad_request("owner_ref requires owner_type")),
    }
}

enum CacheNamespaceVisibility {
    NoAccess,
    All,
    Refs(Vec<String>),
}

/// Compiles the namespace-ref portion of cache read grants for one explicit
/// owner selector. The owner and all other constraints are evaluated through
/// the same `Grant::allows` implementation used by point reads; the resulting
/// exact namespace list is then applied by the repository query.
fn cache_namespace_visibility(
    authority: &CacheAuthority,
    owner_type: OwnerType,
    owner_ref: Option<&str>,
) -> CacheNamespaceVisibility {
    let identity_id = authority.identity_id;
    let owner_identity = (owner_type == OwnerType::Identity).then_some(identity_id);
    let mut refs = std::collections::BTreeSet::new();

    for grant in authority
        .grants
        .iter()
        .filter(|grant| grant.resource == Resource::Caches)
        .filter(|grant| grant.actions.contains(&Action::Read))
    {
        match grant
            .constraints
            .as_ref()
            .and_then(|constraints| constraints.refs.as_ref())
        {
            Some(grant_refs) => {
                for namespace in grant_refs {
                    let mut ctx = cache_context(
                        identity_id,
                        owner_type,
                        owner_ref,
                        owner_identity,
                        Some(namespace),
                    );
                    if let Some(snapshot) = authority.snapshot.as_ref() {
                        ctx.identity_attributes = snapshot.identity_attributes.clone();
                    }
                    if grant.allows(Resource::Caches, Action::Read, &ctx) {
                        refs.insert(namespace.clone());
                    }
                }
            }
            None => {
                let mut ctx = cache_context(
                    identity_id,
                    owner_type,
                    owner_ref,
                    owner_identity,
                    Some("__cache_namespace_visibility_probe__"),
                );
                if let Some(snapshot) = authority.snapshot.as_ref() {
                    ctx.identity_attributes = snapshot.identity_attributes.clone();
                }
                if grant.allows(Resource::Caches, Action::Read, &ctx) {
                    return CacheNamespaceVisibility::All;
                }
            }
        }
    }

    if refs.is_empty() {
        CacheNamespaceVisibility::NoAccess
    } else {
        CacheNamespaceVisibility::Refs(refs.into_iter().collect())
    }
}

fn compile_cache_namespace_read_visibility(
    authority: &CacheAuthority,
) -> CacheNamespaceReadVisibility {
    let identity_attributes = authority
        .snapshot
        .as_ref()
        .map(|snapshot| &snapshot.identity_attributes);
    let grants = authority
        .grants
        .iter()
        .filter_map(|grant| compile_cache_namespace_grant_filter(grant, identity_attributes))
        .collect();

    CacheNamespaceReadVisibility {
        identity_id: authority.identity_id,
        grants,
    }
}

fn compile_cache_namespace_grant_filter(
    grant: &Grant,
    identity_attributes: Option<&HashMap<String, serde_json::Value>>,
) -> Option<CacheNamespaceGrantFilter> {
    if grant.resource != Resource::Caches || !grant.actions.contains(&Action::Read) {
        return None;
    }

    let Some(constraints) = &grant.constraints else {
        return Some(CacheNamespaceGrantFilter::default());
    };

    if constraints.attributes.as_ref().is_some_and(|expected| {
        !expected.iter().all(|(key, value)| {
            identity_attributes.and_then(|actual| actual.get(key)) == Some(value)
        })
    }) {
        return None;
    }

    if constraints.pack_refs.is_some()
        || constraints.ids.is_some()
        || constraints.visibility.is_some()
        || constraints.encrypted.is_some()
        || matches!(
            constraints.execution_scope,
            Some(ExecutionScopeConstraint::SelfOnly | ExecutionScopeConstraint::Descendants)
        )
    {
        return None;
    }

    Some(CacheNamespaceGrantFilter {
        owner: constraints.owner,
        owner_types: constraints.owner_types.clone(),
        owner_refs: constraints.owner_refs.clone(),
        namespace_refs: constraints.refs.clone(),
    })
}

fn action_word(action: Action) -> &'static str {
    match action {
        Action::Read => "read",
        Action::Create => "create",
        Action::Update => "update",
        Action::Delete => "delete",
        _ => "access",
    }
}

/// Resolves an API owner selector to canonical owner IDs via the existing
/// repositories. Identity scope resolves to the authenticated identity.
async fn resolve_owner_scope(
    db: &sqlx::PgPool,
    owner_type: OwnerType,
    owner_ref: Option<&str>,
    identity_id: i64,
) -> CacheResult<CacheOwnerScope> {
    match owner_type {
        OwnerType::System => Ok(CacheOwnerScope::system()),
        OwnerType::Identity => Ok(CacheOwnerScope::identity(identity_id)),
        OwnerType::Pack => {
            let reference = require_owner_ref(owner_ref, "pack")?;
            let pack = PackRepository::find_by_ref(db, &reference)
                .await?
                .ok_or_else(|| CacheApiError::not_found(format!("Pack '{reference}' not found")))?;
            Ok(CacheOwnerScope::pack(pack.id, Some(reference)))
        }
        OwnerType::Action => {
            let reference = require_owner_ref(owner_ref, "action")?;
            let action = ActionRepository::find_by_ref(db, &reference)
                .await?
                .ok_or_else(|| {
                    CacheApiError::not_found(format!("Action '{reference}' not found"))
                })?;
            Ok(CacheOwnerScope::action(action.id, Some(reference)))
        }
        OwnerType::Sensor => {
            let reference = require_owner_ref(owner_ref, "sensor")?;
            let sensor = SensorRepository::find_by_ref(db, &reference)
                .await?
                .ok_or_else(|| {
                    CacheApiError::not_found(format!("Sensor '{reference}' not found"))
                })?;
            Ok(CacheOwnerScope::sensor(sensor.id, Some(reference)))
        }
    }
}

fn require_owner_ref(owner_ref: Option<&str>, owner_type: &str) -> CacheResult<String> {
    let reference = owner_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CacheApiError::bad_request(format!("owner_ref is required for {owner_type} scope"))
        })?;
    Ok(reference.to_string())
}

// ---------------------------------------------------------------------------
// Response builders
// ---------------------------------------------------------------------------

fn namespace_owner_ref(namespace: &CacheNamespace) -> Option<String> {
    match namespace.owner_type {
        OwnerType::Pack => namespace.owner_pack_ref.clone(),
        OwnerType::Action => namespace.owner_action_ref.clone(),
        OwnerType::Sensor => namespace.owner_sensor_ref.clone(),
        _ => None,
    }
}

fn scope_owner_ref(scope: &CacheOwnerScope) -> Option<&str> {
    match scope.owner_type {
        OwnerType::Pack => scope.owner_pack_ref.as_deref(),
        OwnerType::Action => scope.owner_action_ref.as_deref(),
        OwnerType::Sensor => scope.owner_sensor_ref.as_deref(),
        OwnerType::System | OwnerType::Identity => None,
    }
}

fn generation_response(generation: &CacheGeneration) -> CacheGenerationResponse {
    CacheGenerationResponse {
        generation_id: generation.id,
        namespace_id: generation.namespace,
        status: generation.state,
        client_refresh_id: generation.client_refresh_id.clone(),
        expected_active_generation_id: generation.expected_active_generation,
        expected_chunk_count: generation.expected_chunk_count,
        expected_record_count: generation.expected_count,
        expected_size_bytes: generation.expected_bytes,
        record_count: generation.record_count,
        size_bytes: generation.size_bytes,
        checksum_algorithm: generation.checksum_algorithm.clone(),
        checksum: generation.checksum.clone(),
        source_revision: generation.source_revision.clone(),
        created_by: generation.created_by,
        created: generation.created,
        sealed: generation.sealed,
        activated: generation.activated,
        retired: generation.retired,
        readable_until: generation.readable_until,
        failed: generation.failed,
        failure_reason: generation.failure_reason.clone(),
    }
}

/// True when a generation is not the current authoritative snapshot or its
/// active age exceeds the namespace freshness target.
fn compute_stale(
    namespace: &CacheNamespace,
    generation: &CacheGeneration,
    now: DateTime<Utc>,
) -> bool {
    if generation.state != CacheGenerationState::Active {
        return true;
    }
    if namespace.freshness_target_seconds <= 0 {
        return false;
    }
    match generation.activated {
        Some(activated) => (now - activated).num_seconds() > namespace.freshness_target_seconds,
        None => false,
    }
}

fn namespace_response(
    namespace: &CacheNamespace,
    canonical_owner_ref: Option<&str>,
    active: Option<&CacheGeneration>,
) -> CacheNamespaceResponse {
    let now = Utc::now();
    let (stale, record_count, size_bytes, source_revision, last_refreshed_at) = match active {
        Some(generation) => (
            compute_stale(namespace, generation, now),
            Some(generation.record_count),
            Some(generation.size_bytes),
            generation.source_revision.clone(),
            generation.activated,
        ),
        None => (false, None, None, None, None),
    };

    CacheNamespaceResponse {
        id: namespace.id,
        owner_type: namespace.owner_type,
        owner: namespace.owner.clone(),
        owner_ref: canonical_owner_ref
            .map(ToOwned::to_owned)
            .or_else(|| namespace_owner_ref(namespace)),
        managed: namespace.definition_ref.is_some(),
        definition_ref: namespace.definition_ref.clone(),
        managing_pack_ref: namespace.managing_pack_ref.clone(),
        namespace: namespace.namespace.clone(),
        active_generation: namespace.active_generation,
        freshness_target_seconds: namespace.freshness_target_seconds,
        max_records_per_generation: namespace.max_records_per_generation,
        max_generation_bytes: namespace.max_generation_bytes,
        max_retained_bytes: namespace.max_retained_bytes,
        max_retained_generations: namespace.max_retained_generations,
        max_staging_generations: namespace.max_staging_generations,
        tombstoned: namespace.tombstoned_at.is_some(),
        created: namespace.created,
        updated: namespace.updated,
        cache_not_populated: namespace.active_generation.is_none(),
        stale,
        record_count,
        size_bytes,
        source_revision,
        last_refreshed_at,
    }
}

async fn build_namespace_response(
    db: &sqlx::PgPool,
    namespace: &CacheNamespace,
    canonical_owner_ref: Option<&str>,
) -> CacheResult<CacheNamespaceResponse> {
    let active = match namespace.active_generation {
        Some(id) => CacheGenerationRepository::find_by_id(db, id).await?,
        None => None,
    };
    Ok(namespace_response(
        namespace,
        canonical_owner_ref,
        active.as_ref(),
    ))
}

async fn build_namespace_responses(
    db: &sqlx::PgPool,
    namespaces: &[CacheNamespace],
    canonical_owner_ref: Option<&str>,
) -> CacheResult<Vec<CacheNamespaceResponse>> {
    let generation_ids: Vec<Id> = namespaces
        .iter()
        .filter_map(|namespace| namespace.active_generation)
        .collect();
    let generations = CacheGenerationRepository::find_by_ids(db, &generation_ids).await?;
    let generations_by_id: HashMap<Id, CacheGeneration> = generations
        .into_iter()
        .map(|generation| (generation.id, generation))
        .collect();
    Ok(namespaces
        .iter()
        .map(|namespace| {
            let active = namespace
                .active_generation
                .and_then(|id| generations_by_id.get(&id));
            namespace_response(namespace, canonical_owner_ref, active)
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn emit_cache_audit(
    state: &Arc<AppState>,
    user: &RequireAuth,
    event_type: &'static str,
    outcome: AuditOutcome,
    resource_type: &'static str,
    resource_id: Option<Id>,
    resource_ref: Option<String>,
    details: serde_json::Value,
) {
    state.audit_emitter.emit(build_cache_audit_event(
        user,
        event_type,
        outcome,
        resource_type,
        resource_id,
        resource_ref,
        details,
    ));
}

#[allow(clippy::too_many_arguments)]
fn build_cache_audit_event(
    user: &RequireAuth,
    event_type: &'static str,
    outcome: AuditOutcome,
    resource_type: &'static str,
    resource_id: Option<Id>,
    resource_ref: Option<String>,
    details: serde_json::Value,
) -> PendingAuditEvent {
    let mut builder =
        AuditEventBuilder::new(AuditCategory::Admin, event_type, outcome).resource(resource_type);
    if let Some(id) = resource_id {
        builder = builder.resource_id(id);
    }
    if let Some(reference) = resource_ref {
        builder = builder.resource_ref(reference);
    }
    builder = builder.with_details(details);
    if let Ok(identity_id) = user.0.identity_id() {
        builder = builder.actor_identity(identity_id);
    }
    builder
        .actor_login(user.0.login().to_string())
        .actor_token_type(format!("{:?}", user.0.claims.token_type).to_lowercase())
        .build()
}

fn policy_from_body(
    base: CacheNamespacePolicy,
    body: &CacheNamespacePolicyBody,
) -> CacheNamespacePolicy {
    CacheNamespacePolicy {
        freshness_target_seconds: body
            .freshness_target_seconds
            .unwrap_or(base.freshness_target_seconds),
        max_records_per_generation: body
            .max_records_per_generation
            .unwrap_or(base.max_records_per_generation),
        max_generation_bytes: body
            .max_generation_bytes
            .unwrap_or(base.max_generation_bytes),
        max_retained_bytes: body.max_retained_bytes.unwrap_or(base.max_retained_bytes),
        max_retained_generations: body
            .max_retained_generations
            .unwrap_or(base.max_retained_generations),
        max_staging_generations: body
            .max_staging_generations
            .unwrap_or(base.max_staging_generations),
    }
}

fn validate_policy_body(body: &CacheNamespacePolicyBody) -> CacheResult<()> {
    if body
        .max_retained_generations
        .is_some_and(|generations| generations < 2)
    {
        return Err(CacheApiError::bad_request(
            "max_retained_generations must be at least 2",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Namespace routes
// ---------------------------------------------------------------------------

/// List cache namespaces visible to the caller, optionally within one owner scope.
#[utoipa::path(
    get,
    path = "/api/v1/cache/namespaces",
    operation_id = "list_namespaces",
    tag = "caches",
    params(CacheNamespaceListQuery),
    responses(
        (status = 200, description = "Namespaces visible to the caller", body = CacheNamespaceListApiResponse),
        (status = 400, description = "Invalid owner selector, filter, limit, or cursor", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Cache scope is not accessible", body = CacheForbiddenResponse),
        (status = 500, description = "Cache metadata lookup failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_namespaces(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<CacheNamespaceListQuery>,
) -> CacheResult<Response> {
    let requested_page_size = validate_metadata_page_size(query.limit)?;
    let authority = load_cache_authority(&state, &user.0).await?;
    let scope = normalize_namespace_list_scope(query.owner_type, query.owner_ref.as_deref())?;
    let requested_owner_type = scope.owner_type();
    let owner_ref = scope.owner_ref().map(ToOwned::to_owned);
    let requested_namespace_filter = normalize_namespace_filter(query.namespace.as_deref())?;
    let (namespace_filter, freshness, page_size, after_id) =
        if let Some(cursor_token) = query.cursor.as_deref() {
            let cursor = decode_namespace_cursor(&state.jwt_config.secret, cursor_token)?;
            if cursor.owner_type != requested_owner_type || cursor.owner_ref != owner_ref {
                return Err(CacheApiError::cursor_invalid("cursor owner scope mismatch"));
            }
            if requested_namespace_filter.is_some()
                && requested_namespace_filter != cursor.namespace_filter
            {
                return Err(CacheApiError::cursor_invalid(
                    "cursor namespace filter mismatch",
                ));
            }
            if query.freshness.is_some() && query.freshness != cursor.freshness {
                return Err(CacheApiError::cursor_invalid(
                    "cursor freshness filter mismatch",
                ));
            }
            if query
                .limit
                .is_some_and(|_| requested_page_size != cursor.page_size)
            {
                return Err(CacheApiError::cursor_invalid("cursor page shape mismatch"));
            }
            (
                cursor.namespace_filter,
                cursor.freshness,
                cursor.page_size,
                Some(cursor.after_id),
            )
        } else {
            (
                requested_namespace_filter,
                query.freshness,
                requested_page_size,
                None,
            )
        };
    let page = match scope {
        CacheNamespaceListScope::Any => {
            let visibility = compile_cache_namespace_read_visibility(&authority);
            CacheNamespaceRepository::list_metadata_all_owners_visible_filtered_page(
                &state.db,
                &visibility,
                after_id,
                namespace_filter.as_deref(),
                repository_freshness(freshness),
                page_size,
            )
            .await?
        }
        CacheNamespaceListScope::Owner {
            owner_type,
            owner_ref: scoped_owner_ref,
        } => {
            let visibility =
                cache_namespace_visibility(&authority, owner_type, scoped_owner_ref.as_deref());
            if matches!(visibility, CacheNamespaceVisibility::NoAccess) {
                return Ok((
                    StatusCode::OK,
                    Json(ApiResponse::new(CacheNamespaceListResponse {
                        namespaces: Vec::new(),
                        next_cursor: None,
                    })),
                )
                    .into_response());
            }

            let owner_scope = resolve_owner_scope(
                &state.db,
                owner_type,
                scoped_owner_ref.as_deref(),
                authority.identity_id,
            )
            .await?;
            let visible_refs = match &visibility {
                CacheNamespaceVisibility::Refs(refs) => Some(refs.as_slice()),
                CacheNamespaceVisibility::All => None,
                CacheNamespaceVisibility::NoAccess => unreachable!(),
            };
            CacheNamespaceRepository::list_metadata_visible_filtered_page(
                &state.db,
                &owner_scope,
                visible_refs,
                after_id,
                namespace_filter.as_deref(),
                repository_freshness(freshness),
                page_size,
            )
            .await?
        }
    };
    let items = build_namespace_responses(&state.db, &page.items, owner_ref.as_deref()).await?;
    let next_cursor = page
        .next_after_id
        .map(|after_id| {
            encode_signed_cursor(
                &state.jwt_config.secret,
                &CacheNamespaceCursor {
                    v: CURSOR_VERSION,
                    owner_type: requested_owner_type,
                    owner_ref: owner_ref.clone(),
                    namespace_filter,
                    freshness,
                    page_size,
                    after_id,
                },
            )
        })
        .transpose()?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(CacheNamespaceListResponse {
            namespaces: items,
            next_cursor,
        })),
    )
        .into_response())
}

/// Create a cache namespace.
#[utoipa::path(
    post,
    path = "/api/v1/cache/namespaces",
    operation_id = "create_namespace",
    tag = "caches",
    request_body = CreateCacheNamespaceRequest,
    responses(
        (status = 201, description = "Namespace created", body = CacheNamespaceApiResponse),
        (status = 400, description = "Invalid owner selector, namespace, or policy", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Namespace creation is not permitted", body = CacheForbiddenResponse),
        (status = 409, description = "Namespace already exists", body = ErrorResponse),
        (status = 500, description = "Namespace creation failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_namespace(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateCacheNamespaceRequest>,
) -> CacheResult<Response> {
    let namespace = normalize_namespace(&request.namespace)?;
    validate_policy_body(&request.policy)?;
    let (_authority, scope) = authorize_namespace_action(
        &state,
        &user.0,
        Action::Create,
        request.owner_type,
        request.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let policy = policy_from_body(CacheNamespacePolicy::default(), &request.policy);
    let canonical_owner_ref = scope_owner_ref(&scope).map(ToOwned::to_owned);
    let created = CacheNamespaceRepository::create_api_with_policy(
        &state.db,
        CreateCacheNamespaceInput {
            owner: scope,
            namespace: namespace.clone(),
            policy,
        },
        &state.config.cache_admission,
    )
    .await
    .map_err(map_write_error)?;

    emit_cache_audit(
        &state,
        &user,
        cache_event::NAMESPACE_CREATED,
        AuditOutcome::Success,
        "cache_namespace",
        Some(created.id),
        Some(created.namespace.clone()),
        serde_json::json!({
            "owner_type": created.owner_type,
            "owner_ref": namespace_owner_ref(&created),
            "namespace": created.namespace,
        }),
    );

    let response =
        build_namespace_response(&state.db, &created, canonical_owner_ref.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(ApiResponse::new(response))).into_response())
}

/// Show cache namespace metadata and health.
#[utoipa::path(
    get,
    path = "/api/v1/cache/namespaces/{namespace}",
    operation_id = "show_namespace",
    tag = "caches",
    params(("namespace" = String, Path, description = "Cache namespace"), CacheOwnerQuery),
    responses(
        (status = 200, description = "Namespace metadata", body = CacheNamespaceApiResponse),
        (status = 400, description = "Invalid owner selector or namespace", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Namespace is not accessible", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace not found", body = ErrorResponse),
        (status = 500, description = "Namespace lookup failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn show_namespace(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Query(query): Query<CacheOwnerQuery>,
) -> CacheResult<Response> {
    let namespace = normalize_namespace(&namespace)?;
    let (_authority, scope) = authorize_namespace_action(
        &state,
        &user.0,
        Action::Read,
        query.owner_type,
        query.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let record = CacheNamespaceRepository::resolve(&state.db, &scope, &namespace)
        .await?
        .ok_or_else(|| CacheApiError::not_found("cache namespace not found"))?;
    let response = build_namespace_response(&state.db, &record, scope_owner_ref(&scope)).await?;
    Ok((StatusCode::OK, Json(ApiResponse::new(response))).into_response())
}

/// Update a cache namespace's publication policy.
#[utoipa::path(
    put,
    path = "/api/v1/cache/namespaces/{namespace}",
    operation_id = "update_namespace",
    tag = "caches",
    request_body = UpdateCacheNamespaceRequest,
    params(("namespace" = String, Path, description = "Cache namespace")),
    responses(
        (status = 200, description = "Namespace updated", body = CacheNamespaceApiResponse),
        (status = 400, description = "Invalid owner selector, namespace, or policy", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Namespace update is not permitted", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace not found", body = ErrorResponse),
        (status = 409, description = "Namespace is deleted or policy update conflicts", body = ErrorResponse),
        (status = 500, description = "Namespace update failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_namespace(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Json(request): Json<UpdateCacheNamespaceRequest>,
) -> CacheResult<Response> {
    let namespace = normalize_namespace(&namespace)?;
    let (_authority, scope) = authorize_namespace_action(
        &state,
        &user.0,
        Action::Update,
        request.owner_type,
        request.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let record = resolve_namespace_for_write(&state.db, &scope, &namespace).await?;
    if record.definition_ref.is_some() {
        return Err(CacheApiError::pack_managed_namespace("updated"));
    }
    validate_policy_body(&request.policy)?;

    let base = CacheNamespacePolicy {
        freshness_target_seconds: record.freshness_target_seconds,
        max_records_per_generation: record.max_records_per_generation,
        max_generation_bytes: record.max_generation_bytes,
        max_retained_bytes: record.max_retained_bytes,
        max_retained_generations: record.max_retained_generations,
        max_staging_generations: record.max_staging_generations,
    };
    let policy = policy_from_body(base, &request.policy);
    let updated = CacheNamespaceRepository::update_policy(&state.db, record.id, &policy)
        .await
        .map_err(map_write_error)?;

    emit_cache_audit(
        &state,
        &user,
        cache_event::NAMESPACE_UPDATED,
        AuditOutcome::Success,
        "cache_namespace",
        Some(updated.id),
        Some(updated.namespace.clone()),
        serde_json::json!({
            "owner_type": updated.owner_type,
            "owner_ref": namespace_owner_ref(&updated),
        }),
    );

    let response = build_namespace_response(&state.db, &updated, scope_owner_ref(&scope)).await?;
    Ok((StatusCode::OK, Json(ApiResponse::new(response))).into_response())
}

/// Tombstone a cache namespace and queue bounded cleanup.
#[utoipa::path(
    delete,
    path = "/api/v1/cache/namespaces/{namespace}",
    operation_id = "delete_namespace",
    tag = "caches",
    params(("namespace" = String, Path, description = "Cache namespace"), CacheOwnerQuery),
    responses(
        (status = 200, description = "Namespace tombstoned", body = CacheNamespaceDeletionApiResponse),
        (status = 400, description = "Invalid owner selector or namespace", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Namespace deletion is not permitted", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace not found", body = ErrorResponse),
        (status = 409, description = "Namespace deletion conflicts with current state", body = ErrorResponse),
        (status = 500, description = "Namespace deletion failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_namespace(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Query(query): Query<CacheOwnerQuery>,
) -> CacheResult<Response> {
    let namespace = normalize_namespace(&namespace)?;
    let (_authority, scope) = authorize_namespace_action(
        &state,
        &user.0,
        Action::Delete,
        query.owner_type,
        query.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let record = CacheNamespaceRepository::resolve(&state.db, &scope, &namespace)
        .await?
        .ok_or_else(|| CacheApiError::not_found("cache namespace not found"))?;
    if record.definition_ref.is_some() {
        return Err(CacheApiError::pack_managed_namespace("deleted"));
    }

    CacheNamespaceRepository::tombstone(&state.db, record.id)
        .await
        .map_err(map_write_error)?;

    emit_cache_audit(
        &state,
        &user,
        cache_event::NAMESPACE_TOMBSTONED,
        AuditOutcome::Success,
        "cache_namespace",
        Some(record.id),
        Some(record.namespace.clone()),
        serde_json::json!({
            "owner_type": record.owner_type,
            "owner_ref": namespace_owner_ref(&record),
        }),
    );

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(CacheNamespaceDeletionResponse {
            id: record.id,
            namespace: record.namespace,
            tombstoned: true,
            cleanup_pending: true,
            status: "tombstoned".to_string(),
        })),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Generation metadata routes
// ---------------------------------------------------------------------------

/// List generations for a namespace.
#[utoipa::path(
    get,
    path = "/api/v1/cache/namespaces/{namespace}/generations",
    operation_id = "list_generations",
    tag = "caches",
    params(("namespace" = String, Path, description = "Cache namespace"), CacheGenerationListQuery),
    responses(
        (status = 200, description = "Generations", body = CacheGenerationListApiResponse),
        (status = 400, description = "Invalid owner selector, limit, or cursor", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Namespace is not accessible", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace not found", body = ErrorResponse),
        (status = 500, description = "Generation metadata lookup failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_generations(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Query(query): Query<CacheGenerationListQuery>,
) -> CacheResult<Response> {
    let requested_page_size = validate_metadata_page_size(query.limit)?;
    let record = resolve_namespace_for_read(
        &state,
        &user.0,
        query.owner_type,
        query.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let (page_size, before) = if let Some(cursor_token) = query.cursor.as_deref() {
        let cursor = decode_generation_cursor(&state.jwt_config.secret, cursor_token)?;
        if cursor.namespace_id != record.id {
            return Err(CacheApiError::cursor_invalid("cursor namespace mismatch"));
        }
        if query
            .limit
            .is_some_and(|_| requested_page_size != cursor.page_size)
        {
            return Err(CacheApiError::cursor_invalid("cursor page shape mismatch"));
        }
        (
            cursor.page_size,
            Some((cursor.before_created, cursor.before_id)),
        )
    } else {
        (requested_page_size, None)
    };
    let page =
        CacheGenerationRepository::list_for_namespace_page(&state.db, record.id, before, page_size)
            .await?;
    let generations = page.items.iter().map(generation_response).collect();
    let next_cursor = page
        .next_before
        .map(|(before_created, before_id)| {
            encode_signed_cursor(
                &state.jwt_config.secret,
                &CacheGenerationCursor {
                    v: CURSOR_VERSION,
                    namespace_id: record.id,
                    page_size,
                    before_created,
                    before_id,
                },
            )
        })
        .transpose()?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(CacheGenerationListResponse {
            generations,
            next_cursor,
        })),
    )
        .into_response())
}

/// Show a single generation.
#[utoipa::path(
    get,
    path = "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}",
    operation_id = "show_generation",
    tag = "caches",
    params(
        ("namespace" = String, Path, description = "Cache namespace"),
        ("generation_id" = i64, Path, description = "Generation id"),
        CacheOwnerQuery
    ),
    responses(
        (status = 200, description = "Generation", body = CacheGenerationApiResponse),
        (status = 400, description = "Invalid owner selector, namespace, or generation id", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Namespace is not accessible", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace or generation not found", body = ErrorResponse),
        (status = 500, description = "Generation lookup failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn show_generation(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((namespace, generation_id)): Path<(String, i64)>,
    Query(query): Query<CacheOwnerQuery>,
) -> CacheResult<Response> {
    let record = resolve_namespace_for_read(
        &state,
        &user.0,
        query.owner_type,
        query.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let generation = CacheGenerationRepository::find_by_id(&state.db, generation_id)
        .await?
        .filter(|generation| generation.namespace == record.id)
        .ok_or_else(|| CacheApiError::not_found("cache generation not found"))?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(generation_response(&generation))),
    )
        .into_response())
}

/// Authorizes read access and resolves the namespace, returning 404 (never a
/// distinct forbidden shape) once authorization has already passed.
async fn resolve_namespace_for_read(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    owner_type: OwnerType,
    owner_ref: Option<&str>,
    namespace: &str,
) -> CacheResult<CacheNamespace> {
    let namespace = normalize_namespace(namespace)?;
    let (_authority, scope) =
        authorize_namespace_action(state, user, Action::Read, owner_type, owner_ref, &namespace)
            .await?;
    CacheNamespaceRepository::resolve(&state.db, &scope, &namespace)
        .await?
        .ok_or_else(|| CacheApiError::not_found("cache namespace not found"))
}

async fn resolve_namespace_for_write(
    db: &sqlx::PgPool,
    scope: &CacheOwnerScope,
    namespace: &str,
) -> CacheResult<CacheNamespace> {
    let record = CacheNamespaceRepository::resolve_including_tombstoned(db, scope, namespace)
        .await?
        .ok_or_else(|| CacheApiError::not_found("cache namespace not found"))?;
    if record.tombstoned_at.is_some() {
        return Err(CacheApiError::namespace_deleted());
    }
    Ok(record)
}

// ---------------------------------------------------------------------------
// Read routes
// ---------------------------------------------------------------------------

/// Point lookup by external id.
#[utoipa::path(
    post,
    path = "/api/v1/cache/namespaces/{namespace}/entries/lookup",
    operation_id = "lookup_entry",
    tag = "caches",
    request_body = CachePointLookupRequest,
    params(("namespace" = String, Path, description = "Cache namespace")),
    responses(
        (status = 200, description = "Lookup result", body = CachePointLookupApiResponse),
        (status = 400, description = "Invalid owner selector, namespace, or lookup request", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Namespace is not accessible", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace not found", body = ErrorResponse),
        (status = 409, description = "Cache is stale, unpopulated, deleted, or the snapshot expired", body = ErrorResponse),
        (status = 500, description = "Cache lookup failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn lookup_entry(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Json(request): Json<CachePointLookupRequest>,
) -> CacheResult<Response> {
    let record = resolve_namespace_for_read(
        &state,
        &user.0,
        request.owner_type,
        request.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let generation_id = match request.generation_id {
        Some(generation_id) => generation_id,
        None => record
            .active_generation
            .ok_or_else(CacheApiError::not_populated)?,
    };
    let generation =
        CacheGenerationRepository::find_readable_pinned(&state.db, record.id, generation_id)
            .await?
            .ok_or_else(|| {
                CacheApiError::snapshot_expired("cache generation is no longer readable")
            })?;
    let stale = compute_stale(&record, &generation, Utc::now());
    if request.require_fresh && stale {
        return Err(CacheApiError::stale("active cache generation is stale"));
    }

    let entry = CacheEntryRepository::find_pinned(
        &state.db,
        record.id,
        generation_id,
        &request.external_id,
    )
    .await
    .map_err(map_read_error)?;

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(CachePointLookupResponse {
            generation_id,
            item: entry.map(CacheEntryResponse::from),
            stale,
        })),
    )
        .into_response())
}

/// Bounded multi-ID lookup.
#[utoipa::path(
    post,
    path = "/api/v1/cache/namespaces/{namespace}/entries/lookup-many",
    operation_id = "lookup_entries",
    tag = "caches",
    request_body = CacheMultiLookupRequest,
    params(("namespace" = String, Path, description = "Cache namespace")),
    responses(
        (status = 200, description = "Lookup results", body = CacheMultiLookupApiResponse),
        (status = 400, description = "Invalid owner selector, namespace, or identifier list", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Namespace is not accessible", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace not found", body = ErrorResponse),
        (status = 409, description = "Cache is stale, unpopulated, deleted, or the snapshot expired", body = ErrorResponse),
        (status = 500, description = "Cache lookup failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn lookup_entries(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Json(request): Json<CacheMultiLookupRequest>,
) -> CacheResult<Response> {
    if request.external_ids.is_empty() {
        return Err(CacheApiError::bad_request("external_ids cannot be empty"));
    }
    if request.external_ids.len() > MAX_MULTI_LOOKUP_IDS {
        return Err(CacheApiError::bad_request(format!(
            "external_ids exceeds the maximum of {MAX_MULTI_LOOKUP_IDS}"
        )));
    }

    let record = resolve_namespace_for_read(
        &state,
        &user.0,
        request.owner_type,
        request.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let generation_id = match request.generation_id {
        Some(generation_id) => generation_id,
        None => record
            .active_generation
            .ok_or_else(CacheApiError::not_populated)?,
    };
    let generation =
        CacheGenerationRepository::find_readable_pinned(&state.db, record.id, generation_id)
            .await?
            .ok_or_else(|| {
                CacheApiError::snapshot_expired("cache generation is no longer readable")
            })?;
    let stale = compute_stale(&record, &generation, Utc::now());
    if request.require_fresh && stale {
        return Err(CacheApiError::stale("active cache generation is stale"));
    }

    let found = CacheEntryRepository::find_pinned_many(
        &state.db,
        record.id,
        generation_id,
        &request.external_ids,
    )
    .await
    .map_err(map_read_error)?;

    let found_ids: std::collections::HashSet<&str> = found
        .iter()
        .map(|entry| entry.external_id.as_str())
        .collect();
    let mut missing = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for external_id in &request.external_ids {
        if !found_ids.contains(external_id.as_str()) && seen.insert(external_id.as_str()) {
            missing.push(external_id.clone());
        }
    }

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(CacheMultiLookupResponse {
            generation_id,
            items: found.into_iter().map(CacheEntryResponse::from).collect(),
            missing_external_ids: missing,
            stale,
        })),
    )
        .into_response())
}

fn map_read_error(err: CommonError) -> CacheApiError {
    match &err {
        // The repository signals a vanished/expired pinned snapshot explicitly
        // so a traversal is never silently truncated to an empty page.
        CommonError::CacheSnapshotExpired(message) => {
            CacheApiError::snapshot_expired(message.clone())
        }
        CommonError::Validation(message) => CacheApiError::bad_request(message.clone()),
        _ => CacheApiError::from(err),
    }
}

/// Generation-pinned cursor scan.
#[utoipa::path(
    get,
    path = "/api/v1/cache/namespaces/{namespace}/entries",
    operation_id = "scan_entries",
    tag = "caches",
    params(("namespace" = String, Path, description = "Cache namespace"), CacheScanQuery),
    responses(
        (status = 200, description = "One scan page", body = CacheScanPageApiResponse),
        (status = 400, description = "Invalid owner selector, page shape, generation, or cursor", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Namespace is not accessible", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace not found", body = ErrorResponse),
        (status = 409, description = "Cache is stale, unpopulated, deleted, or the snapshot expired", body = ErrorResponse),
        (status = 500, description = "Cache scan failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn scan_entries(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Query(query): Query<CacheScanQuery>,
) -> CacheResult<Response> {
    let requested_page_size = validate_scan_page_size(query.limit)?;
    let record = resolve_namespace_for_read(
        &state,
        &user.0,
        query.owner_type,
        query.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let secret = &state.jwt_config.secret;
    let now = Utc::now();

    // Resolve the pinned generation, keyset position, and page shape.
    let (pinned_generation, after_external_id, page_size, traversal_deadline) =
        if let Some(cursor_token) = query.cursor.as_deref() {
            let cursor = decode_cursor(secret, cursor_token)?;
            if cursor.namespace_id != record.id {
                return Err(CacheApiError::cursor_invalid("cursor namespace mismatch"));
            }
            let generation = query.generation.ok_or_else(|| {
                CacheApiError::bad_request("generation is required when continuing a cache cursor")
            })?;
            if generation != cursor.generation_id {
                return Err(CacheApiError::cursor_invalid("cursor generation mismatch"));
            }
            if let Some(limit) = query.limit {
                if limit != cursor.page_size {
                    return Err(CacheApiError::cursor_invalid("cursor page shape mismatch"));
                }
            }
            if query.require_fresh != cursor.require_fresh {
                return Err(CacheApiError::cursor_invalid("cursor page shape mismatch"));
            }
            if cursor.expires_at <= now.timestamp() {
                return Err(CacheApiError::snapshot_expired("cache cursor has expired"));
            }
            let traversal_deadline =
                DateTime::from_timestamp(cursor.expires_at, 0).ok_or_else(|| {
                    CacheApiError::cursor_invalid("cache cursor expiration is invalid")
                })?;
            (
                cursor.generation_id,
                Some(cursor.last_external_id),
                cursor.page_size,
                Some(traversal_deadline),
            )
        } else if let Some(generation) = query.generation {
            (generation, None, requested_page_size, None)
        } else {
            let active = record
                .active_generation
                .ok_or_else(CacheApiError::not_populated)?;
            (active, None, requested_page_size, None)
        };

    // The repository is the single authority on readability: on an expired,
    // removed, tombstoned, or wrong-namespace generation it returns a typed
    // snapshot-expired error instead of an empty (silently truncated) page. No
    // API readability pre-check is performed, so nothing can race the scan.
    let page = CacheEntryRepository::scan_pinned_page_with_budget(
        &state.db,
        record.id,
        pinned_generation,
        after_external_id.as_deref(),
        page_size,
        MAX_SCAN_PAGE_BYTES,
    )
    .await
    .map_err(map_read_error)?;
    let generation = page.generation;
    let rows = page.entries;
    let repository_has_more = page.has_more;

    let stale = compute_stale(&record, &generation, now);
    if query.cursor.is_none() && query.require_fresh && stale {
        return Err(CacheApiError::stale("active cache generation is stale"));
    }
    let generation_record_count = Some(generation.record_count);
    let generation_readable_until = generation.readable_until;

    // Enforce the serialized-byte budget by stopping early and continuing via
    // the next cursor.
    let mut items: Vec<CacheEntryResponse> = Vec::new();
    let mut byte_total: i64 = 0;
    let mut truncated = false;
    for row in rows {
        let item = CacheEntryResponse::from(row);
        let encoded_bytes = i64::try_from(
            serde_json::to_vec(&item)
                .map_err(|err| {
                    CacheApiError::Api(ApiError::InternalServerError(format!(
                        "failed to size cache response item: {err}"
                    )))
                })?
                .len(),
        )
        .unwrap_or(i64::MAX);
        if !items.is_empty() && byte_total.saturating_add(encoded_bytes) > MAX_SCAN_PAGE_BYTES {
            truncated = true;
            break;
        }
        byte_total = byte_total.saturating_add(encoded_bytes);
        items.push(item);
    }

    let has_more = truncated || repository_has_more;
    let traversal_window_seconds = cache_traversal_window_seconds(&state).await?;
    let expires_at = cursor_expiration(
        generation_readable_until,
        now,
        user.0.claims.exp,
        traversal_window_seconds,
        traversal_deadline,
    );
    let next_cursor = if has_more {
        match items.last() {
            Some(last) => Some(encode_cursor(
                secret,
                &CacheCursor {
                    v: CURSOR_VERSION,
                    namespace_id: record.id,
                    generation_id: pinned_generation,
                    page_size,
                    require_fresh: query.require_fresh,
                    last_external_id: last.external_id.clone(),
                    expires_at: expires_at.timestamp(),
                },
            )?),
            None => None,
        }
    } else {
        None
    };
    let cursor_expires_at = Some(expires_at);

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(CacheScanPageResponse {
            generation_id: pinned_generation,
            items,
            next_cursor,
            cursor_expires_at,
            record_count: generation_record_count,
            stale,
        })),
    )
        .into_response())
}

fn validate_scan_page_size(limit: Option<i64>) -> CacheResult<i64> {
    validate_page_size(limit, DEFAULT_SCAN_PAGE_SIZE, MAX_SCAN_PAGE_SIZE, "entry")
}

fn validate_metadata_page_size(limit: Option<i64>) -> CacheResult<i64> {
    validate_page_size(
        limit,
        DEFAULT_METADATA_PAGE_SIZE,
        MAX_METADATA_PAGE_SIZE,
        "metadata",
    )
}

fn validate_page_size(
    limit: Option<i64>,
    default: i64,
    maximum: i64,
    page_kind: &str,
) -> CacheResult<i64> {
    let limit = limit.unwrap_or(default);
    if !(1..=maximum).contains(&limit) {
        return Err(CacheApiError::bad_request(format!(
            "{page_kind} page limit must be between 1 and {maximum}"
        )));
    }
    Ok(limit)
}

/// Cursor expiration is the earliest of the generation's readable window, the
/// configured traversal ceiling, and the caller's token expiration.
fn cursor_expiration(
    readable_until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    token_exp: i64,
    traversal_window_seconds: i64,
    initial_deadline: Option<DateTime<Utc>>,
) -> DateTime<Utc> {
    let traversal_ceiling =
        initial_deadline.unwrap_or_else(|| now + Duration::seconds(traversal_window_seconds));
    let mut expires_at = readable_until.unwrap_or(traversal_ceiling);
    if traversal_ceiling < expires_at {
        expires_at = traversal_ceiling;
    }
    if let Some(token_deadline) = DateTime::from_timestamp(token_exp, 0) {
        if token_deadline < expires_at {
            expires_at = token_deadline;
        }
    }
    DateTime::from_timestamp(expires_at.timestamp(), 0).unwrap_or(expires_at)
}

async fn cache_traversal_window_seconds(state: &AppState) -> CacheResult<i64> {
    let retention = RetentionRepository::load_config(&state.db).await?;
    i64::try_from(retention.cache_retention.min_traversal_window_seconds)
        .map_err(|_| CacheApiError::bad_request("cache traversal window is too large"))
}

// ---------------------------------------------------------------------------
// Refresh lifecycle routes
// ---------------------------------------------------------------------------

/// Begin a staging generation.
#[utoipa::path(
    post,
    path = "/api/v1/cache/namespaces/{namespace}/generations",
    operation_id = "create_generation",
    tag = "caches",
    request_body = CreateCacheGenerationRequest,
    params(("namespace" = String, Path, description = "Cache namespace")),
    responses(
        (status = 201, description = "Staging generation created", body = CacheGenerationApiResponse),
        (status = 200, description = "Matching idempotent generation replay", body = CacheGenerationApiResponse),
        (status = 400, description = "Invalid owner selector, namespace, or generation request", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Generation creation is not permitted", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace not found", body = ErrorResponse),
        (status = 409, description = "Refresh id, active-generation precondition, namespace state, or quota conflict", body = ErrorResponse),
        (status = 500, description = "Generation creation failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_generation(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    body: Bytes,
) -> CacheResult<Response> {
    let request: CreateCacheGenerationRequest = parse_json_with_required_field(
        &body,
        "expected_active_generation_id",
        "generation request",
    )?;
    let namespace = normalize_namespace(&namespace)?;
    if request.client_refresh_id.trim().is_empty() {
        return Err(CacheApiError::bad_request("client_refresh_id is required"));
    }
    let (_authority, scope) = authorize_namespace_action(
        &state,
        &user.0,
        Action::Create,
        request.owner_type,
        request.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let record = resolve_namespace_for_write(&state.db, &scope, &namespace).await?;

    let expected_chunk_count = i32::try_from(request.expected_chunk_count)
        .map_err(|_| CacheApiError::bad_request("expected_chunk_count is out of range"))?;

    let result = CacheGenerationRepository::create_or_get_with_policy(
        &state.db,
        &CreateCacheGenerationInput {
            namespace: record.id,
            client_refresh_id: request.client_refresh_id.clone(),
            expected_active_generation: request.expected_active_generation_id,
            expected_chunk_count,
            expected_count: request.expected_record_count,
            expected_bytes: request.expected_size_bytes,
            checksum_algorithm: None,
            checksum: None,
            source_revision: request.source_revision.clone(),
            created_by: user.0.identity_id().ok(),
        },
        &state.config.cache_admission,
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            emit_cache_audit(
                &state,
                &user,
                cache_event::GENERATION_CREATED,
                AuditOutcome::Failure,
                "cache_generation",
                None,
                Some(record.namespace.clone()),
                serde_json::json!({
                    "namespace_id": record.id,
                    "reason": audit_write_error_reason(&error),
                }),
            );
            return Err(map_write_error(error));
        }
    };

    let (generation, created) = match result {
        CreateCacheGenerationResult::Created(generation) => (generation, true),
        CreateCacheGenerationResult::Existing(generation) => (generation, false),
    };

    if created {
        emit_cache_audit(
            &state,
            &user,
            cache_event::GENERATION_CREATED,
            AuditOutcome::Success,
            "cache_generation",
            Some(generation.id),
            Some(record.namespace.clone()),
            serde_json::json!({
                "namespace": record.namespace,
                "generation": generation.id,
                "client_refresh_id": generation.client_refresh_id,
                "source_revision": generation.source_revision,
            }),
        );
    }

    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(ApiResponse::new(generation_response(&generation))),
    )
        .into_response())
}

/// Upload a numbered ingest chunk. Idempotent by generation/chunk index and a
/// server-computed request digest.
#[utoipa::path(
    put,
    path = "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/chunks/{chunk_index}",
    operation_id = "upload_chunk",
    tag = "caches",
    request_body = UploadCacheChunkRequest,
    params(
        ("namespace" = String, Path, description = "Cache namespace"),
        ("generation_id" = i64, Path, description = "Generation id"),
        ("chunk_index" = i32, Path, description = "Zero-based chunk index")
    ),
    responses(
        (status = 200, description = "Chunk accepted or idempotently replayed", body = CacheGenerationApiResponse),
        (status = 400, description = "Invalid owner selector, chunk index, or chunk body", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Chunk upload is not permitted", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace or generation not found", body = ErrorResponse),
        (status = 409, description = "Chunk, generation state, duplicate identifier, or quota conflict", body = ErrorResponse),
        (status = 413, description = "Chunk request exceeds the configured body limit", body = String, content_type = "text/plain"),
        (status = 500, description = "Chunk upload failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn upload_chunk(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((namespace, generation_id, chunk_index)): Path<(String, i64, i32)>,
    body: Bytes,
) -> CacheResult<Response> {
    let request: UploadCacheChunkRequest = serde_json::from_slice(&body)
        .map_err(|err| CacheApiError::bad_request(format!("invalid chunk body: {err}")))?;
    // The request digest is derived from the exact request bytes so an
    // identical retry replays as success while a different payload for the same
    // chunk index is a conflict.
    let request_checksum = hex_encode(&Sha256::digest(&body));

    let namespace = normalize_namespace(&namespace)?;
    let (_authority, scope) = authorize_namespace_action(
        &state,
        &user.0,
        Action::Update,
        request.owner_type,
        request.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let record = resolve_namespace_for_write(&state.db, &scope, &namespace).await?;
    let generation = CacheGenerationRepository::find_by_id(&state.db, generation_id)
        .await?
        .filter(|generation| generation.namespace == record.id)
        .ok_or_else(|| CacheApiError::not_found("cache generation not found"))?;

    if request.entries.is_empty() {
        return Err(CacheApiError::bad_request("chunk entries cannot be empty"));
    }

    let entries: Vec<CacheEntryInput> = request
        .entries
        .into_iter()
        .map(|entry| CacheEntryInput {
            external_id: entry.external_id,
            value: entry.value,
            source_updated_at: entry.source_updated_at,
            source_checksum: entry.source_checksum,
        })
        .collect();

    let result = CacheIngestRepository::insert_chunk_with_policy(
        &state.db,
        generation.id,
        chunk_index,
        &request_checksum,
        &entries,
        &state.config.cache_admission,
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            emit_cache_audit(
                &state,
                &user,
                cache_event::GENERATION_CHUNK_UPLOADED,
                AuditOutcome::Failure,
                "cache_generation",
                Some(generation.id),
                Some(record.namespace.clone()),
                serde_json::json!({
                    "namespace_id": record.id,
                    "generation": generation.id,
                    "chunk_index": chunk_index,
                    "reason": audit_write_error_reason(&error),
                }),
            );
            return Err(map_write_error(error));
        }
    };

    let (chunk, disposition) = match &result {
        InsertCacheChunkResult::Inserted(chunk) => (chunk, "inserted"),
        InsertCacheChunkResult::Replayed(chunk) => (chunk, "replayed"),
    };
    emit_cache_audit(
        &state,
        &user,
        cache_event::GENERATION_CHUNK_UPLOADED,
        AuditOutcome::Success,
        "cache_generation",
        Some(generation.id),
        Some(record.namespace.clone()),
        serde_json::json!({
            "namespace_id": record.id,
            "generation": generation.id,
            "chunk_index": chunk.chunk_index,
            "record_count": chunk.record_count,
            "size_bytes": chunk.size_bytes,
            "disposition": disposition,
        }),
    );

    let refreshed = CacheGenerationRepository::find_by_id(&state.db, generation.id)
        .await?
        .unwrap_or(generation);
    let status = match result {
        InsertCacheChunkResult::Inserted(_) => StatusCode::OK,
        InsertCacheChunkResult::Replayed(_) => StatusCode::OK,
    };
    Ok((
        status,
        Json(ApiResponse::new(generation_response(&refreshed))),
    )
        .into_response())
}

/// Seal a staging generation into `ready`.
#[utoipa::path(
    post,
    path = "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/seal",
    operation_id = "seal_generation",
    tag = "caches",
    request_body = SealCacheGenerationRequest,
    params(
        ("namespace" = String, Path, description = "Cache namespace"),
        ("generation_id" = i64, Path, description = "Generation id")
    ),
    responses(
        (status = 200, description = "Sealed generation", body = CacheGenerationApiResponse),
        (status = 400, description = "Invalid owner selector, namespace, or seal expectations", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Generation sealing is not permitted", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace or generation not found", body = ErrorResponse),
        (status = 409, description = "Generation state or seal expectations conflict", body = ErrorResponse),
        (status = 500, description = "Generation sealing failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn seal_generation(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((namespace, generation_id)): Path<(String, i64)>,
    Json(request): Json<SealCacheGenerationRequest>,
) -> CacheResult<Response> {
    let namespace = normalize_namespace(&namespace)?;
    let (_authority, scope) = authorize_namespace_action(
        &state,
        &user.0,
        Action::Update,
        request.owner_type,
        request.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let record = resolve_namespace_for_write(&state.db, &scope, &namespace).await?;
    let generation = CacheGenerationRepository::find_by_id(&state.db, generation_id)
        .await?
        .filter(|generation| generation.namespace == record.id)
        .ok_or_else(|| CacheApiError::not_found("cache generation not found"))?;

    let expected_chunk_count = i32::try_from(request.expected_chunk_count)
        .map_err(|_| CacheApiError::bad_request("expected_chunk_count is out of range"))?;
    let sealed = CacheGenerationRepository::seal_with_expectations(
        &state.db,
        generation.id,
        Some(SealCacheGenerationInput {
            expected_chunk_count,
            expected_count: request.expected_record_count,
            expected_bytes: request.expected_size_bytes,
        }),
    )
    .await;
    let sealed = match sealed {
        Ok(sealed) => sealed,
        Err(error) => {
            emit_cache_audit(
                &state,
                &user,
                cache_event::GENERATION_SEALED,
                AuditOutcome::Failure,
                "cache_generation",
                Some(generation.id),
                Some(record.namespace.clone()),
                serde_json::json!({
                    "namespace_id": record.id,
                    "generation": generation.id,
                    "reason": audit_write_error_reason(&error),
                }),
            );
            return Err(map_write_error(error));
        }
    };

    if generation.state == CacheGenerationState::Staging {
        emit_cache_audit(
            &state,
            &user,
            cache_event::GENERATION_SEALED,
            AuditOutcome::Success,
            "cache_generation",
            Some(sealed.id),
            Some(record.namespace.clone()),
            serde_json::json!({
                "namespace": record.namespace,
                "generation": sealed.id,
                "record_count": sealed.record_count,
                "size_bytes": sealed.size_bytes,
            }),
        );
    }

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(generation_response(&sealed))),
    )
        .into_response())
}

/// Atomically promote a ready generation.
#[utoipa::path(
    post,
    path = "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/promote",
    operation_id = "promote_generation",
    tag = "caches",
    request_body = PromoteCacheGenerationRequest,
    params(
        ("namespace" = String, Path, description = "Cache namespace"),
        ("generation_id" = i64, Path, description = "Generation id")
    ),
    responses(
        (status = 200, description = "Promoted generation", body = CacheGenerationApiResponse),
        (status = 400, description = "Invalid owner selector, namespace, or promotion request", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Generation promotion is not permitted", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace or generation not found", body = ErrorResponse),
        (status = 409, description = "Promotion state or active-generation precondition failed", body = ErrorResponse),
        (status = 500, description = "Generation promotion failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn promote_generation(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((namespace, generation_id)): Path<(String, i64)>,
    body: Bytes,
) -> CacheResult<Response> {
    let request: PromoteCacheGenerationRequest = parse_json_with_required_field(
        &body,
        "expected_active_generation_id",
        "promotion request",
    )?;
    let namespace = normalize_namespace(&namespace)?;
    let (_authority, scope) = authorize_namespace_action(
        &state,
        &user.0,
        Action::Update,
        request.owner_type,
        request.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let record = resolve_namespace_for_write(&state.db, &scope, &namespace).await?;
    let generation = CacheGenerationRepository::find_by_id(&state.db, generation_id)
        .await?
        .filter(|generation| generation.namespace == record.id)
        .ok_or_else(|| CacheApiError::not_found("cache generation not found"))?;

    let traversal_window_seconds = cache_traversal_window_seconds(&state).await?;
    let prior_readable_until = Utc::now() + Duration::seconds(traversal_window_seconds);
    let promotion = CacheGenerationRepository::promote(
        &state.db,
        record.id,
        generation.id,
        request.expected_active_generation_id,
        prior_readable_until,
    )
    .await;
    let promotion = match promotion {
        Ok(promotion) => promotion,
        Err(error) => {
            tracing::warn!(
                component = "cache_api",
                metric_set = "cache_api_outcomes",
                cache_promotion_failure_count = 1u64,
                namespace_id = record.id,
                generation_id = generation.id,
                "Cache generation promotion failed"
            );
            emit_cache_audit(
                &state,
                &user,
                cache_event::GENERATION_PROMOTED,
                AuditOutcome::Failure,
                "cache_generation",
                Some(generation.id),
                Some(record.namespace.clone()),
                serde_json::json!({
                    "namespace_id": record.id,
                    "generation": generation.id,
                    "reason": audit_write_error_reason(&error),
                }),
            );
            return Err(map_write_error(error));
        }
    };

    if !promotion.replayed {
        emit_cache_audit(
            &state,
            &user,
            cache_event::GENERATION_PROMOTED,
            AuditOutcome::Success,
            "cache_generation",
            Some(promotion.activated_generation.id),
            Some(record.namespace.clone()),
            serde_json::json!({
                "namespace": record.namespace,
                "generation": promotion.activated_generation.id,
                "retired_generation": promotion.retired_generation,
                "record_count": promotion.activated_generation.record_count,
                "size_bytes": promotion.activated_generation.size_bytes,
            }),
        );
    }

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(generation_response(
            &promotion.activated_generation,
        ))),
    )
        .into_response())
}

/// Abandon a staging or ready generation.
#[utoipa::path(
    post,
    path = "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/abandon",
    operation_id = "abandon_generation",
    tag = "caches",
    request_body = CacheOwnerBody,
    params(
        ("namespace" = String, Path, description = "Cache namespace"),
        ("generation_id" = i64, Path, description = "Generation id")
    ),
    responses(
        (status = 200, description = "Abandoned generation", body = CacheGenerationApiResponse),
        (status = 400, description = "Invalid owner selector, namespace, or generation id", body = ErrorResponse),
        (status = 401, description = "Authentication required", body = AuthErrorResponse),
        (status = 403, description = "Generation abandonment is not permitted", body = CacheForbiddenResponse),
        (status = 404, description = "Namespace or generation not found", body = ErrorResponse),
        (status = 409, description = "Generation cannot be abandoned from its current state", body = ErrorResponse),
        (status = 500, description = "Generation abandonment failed", body = ErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn abandon_generation(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((namespace, generation_id)): Path<(String, i64)>,
    Json(request): Json<CacheOwnerBody>,
) -> CacheResult<Response> {
    let namespace = normalize_namespace(&namespace)?;
    let (_authority, scope) = authorize_namespace_action(
        &state,
        &user.0,
        Action::Update,
        request.owner_type,
        request.owner_ref.as_deref(),
        &namespace,
    )
    .await?;

    let record = resolve_namespace_for_write(&state.db, &scope, &namespace).await?;
    let generation = CacheGenerationRepository::find_by_id(&state.db, generation_id)
        .await?
        .filter(|generation| generation.namespace == record.id)
        .ok_or_else(|| CacheApiError::not_found("cache generation not found"))?;

    let failed =
        CacheGenerationRepository::fail(&state.db, generation.id, "refresh abandoned").await;
    let failed = match failed {
        Ok(failed) => failed,
        Err(error) => {
            emit_cache_audit(
                &state,
                &user,
                cache_event::GENERATION_ABANDONED,
                AuditOutcome::Failure,
                "cache_generation",
                Some(generation.id),
                Some(record.namespace.clone()),
                serde_json::json!({
                    "namespace_id": record.id,
                    "generation": generation.id,
                    "reason": audit_write_error_reason(&error),
                }),
            );
            return Err(map_write_error(error));
        }
    };

    if generation.state != CacheGenerationState::Failed {
        emit_cache_audit(
            &state,
            &user,
            cache_event::GENERATION_ABANDONED,
            AuditOutcome::Success,
            "cache_generation",
            Some(failed.id),
            Some(record.namespace.clone()),
            serde_json::json!({
                "namespace": record.namespace,
                "generation": failed.id,
            }),
        );
    }

    Ok((
        StatusCode::OK,
        Json(ApiResponse::new(generation_response(&failed))),
    )
        .into_response())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod admission_error_tests {
    use super::*;

    #[test]
    fn aggregate_quota_errors_keep_their_stable_api_code() {
        let error = map_write_error(CommonError::cache_quota_exceeded(
            "cache_owner_physical_bytes_limit_exceeded",
            "cache owner physical byte limit exceeded",
        ));
        assert!(matches!(
            error,
            CacheApiError::Coded {
                status: StatusCode::CONFLICT,
                code: "cache_owner_physical_bytes_limit_exceeded",
                ref message,
            } if message == "cache owner physical byte limit exceeded"
        ));
    }
}

/// Registers all cache routes. Every route is protected with [`RequireAuth`].
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/cache/namespaces",
            get(list_namespaces).post(create_namespace),
        )
        .route(
            "/cache/namespaces/{namespace}",
            get(show_namespace)
                .put(update_namespace)
                .delete(delete_namespace),
        )
        .route("/cache/namespaces/{namespace}/entries", get(scan_entries))
        .route(
            "/cache/namespaces/{namespace}/entries/lookup",
            post(lookup_entry),
        )
        .route(
            "/cache/namespaces/{namespace}/entries/lookup-many",
            post(lookup_entries),
        )
        .route(
            "/cache/namespaces/{namespace}/generations",
            get(list_generations).post(create_generation),
        )
        .route(
            "/cache/namespaces/{namespace}/generations/{generation_id}",
            get(show_generation),
        )
        .route(
            "/cache/namespaces/{namespace}/generations/{generation_id}/chunks/{chunk_index}",
            put(upload_chunk).layer(DefaultBodyLimit::max(MAX_INGEST_CHUNK_BYTES)),
        )
        .route(
            "/cache/namespaces/{namespace}/generations/{generation_id}/seal",
            post(seal_generation),
        )
        .route(
            "/cache/namespaces/{namespace}/generations/{generation_id}/promote",
            post(promote_generation),
        )
        .route(
            "/cache/namespaces/{namespace}/generations/{generation_id}/abandon",
            post(abandon_generation),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::rbac::{GrantConstraints, OwnerConstraint};

    const SECRET: &str = "test-cursor-signing-secret";

    fn sample_cursor() -> CacheCursor {
        CacheCursor {
            v: CURSOR_VERSION,
            namespace_id: 7,
            generation_id: 42,
            page_size: 100,
            require_fresh: false,
            last_external_id: "user-000123".to_string(),
            expires_at: Utc::now().timestamp() + 600,
        }
    }

    #[test]
    fn cursor_round_trips() {
        let cursor = sample_cursor();
        let token = encode_cursor(SECRET, &cursor).unwrap();
        let decoded = decode_cursor(SECRET, &token).unwrap();
        assert_eq!(cursor, decoded);
    }

    #[test]
    fn cursor_rejects_tampered_payload() {
        let token = encode_cursor(SECRET, &sample_cursor()).unwrap();
        let (payload, signature) = token.split_once('.').unwrap();
        // Flip the payload but keep the old signature.
        let mut tampered = sample_cursor();
        tampered.generation_id = 99;
        let forged_payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&tampered).unwrap());
        let forged = format!("{forged_payload}.{signature}");
        assert!(decode_cursor(SECRET, &forged).is_err());
        // Sanity: the untouched token still validates.
        assert!(decode_cursor(SECRET, &format!("{payload}.{signature}")).is_ok());
    }

    #[test]
    fn cursor_rejects_wrong_secret() {
        let token = encode_cursor(SECRET, &sample_cursor()).unwrap();
        assert!(decode_cursor("another-secret", &token).is_err());
    }

    #[test]
    fn cursor_rejects_wrong_version() {
        let mut cursor = sample_cursor();
        cursor.v = CURSOR_VERSION + 1;
        let token = encode_cursor(SECRET, &cursor).unwrap();
        assert!(decode_cursor(SECRET, &token).is_err());
    }

    #[test]
    fn cursor_rejects_malformed_token() {
        assert!(decode_cursor(SECRET, "not-a-cursor").is_err());
        assert!(decode_cursor(SECRET, "").is_err());
    }

    #[test]
    fn namespace_is_normalized_and_validated() {
        assert_eq!(
            normalize_namespace("  Salesforce.Users ").unwrap(),
            "salesforce.users"
        );
        assert_eq!(normalize_namespace("users").unwrap(), "users");
        assert!(normalize_namespace("").is_err());
        assert!(normalize_namespace("-leading").is_err());
        assert!(normalize_namespace("bad/name").is_err());
        assert!(normalize_namespace(&"x".repeat(129)).is_err());
    }

    #[test]
    fn cache_page_limits_are_rejected_instead_of_clamped() {
        assert_eq!(validate_metadata_page_size(None).unwrap(), 100);
        assert_eq!(validate_metadata_page_size(Some(500)).unwrap(), 500);
        assert!(validate_metadata_page_size(Some(0)).is_err());
        assert!(validate_metadata_page_size(Some(501)).is_err());

        assert_eq!(validate_scan_page_size(None).unwrap(), 100);
        assert_eq!(validate_scan_page_size(Some(1_000)).unwrap(), 1_000);
        assert!(validate_scan_page_size(Some(-1)).is_err());
        assert!(validate_scan_page_size(Some(1_001)).is_err());
    }

    #[test]
    fn retained_generation_policy_requires_traversal_overlap() {
        let valid = CacheNamespacePolicyBody {
            max_retained_generations: Some(2),
            ..Default::default()
        };
        assert!(validate_policy_body(&valid).is_ok());

        let invalid = CacheNamespacePolicyBody {
            max_retained_generations: Some(1),
            ..Default::default()
        };
        assert!(validate_policy_body(&invalid).is_err());
    }

    #[test]
    fn audit_failure_reasons_are_bounded_labels() {
        assert_eq!(
            audit_write_error_reason(&CommonError::Validation(
                "generation record quota exceeded".to_string()
            )),
            "quota"
        );
        assert_eq!(
            audit_write_error_reason(&CommonError::InvalidState(
                "active generation changed".to_string()
            )),
            "precondition"
        );
        assert_eq!(
            audit_write_error_reason(&CommonError::InvalidState(
                "generation cannot be abandoned".to_string()
            )),
            "conflict"
        );
    }

    #[test]
    fn system_and_identity_owner_selectors_reject_ambient_refs() {
        assert!(normalize_owner_selector(OwnerType::System, None).is_ok());
        assert!(normalize_owner_selector(OwnerType::Identity, None).is_ok());
        assert!(normalize_owner_selector(OwnerType::System, Some("system")).is_err());
        assert!(normalize_owner_selector(OwnerType::Identity, Some("42")).is_err());
    }

    #[test]
    fn namespace_list_scope_distinguishes_all_and_concrete_owners() {
        assert_eq!(
            normalize_namespace_list_scope(None, None).unwrap(),
            CacheNamespaceListScope::Any
        );
        assert_eq!(
            normalize_namespace_list_scope(Some(OwnerType::Pack), Some(" core ")).unwrap(),
            CacheNamespaceListScope::Owner {
                owner_type: OwnerType::Pack,
                owner_ref: Some("core".to_string()),
            }
        );
        assert!(normalize_namespace_list_scope(None, Some("core")).is_err());
    }

    #[test]
    fn global_cache_grant_compiler_projects_row_constraints() {
        let grant = Grant {
            resource: Resource::Caches,
            actions: vec![Action::Read],
            constraints: Some(GrantConstraints {
                owner: Some(OwnerConstraint::None),
                owner_types: Some(vec![OwnerType::Pack]),
                owner_refs: Some(vec!["core".to_string()]),
                refs: Some(vec!["users".to_string()]),
                attributes: Some(HashMap::from([(
                    "department".to_string(),
                    serde_json::json!("platform"),
                )])),
                ..Default::default()
            }),
        };
        let attributes = HashMap::from([("department".to_string(), serde_json::json!("platform"))]);

        let filter = compile_cache_namespace_grant_filter(&grant, Some(&attributes)).unwrap();
        assert_eq!(filter.owner, Some(OwnerConstraint::None));
        assert_eq!(filter.owner_types, Some(vec![OwnerType::Pack]));
        assert_eq!(filter.owner_refs, Some(vec!["core".to_string()]));
        assert_eq!(filter.namespace_refs, Some(vec!["users".to_string()]));
        assert!(compile_cache_namespace_grant_filter(&grant, None).is_none());
    }

    #[test]
    fn global_cache_grant_compiler_fails_closed_for_irrelevant_constraints() {
        for constraints in [
            GrantConstraints {
                ids: Some(vec![1]),
                ..Default::default()
            },
            GrantConstraints {
                pack_refs: Some(vec!["core".to_string()]),
                ..Default::default()
            },
            GrantConstraints {
                execution_scope: Some(ExecutionScopeConstraint::SelfOnly),
                ..Default::default()
            },
        ] {
            let grant = Grant {
                resource: Resource::Caches,
                actions: vec![Action::Read],
                constraints: Some(constraints),
            };
            assert!(compile_cache_namespace_grant_filter(&grant, None).is_none());
        }
    }

    #[test]
    fn existing_scoped_namespace_cursor_decodes_with_optional_owner_type() {
        let payload = serde_json::json!({
            "v": CURSOR_VERSION,
            "owner_type": "pack",
            "owner_ref": "core",
            "namespace_filter": null,
            "freshness": null,
            "page_size": 100,
            "after_id": 42,
        });
        let token = encode_signed_cursor(SECRET, &payload).unwrap();
        let cursor = decode_namespace_cursor(SECRET, &token).unwrap();
        assert_eq!(cursor.owner_type, Some(OwnerType::Pack));
        assert_eq!(cursor.owner_ref.as_deref(), Some("core"));
    }

    fn cache_read_grant(owner_type: OwnerType, owner_ref: &str, namespace: Option<&str>) -> Grant {
        Grant {
            resource: Resource::Caches,
            actions: vec![Action::Read],
            constraints: Some(GrantConstraints {
                owner_types: Some(vec![owner_type]),
                owner_refs: Some(vec![owner_ref.to_string()]),
                refs: namespace.map(|value| vec![value.to_string()]),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn owner_scoped_read_grant_covers_all_namespaces() {
        let grants = vec![cache_read_grant(OwnerType::Pack, "salesforce", None)];
        let ctx = cache_context(1, OwnerType::Pack, Some("salesforce"), None, Some("users"));
        assert!(cache_action_allowed(&grants, Action::Read, &ctx));
        // Owner-level read grant does not authorize writes.
        assert!(!cache_action_allowed(&grants, Action::Update, &ctx));
    }

    #[test]
    fn namespace_scoped_grant_is_limited_to_its_namespace() {
        let grants = vec![cache_read_grant(
            OwnerType::Pack,
            "salesforce",
            Some("users"),
        )];
        let users_ctx = cache_context(1, OwnerType::Pack, Some("salesforce"), None, Some("users"));
        let other_ctx = cache_context(
            1,
            OwnerType::Pack,
            Some("salesforce"),
            None,
            Some("locations"),
        );
        assert!(cache_action_allowed(&grants, Action::Read, &users_ctx));
        assert!(!cache_action_allowed(&grants, Action::Read, &other_ctx));
    }

    #[test]
    fn unrelated_owner_is_never_covered() {
        let grants = vec![cache_read_grant(OwnerType::Pack, "salesforce", None)];
        let ctx = cache_context(1, OwnerType::Pack, Some("other"), None, Some("users"));
        assert!(!cache_action_allowed(&grants, Action::Read, &ctx));
    }

    #[test]
    fn sensor_cache_grants_fail_closed_without_signed_authority() {
        let claims = crate::auth::jwt::Claims {
            sub: "5".to_string(),
            login: "sensor:core.timer".to_string(),
            iat: 0,
            exp: Utc::now().timestamp() + 600,
            token_type: TokenType::Sensor,
            scope: Some("sensor".to_string()),
            metadata: Some(serde_json::json!({ "trigger_types": ["core.timer"] })),
        };
        let user = AuthenticatedUser { claims };
        assert!(sensor_cache_grants(&user).is_empty());
    }

    #[test]
    fn sensor_cache_grants_honor_signed_read_authority_only() {
        let signed = serde_json::json!([
            {
                "resource": "caches",
                "actions": ["read"],
                "constraints": { "owner_types": ["sensor"], "owner_refs": ["core.timer"] }
            },
            {
                "resource": "keys",
                "actions": ["read", "decrypt"]
            }
        ]);
        let claims = crate::auth::jwt::Claims {
            sub: "5".to_string(),
            login: "sensor:core.timer".to_string(),
            iat: 0,
            exp: Utc::now().timestamp() + 600,
            token_type: TokenType::Sensor,
            scope: Some("sensor".to_string()),
            metadata: Some(serde_json::json!({ "cache_grants": signed })),
        };
        let user = AuthenticatedUser { claims };
        let grants = sensor_cache_grants(&user);
        // The non-cache grant is dropped; only the scoped cache read remains.
        assert_eq!(grants.len(), 1);
        let ctx = cache_context(
            5,
            OwnerType::Sensor,
            Some("core.timer"),
            None,
            Some("events"),
        );
        assert!(cache_action_allowed(&grants, Action::Read, &ctx));
        assert!(!cache_action_allowed(&grants, Action::Update, &ctx));
    }

    #[test]
    fn cursor_expiration_is_bounded_by_token_expiry() {
        let now = Utc::now();
        let readable_until = Some(now + Duration::seconds(10_000));
        // Token expires sooner than both the readable window and traversal ceiling.
        let token_exp = (now + Duration::seconds(30)).timestamp();
        let expires_at = cursor_expiration(readable_until, now, token_exp, 60 * 60, None);
        assert_eq!(expires_at.timestamp(), token_exp);
    }

    #[test]
    fn cursor_expiration_uses_readable_until_when_earliest() {
        let now = Utc::now();
        let readable_until = Some(now + Duration::seconds(120));
        let token_exp = (now + Duration::seconds(100_000)).timestamp();
        let expires_at = cursor_expiration(readable_until, now, token_exp, 60 * 60, None);
        assert_eq!(
            expires_at.timestamp(),
            (now + Duration::seconds(120)).timestamp()
        );
    }

    #[test]
    fn cursor_expiration_preserves_initial_traversal_deadline() {
        let now = Utc::now();
        let initial_deadline = now + Duration::seconds(90);
        let expires_at = cursor_expiration(
            Some(now + Duration::hours(1)),
            now + Duration::seconds(30),
            (now + Duration::hours(2)).timestamp(),
            60 * 60,
            Some(initial_deadline),
        );
        assert_eq!(expires_at.timestamp(), initial_deadline.timestamp());
    }

    #[test]
    fn scan_snapshot_expiry_is_surfaced_as_snapshot_expired() {
        // The repository signals a vanished pinned generation with a typed
        // error; the read mapper must surface it as snapshot_expired (not a
        // generic conflict) so clients restart cleanly instead of treating an
        // empty page as end-of-data.
        let mapped = map_read_error(CommonError::CacheSnapshotExpired("gone".to_string()));
        match mapped {
            CacheApiError::Coded { code, .. } => assert_eq!(code, "snapshot_expired"),
            other => panic!("expected snapshot_expired coded error, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_external_id_is_surfaced_with_a_distinct_code() {
        // insert_chunk returns a typed, ID-free duplicate error; the write
        // mapper gives it a distinct machine code (never a raw ID list).
        let mapped = map_write_error(CommonError::CacheDuplicateExternalId);
        match mapped {
            CacheApiError::Coded {
                code,
                status,
                message,
            } => {
                assert_eq!(code, "cache_duplicate_external_id");
                assert_eq!(status, StatusCode::CONFLICT);
                assert!(!message.is_empty());
            }
            other => panic!("expected cache_duplicate_external_id coded error, got {other:?}"),
        }
    }
}
