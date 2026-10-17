//! Key/Secret management API routes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;
use validator::Validate;

use attune_common::repositories::{
    action::ActionRepository,
    identity::IdentityRepository,
    key::{
        CreateKeyInput, KeyGrantFilter, KeyRepository, KeySearchFilters, KeyVisibility,
        UpdateKeyInput,
    },
    key_schema::{KeySchemaAssignment, KeySchemaRepository},
    pack::PackRepository,
    schema_contract::SchemaContractRepository,
    schema_type::SchemaTypeRepository,
    trigger::SensorRepository,
    Create, Delete, FindById, FindByRef, Update,
};
use attune_common::{
    audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome, PendingAuditEvent},
    key_ref::canonical_key_ref,
    models::{key::Key, OwnerType},
    rbac::{
        Action, AuthorizationContext, ExecutionScopeConstraint, Grant, GrantConstraints, Resource,
    },
};

use crate::auth::{jwt::TokenType, RequireAuth};
use crate::{
    authz::AuthorizationService,
    dto::{
        common::{PaginatedResponse, PaginationParams},
        key::{
            CreateKeyRequest, KeyQueryParams, KeyResponse, KeySummary, SignKeyJwtRequest,
            SignKeyJwtResponse, UpdateKeyRequest,
        },
        ApiResponse, SuccessResponse,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};

async fn assign_key_schema(
    connection: &mut sqlx::PgConnection,
    key_ref: &str,
    schema: &serde_json::Value,
) -> ApiResult<KeySchemaAssignment> {
    let contract = SchemaContractRepository::materialize_assignment(
        connection,
        attune_common::value_schema::SourceIdentity::Assigned {
            component_ref: key_ref.to_owned(),
            assignment_id: uuid::Uuid::new_v4(),
        },
        attune_common::value_schema::SchemaContext::SingleValue,
        schema,
    )
    .await?;
    Ok(KeySchemaAssignment {
        authored_schema: schema.clone(),
        contract,
    })
}

fn compare_key_schema_preview(
    expected: Option<&str>,
    assignment: Option<&KeySchemaAssignment>,
) -> ApiResult<()> {
    if let Some(expected) = expected {
        let assignment = assignment.ok_or_else(|| {
            ApiError::BadRequest("A schema preview digest requires an assigned value schema".into())
        })?;
        let actual = assignment
            .contract
            .semantic_digest()
            .map_err(|_| ApiError::BadRequest("Invalid Key schema digest".into()))?;
        if actual != expected {
            return Err(ApiError::Conflict(
                "The accepted Key schema differs from the preview; refresh the assigned schema"
                    .into(),
            ));
        }
    }
    Ok(())
}

async fn key_grants(state: &AppState, user: &RequireAuth) -> ApiResult<(i64, Vec<Grant>)> {
    let identity_id = user
        .0
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".to_string()))?;

    match user.0.claims.token_type {
        TokenType::Access | TokenType::Execution => {
            let snapshot = state
                .authorization_service()
                .load_snapshot(&user.0)
                .await?
                .ok_or_else(|| {
                    ApiError::Unauthorized("Identity authorization is unavailable".into())
                })?;
            let grants = snapshot
                .grants
                .into_iter()
                .filter_map(|mut grant| {
                    if let Some(scope) = &mut grant.constraints {
                        if scope.attributes.as_ref().is_some_and(|attributes| {
                            attributes.iter().any(|(name, expected)| {
                                snapshot.identity_attributes.get(name) != Some(expected)
                            })
                        }) {
                            return None;
                        }
                        scope.attributes = None;
                    }
                    Some(grant)
                })
                .collect();
            Ok((identity_id, grants))
        }
        TokenType::Sensor => {
            let pack_ref = user.0.claims.sensor_pack_ref().map_err(|_| {
                ApiError::Unauthorized("Sensor token is missing its pack scope".to_string())
            })?;
            Ok((
                identity_id,
                vec![Grant {
                    resource: Resource::Keys,
                    actions: vec![Action::Read, Action::Decrypt],
                    constraints: Some(GrantConstraints {
                        owner_types: Some(vec![OwnerType::Pack]),
                        owner_refs: Some(vec![pack_ref.to_string()]),
                        ..GrantConstraints::default()
                    }),
                }],
            ))
        }
        TokenType::Worker | TokenType::Refresh => Err(ApiError::Forbidden(
            "This token type cannot access keys".to_string(),
        )),
    }
}

/// Load human/sensor grants before reserving an effect connection. Recorded
/// credentials never enter mutable IAM; their permits are validated in the
/// effect transaction instead. This also supports single-connection pools.
async fn delegated_key_grants(
    state: &AppState,
    user: &RequireAuth,
) -> ApiResult<Option<(i64, Vec<Grant>)>> {
    if user
        .0
        .claims
        .admitted_execution_credential()
        .map_err(|_| ApiError::Unauthorized("Invalid recorded execution credential".into()))?
        .is_some()
    {
        Ok(None)
    } else {
        key_grants(state, user).await.map(Some)
    }
}

fn missing_key_authority() -> ApiError {
    ApiError::Unauthorized("Key authority is unavailable".into())
}

fn require_key_mutation_token(user: &RequireAuth) -> ApiResult<()> {
    match user.0.claims.token_type {
        TokenType::Access | TokenType::Execution => Ok(()),
        TokenType::Sensor | TokenType::Worker | TokenType::Refresh => Err(ApiError::Forbidden(
            "This token type cannot modify keys".to_string(),
        )),
    }
}

/// List all keys with pagination and optional filters (values redacted)
#[utoipa::path(
    get,
    path = "/api/v1/keys",
    tag = "secrets",
    params(KeyQueryParams),
    responses(
        (status = 200, description = "List of keys (values redacted)", body = PaginatedResponse<KeySummary>),
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_keys(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<KeyQueryParams>,
) -> ApiResult<impl IntoResponse> {
    let delegated = delegated_key_grants(&state, &user).await?;
    let mut transaction = state.db.begin().await?;
    let recorded = user.0.recorded_runtime_authority(&mut transaction).await?;
    let (identity_id, filters) = if let Some(authority) = &recorded {
        (
            authority.authority().owner.sponsor_identity_id,
            authority.key_read_filters(),
        )
    } else {
        let (identity_id, grants) = delegated.ok_or_else(missing_key_authority)?;
        if !grants
            .iter()
            .any(|grant| grant.resource == Resource::Keys && grant.actions.contains(&Action::Read))
        {
            return Err(ApiError::Forbidden(
                "Insufficient permissions: keys:read".into(),
            ));
        }
        (identity_id, compile_key_read_grant_filters(&grants))
    };
    if recorded.is_some() && filters.is_empty() {
        return Err(ApiError::Forbidden(
            "Insufficient recorded authority: keys:read".into(),
        ));
    }
    let visibility = Some(KeyVisibility {
        identity_id,
        grants: filters,
    });

    // Owner filters, RBAC visibility, and pagination are all pushed into a
    // single filtered SQL query (see `KeyRepository::search`), so totals and
    // pages are always consistent with what's actually visible.
    let filters = KeySearchFilters {
        owner_type: query.owner_type,
        owner: query.owner.clone(),
        limit: query.limit(),
        offset: query.offset(),
        visibility,
    };

    let result = KeyRepository::search_with_connection(&mut transaction, &filters).await?;
    if let Some(authority) = &recorded {
        authority.ensure_fresh_on(&mut transaction).await?;
    }
    transaction.commit().await?;

    let paginated_keys: Vec<KeySummary> = result.rows.into_iter().map(KeySummary::from).collect();

    let pagination_params = PaginationParams {
        page: query.page,
        page_size: query.per_page,
    };

    let response = PaginatedResponse::new(paginated_keys, &pagination_params, result.total);

    Ok((StatusCode::OK, Json(response)))
}

/// Compiles a caller's effective RBAC grants into the SQL-translatable
/// [`KeyGrantFilter`] list consumed by `KeyRepository::search`.
///
/// Only grants for `Resource::Keys` + `Action::Read` are considered. Grants
/// whose constraints can never be satisfied for keys are dropped entirely
/// (see [`compile_key_grant_filter`]), which is safe because such a grant
/// would never allow any row via `Grant::constraints_match` either.
fn compile_key_read_grant_filters(grants: &[Grant]) -> Vec<KeyGrantFilter> {
    grants
        .iter()
        .filter(|grant| grant.resource == Resource::Keys && grant.actions.contains(&Action::Read))
        .filter_map(compile_key_grant_filter)
        .collect()
}

/// Translates a single grant's constraints into a [`KeyGrantFilter`],
/// mirroring `Grant::constraints_match` for the fields the key
/// `AuthorizationContext` actually populates (owner/owner_type/owner_ref/
/// ref/id/encrypted).
///
/// Returns `None` when the grant can never match any key row: the key
/// authorization context never sets `pack_ref`, `visibility`, an execution
/// scope owner, or identity attributes, so a grant constrained on those
/// fields (other than `execution_scope: any`) always fails
/// `constraints_match` regardless of the row.
fn compile_key_grant_filter(grant: &Grant) -> Option<KeyGrantFilter> {
    let Some(constraints) = &grant.constraints else {
        // Unconstrained grant: matches everything except (per
        // `constrained_key_grant_allows`) other identities' identity-owned
        // keys, which `owner_scoped: false` handles in SQL.
        return Some(KeyGrantFilter {
            owner_scoped: false,
            ..Default::default()
        });
    };

    let always_excluded = constraints.pack_refs.is_some()
        || constraints.visibility.is_some()
        || matches!(
            constraints.execution_scope,
            Some(ExecutionScopeConstraint::SelfOnly) | Some(ExecutionScopeConstraint::Descendants)
        )
        || constraints
            .attributes
            .as_ref()
            .is_some_and(|attrs| !attrs.is_empty());

    if always_excluded {
        return None;
    }

    let owner_scoped = constraints.owner.is_some()
        || constraints.owner_types.is_some()
        || constraints.owner_refs.is_some()
        || constraints.refs.is_some()
        || constraints.ids.is_some();

    Some(KeyGrantFilter {
        ownership: constraints
            .managing_pack_id
            .map(attune_common::repositories::key::KeyOwnershipScope::ManagedPack)
            .unwrap_or_default(),
        owner_types: constraints.owner_types.clone(),
        owner: constraints.owner,
        owner_refs: constraints.owner_refs.clone(),
        refs: constraints.refs.clone(),
        ids: constraints.ids.clone(),
        encrypted: constraints.encrypted,
        owner_scoped,
        ..Default::default()
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/keys/{ref}/sign-jwt",
    tag = "secrets",
    params(("ref" = String, Path, description = "System-owned signing key reference")),
    request_body = SignKeyJwtRequest,
    responses((status = 200, description = "Approved signed JWT assertion; no private key is disclosed", body = inline(ApiResponse<SignKeyJwtResponse>))),
    security(("bearer_auth" = []))
)]
pub async fn sign_key_jwt(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(key_ref): Path<String>,
    Json(request): Json<SignKeyJwtRequest>,
) -> ApiResult<impl IntoResponse> {
    if !matches!(
        user.0.claims.token_type,
        TokenType::Execution | TokenType::Sensor
    ) {
        return Err(ApiError::Forbidden(
            "Approved key signing requires execution-scoped access".into(),
        ));
    }
    request.validate()?;
    let mut recorded_transaction = state.db.begin().await?;
    let recorded = user
        .0
        .recorded_runtime_authority(&mut recorded_transaction)
        .await?;
    let recorded_transaction = if recorded.is_some() {
        Some(recorded_transaction)
    } else {
        recorded_transaction.rollback().await?;
        None
    };
    let (identity_id, grants, pack_ref, execution_id, sensor_ref, fence_transaction) = if user
        .0
        .claims
        .token_type
        == TokenType::Sensor
    {
        let sensor_ref = user
            .0
            .claims
            .sensor_ref()
            .map_err(|_| ApiError::Unauthorized("Sensor token scope is missing".into()))?
            .to_string();
        let pack_ref = user
            .0
            .claims
            .sensor_pack_ref()
            .map_err(|_| ApiError::Unauthorized("Sensor pack scope is missing".into()))?
            .to_string();
        let sensor = SensorRepository::find_by_ref(&state.db, &sensor_ref)
            .await?
            .filter(|sensor| {
                sensor.pack_ref.as_deref() == Some(pack_ref.as_str())
                    && sensor.enabled
                    && sensor.retired_at.is_none()
            })
            .ok_or_else(|| ApiError::Unauthorized("Sensor is not active".into()))?;
        let fence = user
            .0
            .sensor_workload_fence()
            .map_err(|_| ApiError::Unauthorized("Sensor workload scope is missing".into()))?;
        let mut tx = state.db.begin().await?;
        if !attune_common::repositories::sensor_workload::SensorWorkloadRepository::lock_current_fence(&mut tx, sensor.id, fence).await? {
            return Err(ApiError::Unauthorized("Sensor workload is no longer current".into()));
        }
        let identity_id = user
            .0
            .identity_id()
            .map_err(|_| ApiError::Unauthorized("Sensor identity is invalid".into()))?;
        let (_, binding, _) =
            attune_common::repositories::sensor_schema::SensorSchemaRepository::find(
                &mut *tx,
                fence.workload_id,
                fence.generation,
            )
            .await?
            .ok_or_else(|| {
                ApiError::Forbidden("Sensor generation binding is unavailable".into())
            })?;
        if binding.executable.sensor.id != sensor.id
            || binding.executable.sensor.r#ref != sensor_ref
            || binding.executable.sensor.pack_ref.as_deref() != Some(pack_ref.as_str())
        {
            return Err(ApiError::Forbidden(
                "Sensor generation signing scope mismatch".into(),
            ));
        }
        let grants = binding.executable.signing_grants;
        let key = KeyRepository::find_by_ref(&mut *tx, &key_ref)
            .await?
            .ok_or_else(|| ApiError::NotFound("Signing key is not available".into()))?;
        if !key_action_allowed(&grants, Action::Use, identity_id, &key) {
            return Err(ApiError::NotFound("Signing key is not available".into()));
        }
        (
            identity_id,
            grants,
            pack_ref,
            None,
            Some(sensor_ref),
            Some(tx),
        )
    } else if let Some(authority) = &recorded {
        (
            authority.authority().owner.sponsor_identity_id,
            Vec::new(),
            authority.authority().owner.pack_ref.clone(),
            Some(authority.credential_binding().execution_id),
            None,
            recorded_transaction,
        )
    } else {
        let (identity_id, grants) = key_grants(&state, &user).await?;
        let metadata =
            user.0.claims.metadata.as_ref().ok_or_else(|| {
                ApiError::Unauthorized("Execution token metadata is missing".into())
            })?;
        let execution_id = metadata["execution_id"]
            .as_i64()
            .ok_or_else(|| ApiError::Unauthorized("Execution token identity is missing".into()))?;
        let execution = attune_common::repositories::execution::ExecutionRepository::find_by_id(
            &state.db,
            execution_id,
        )
        .await?
        .filter(|execution| {
            execution.executor == Some(identity_id)
                && metadata["action_ref"].as_str() == Some(execution.action_ref.as_str())
        })
        .ok_or_else(|| {
            ApiError::Unauthorized("Execution token does not match its execution".into())
        })?;
        let snapshot = execution
            .executable_snapshot
            .as_ref()
            .ok_or_else(|| ApiError::Forbidden("Signing requires a pinned executable".into()))?;
        let pack_ref = snapshot.executable.action.pack_ref.clone();
        (
            identity_id,
            grants,
            pack_ref,
            Some(execution_id),
            None,
            None,
        )
    };
    let mut fence_transaction = match fence_transaction {
        Some(transaction) => transaction,
        None => state.db.begin().await?,
    };
    let key = KeyRepository::find_by_ref_for_share(&mut fence_transaction, &key_ref)
        .await?
        .filter(|key| key.owner_type == OwnerType::System && key.encrypted)
        .ok_or_else(|| ApiError::NotFound("Signing key is not available".into()))?;
    let permitted = if let Some(authority) = &recorded {
        let target = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
            &mut fence_transaction,
            attune_common::repositories::runtime_target::RuntimeTarget::Key(key.id),
            identity_id,
        )
        .await?
        .ok_or_else(|| ApiError::NotFound("Signing key is not available".into()))?;
        authority.allows_new_effect(&target, Action::Use)
            || authority.allows_new_effect(&target, Action::Read)
    } else {
        key_action_allowed(&grants, Action::Use, identity_id, &key)
            || key_action_allowed(&grants, Action::Read, identity_id, &key)
    };
    if !permitted {
        return Err(ApiError::NotFound("Signing key is not available".into()));
    }
    let encryption_key = state
        .config
        .security
        .encryption_key
        .as_ref()
        .ok_or_else(|| ApiError::InternalServerError("Encryption key is not configured".into()))?;
    let value = attune_common::crypto::decrypt_json(&key.value, encryption_key)
        .map_err(|_| ApiError::InternalServerError("Signing key could not be opened".into()))?;
    let signing_key: attune_common::key_signing::JwtSigningKey = serde_json::from_value(value)
        .map_err(|_| {
            ApiError::InternalServerError(
                "Operator signing profile configuration is invalid".into(),
            )
        })?;
    let profile_ref = request.profile_ref.clone();
    let subject = request.subject.clone();
    let signed = tokio::task::spawn_blocking(move || {
        attune_common::key_signing::sign_approved_jwt_for_principal(
            &signing_key,
            &request.profile_ref,
            &pack_ref,
            sensor_ref.as_deref(),
            &request.subject,
            request.ttl_seconds,
            chrono::Utc::now().timestamp(),
        )
    })
    .await
    .map_err(|_| ApiError::InternalServerError("Signing operation failed".into()))??;
    if let Some(authority) = &recorded {
        authority.ensure_fresh_on(&mut fence_transaction).await?;
    }
    fence_transaction.commit().await?;
    emit_key_audit(
        &state,
        &user,
        "secret.key_used",
        AuditOutcome::Success,
        &key,
        serde_json::json!({"operation":"sign_jwt", "profile_ref": profile_ref, "subject": subject, "execution_id":execution_id}),
    );
    Ok(Json(ApiResponse::new(SignKeyJwtResponse {
        assertion: signed.assertion,
        expires_at: signed.expires_at,
    })))
}

/// Get a single key by reference
#[utoipa::path(
    get,
    path = "/api/v1/keys/{ref}",
    tag = "secrets",
    params(
        ("ref" = String, Path, description = "Key reference identifier")
    ),
    responses(
        (status = 200, description = "Key details; encrypted values require decrypt permission", body = inline(ApiResponse<KeyResponse>)),
        (status = 404, description = "Key not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_key(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(key_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    let delegated = delegated_key_grants(&state, &user).await?;
    let mut transaction = state.db.begin().await?;
    SchemaTypeRepository::lock_selection(&mut transaction).await?;
    let recorded = user.0.recorded_runtime_authority(&mut transaction).await?;
    let (identity_id, grants) = match &recorded {
        Some(authority) => (authority.authority().owner.sponsor_identity_id, Vec::new()),
        None => delegated.ok_or_else(missing_key_authority)?,
    };
    let mut key = KeyRepository::find_by_ref_for_share(&mut transaction, &key_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Key '{}' not found", key_ref)))?;

    let resolved = {
        Some(
            attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
                &mut transaction,
                attune_common::repositories::runtime_target::RuntimeTarget::Key(key.id),
                identity_id,
            )
            .await?
            .ok_or_else(|| ApiError::NotFound("Key is not available".into()))?,
        )
    };
    let allows = |action| match (&recorded, &resolved) {
        (Some(authority), Some(target)) => authority.allows_target(target, action),
        _ => key_action_allowed_in_pack(
            &grants,
            action,
            identity_id,
            &key,
            resolved.as_ref().and_then(|target| target.pack_id()),
        ),
    };
    if !allows(Action::Read) {
        return Err(ApiError::NotFound(format!("Key '{}' not found", key_ref)));
    }

    // For encrypted keys, separately check keys:decrypt. Failing this returns
    // a null value rather than ciphertext.
    let can_decrypt = !key.encrypted || allows(Action::Decrypt);
    if can_decrypt {
        if let Some(authority) = &recorded {
            attune_common::repositories::execution_key_read::ExecutionKeyReadRepository::record(
                &mut transaction,
                authority,
                &attune_common::key_access::key_origin(&key),
            )
            .await?;
        }
    }

    if can_decrypt && user.0.claims.token_type == TokenType::Sensor {
        use attune_common::repositories::{
            sensor_schema::SensorSchemaRepository, sensor_workload::SensorWorkloadRepository,
        };
        let fence = user
            .0
            .claims
            .sensor_workload_fence()
            .map_err(|_| ApiError::Unauthorized("Sensor workload scope is missing".into()))?;
        let sensor_ref = user
            .0
            .claims
            .sensor_ref()
            .map_err(|_| ApiError::Unauthorized("Sensor scope is missing".into()))?;
        let sensor = SensorRepository::find_by_ref(&mut *transaction, sensor_ref)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("Sensor is unavailable".into()))?;
        if !SensorWorkloadRepository::lock_current_fence(&mut transaction, sensor.id, fence).await?
        {
            return Err(ApiError::Unauthorized(
                "Sensor workload is no longer current".into(),
            ));
        }
        let (generation, binding, _) =
            SensorSchemaRepository::find(&mut *transaction, fence.workload_id, fence.generation)
                .await?
                .ok_or_else(|| {
                    ApiError::Forbidden("Sensor generation binding is unavailable".into())
                })?;
        if binding.executable.sensor.id != sensor.id
            || binding.executable.sensor.r#ref != sensor_ref
            || binding.executable.sensor.pack_ref.as_deref() != user.0.claims.sensor_pack_ref().ok()
        {
            return Err(ApiError::Forbidden(
                "Sensor generation Key scope mismatch".into(),
            ));
        }
        SensorSchemaRepository::record_key_read_origin(
            &mut transaction,
            generation,
            &attune_common::key_access::key_origin(&key),
        )
        .await?;
    }

    // Decrypt value if encrypted and caller has permission.
    // If they lack Keys::Decrypt, return null rather than the ciphertext.
    if key.encrypted {
        if can_decrypt {
            let encryption_key =
                state
                    .config
                    .security
                    .encryption_key
                    .as_ref()
                    .ok_or_else(|| {
                        ApiError::InternalServerError(
                            "Encryption key not configured on server".to_string(),
                        )
                    })?;

            let decrypted_value = attune_common::crypto::decrypt_json(&key.value, encryption_key)
                .map_err(|e| {
                tracing::error!("Failed to decrypt key '{}': {}", key_ref, e);
                ApiError::InternalServerError(format!("Failed to decrypt key: {}", e))
            })?;

            key.value = decrypted_value;
        } else {
            key.value = serde_json::Value::Null;
        }
    }

    emit_key_audit(
        &state,
        &user,
        if key.encrypted && can_decrypt {
            event_type::secret::KEY_DECRYPTED
        } else {
            event_type::secret::KEY_READ
        },
        AuditOutcome::Success,
        &key,
        serde_json::json!({
            "encrypted": key.encrypted,
            "decrypted": key.encrypted && can_decrypt,
            "owner_type": key.owner_type,
            "owner_ref": key_owner_ref(
                key.owner_type,
                key.owner.as_deref(),
                key.owner_pack_ref.as_deref(),
                key.owner_action_ref.as_deref(),
                key.owner_sensor_ref.as_deref(),
            ),
        }),
    );

    let assignment = KeySchemaRepository::find(&mut *transaction, key.id).await?;
    let response = ApiResponse::new(KeyResponse::from(key).with_schema(assignment.as_ref())?);
    if let Some(authority) = &recorded {
        authority.ensure_fresh_on(&mut transaction).await?;
    }
    transaction.commit().await?;

    Ok((StatusCode::OK, Json(response)))
}

/// Create a new key/secret
#[utoipa::path(
    post,
    path = "/api/v1/keys",
    tag = "secrets",
    request_body = CreateKeyRequest,
    responses(
        (status = 201, description = "Key created successfully", body = inline(ApiResponse<KeyResponse>)),
        (status = 400, description = "Validation error"),
        (status = 409, description = "Key with same ref already exists")
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_key(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateKeyRequest>,
) -> ApiResult<impl IntoResponse> {
    require_key_mutation_token(&user)?;
    // Validate request
    request.validate()?;
    let delegated = delegated_key_grants(&state, &user).await?;
    let mut transaction = state.db.begin().await?;
    SchemaTypeRepository::lock_selection(&mut transaction).await?;
    let recorded = user.0.recorded_runtime_authority(&mut transaction).await?;
    let authorization = if recorded.is_none()
        && matches!(
            user.0.claims.token_type,
            TokenType::Access | TokenType::Execution
        ) {
        let (identity_id, grants) = delegated.ok_or_else(missing_key_authority)?;
        if !grants.iter().any(|grant| {
            grant.resource == Resource::Keys && grant.actions.contains(&Action::Create)
        }) {
            return Err(ApiError::Forbidden(
                "Insufficient permissions: keys:create".to_string(),
            ));
        }
        Some((identity_id, grants))
    } else {
        None
    };

    let requested_owner_ref = requested_key_owner_ref(&request)?;
    let key_ref = canonical_key_ref(
        request.owner_type,
        requested_owner_ref.as_deref(),
        &request.local_ref,
    )
    .map_err(ApiError::BadRequest)?;

    // Resolve the real owner before local-scope matching. Hide missing-owner
    // diagnostics until authorization, just as for foreign owner denial.
    let resolved_owner = resolve_key_owner(&mut transaction, &request)
        .await
        .map_err(|error| {
            if matches!(error, ApiError::BadRequest(_) | ApiError::NotFound(_)) {
                ApiError::Forbidden(
                    "Key owner is not available to the recorded creation authority".into(),
                )
            } else {
                error
            }
        })?;
    if recorded.is_some() || authorization.is_some() {
        use attune_common::repositories::runtime_target::{
            ProspectiveDataOwner, ProspectiveRuntimeTarget, RuntimeTargetRepository,
        };
        let missing = || {
            ApiError::Forbidden(
                "Key owner is not available to the recorded creation authority".into(),
            )
        };
        let owner = match request.owner_type {
            OwnerType::System => ProspectiveDataOwner::System,
            OwnerType::Identity => {
                ProspectiveDataOwner::Identity(resolved_owner.owner_identity.ok_or_else(missing)?)
            }
            OwnerType::Pack => {
                ProspectiveDataOwner::Pack(resolved_owner.owner_pack.ok_or_else(missing)?)
            }
            OwnerType::Action => {
                ProspectiveDataOwner::Action(resolved_owner.owner_action.ok_or_else(missing)?)
            }
            OwnerType::Sensor => {
                ProspectiveDataOwner::Sensor(resolved_owner.owner_sensor.ok_or_else(missing)?)
            }
        };
        let target = RuntimeTargetRepository::resolve_creation_target(
            &mut transaction,
            owner,
            ProspectiveRuntimeTarget::Key {
                local_ref: &request.local_ref,
                encrypted: request.encrypted,
            },
            recorded
                .as_ref()
                .map(|authority| authority.authority().owner.sponsor_identity_id)
                .or_else(|| authorization.as_ref().map(|(identity, _)| *identity))
                .ok_or_else(missing)?,
        )
        .await?
        .ok_or_else(missing)?;
        let allowed = if let Some(authority) = &recorded {
            authority.allows_new_effect(&target, Action::Create)
        } else if let Some((identity_id, grants)) = &authorization {
            let mut ctx = target.context().clone();
            ctx.owner_identity_id = (request.owner_type == OwnerType::Identity
                && request.owner_identity_login.as_deref() == Some(user.0.login()))
            .then_some(*identity_id);
            ctx.owner_ref = requested_owner_ref;
            if request.owner_type == OwnerType::Identity
                && request.owner_identity_login.as_deref() != Some(user.0.login())
            {
                constrained_key_grant_allows(grants, Action::Create, &ctx)
            } else {
                AuthorizationService::is_allowed(grants, Resource::Keys, Action::Create, &ctx)
            }
        } else {
            false
        };
        if target.context().target_ref.as_deref() != Some(key_ref.as_str()) || !allowed {
            return Err(ApiError::Forbidden(
                "Insufficient recorded authority: keys:create".into(),
            ));
        }
    }

    // Keep the database uniqueness constraint as the final concurrency guard.
    if let Some(authority) = &recorded {
        attune_common::repositories::runtime_read::RuntimeReadRepository::require_untainted(
            &mut transaction,
            authority.credential_binding().admission_id,
        )
        .await?;
    }
    if KeyRepository::find_by_ref(&mut *transaction, &key_ref)
        .await?
        .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "Key with ref '{}' already exists",
            key_ref
        )));
    }

    let assignment = if let Some(schema) = &request.value_schema {
        Some(assign_key_schema(&mut transaction, &key_ref, schema).await?)
    } else {
        None
    };
    compare_key_schema_preview(request.schema_digest.as_deref(), assignment.as_ref())?;
    let plaintext = if let Some(assignment) = &assignment {
        KeySchemaRepository::prepare_value(assignment, &request.value, request.encrypted)?
    } else {
        request.value.clone()
    };

    // Validate plaintext before encryption, under the selected assignment.
    let (value, encryption_key_hash) = if request.encrypted {
        let encryption_key = state
            .config
            .security
            .encryption_key
            .as_ref()
            .ok_or_else(|| {
                ApiError::BadRequest(
                    "Cannot encrypt: encryption key not configured on server".to_string(),
                )
            })?;

        let encrypted_value = attune_common::crypto::encrypt_json(&plaintext, encryption_key)
            .map_err(|e| {
                tracing::error!("Failed to encrypt key value: {}", e);
                ApiError::InternalServerError(format!("Failed to encrypt value: {}", e))
            })?;

        let key_hash = attune_common::crypto::hash_encryption_key(encryption_key);

        (encrypted_value, Some(key_hash))
    } else {
        // Store in plaintext (not recommended for sensitive data)
        (plaintext.clone(), None)
    };

    // Create key input
    let key_input = CreateKeyInput {
        local_ref: request.local_ref,
        owner_type: request.owner_type,
        owner_identity: resolved_owner.owner_identity,
        owner_pack: resolved_owner.owner_pack,
        owner_pack_ref: resolved_owner.owner_pack_ref,
        owner_action: resolved_owner.owner_action,
        owner_action_ref: resolved_owner.owner_action_ref,
        owner_sensor: resolved_owner.owner_sensor,
        owner_sensor_ref: resolved_owner.owner_sensor_ref,
        name: request.name,
        encrypted: request.encrypted,
        encryption_key_hash,
        value,
    };

    let mut key = KeyRepository::create(&mut *transaction, key_input).await?;
    KeySchemaRepository::set(&mut transaction, key.id, assignment.as_ref()).await?;
    if let Some(authority) = &recorded {
        authority.ensure_fresh_on(&mut transaction).await?;
    }
    transaction.commit().await?;

    // Return decrypted value in response
    if key.encrypted {
        let encryption_key = state.config.security.encryption_key.as_ref().unwrap();
        key.value =
            attune_common::crypto::decrypt_json(&key.value, encryption_key).map_err(|e| {
                tracing::error!("Failed to decrypt newly created key: {}", e);
                ApiError::InternalServerError(format!("Failed to decrypt value: {}", e))
            })?;
    }

    emit_key_audit(
        &state,
        &user,
        event_type::secret::KEY_CREATED,
        AuditOutcome::Success,
        &key,
        serde_json::json!({
            "encrypted": key.encrypted,
            "owner_type": key.owner_type,
            "owner_ref": key_owner_ref(
                key.owner_type,
                key.owner.as_deref(),
                key.owner_pack_ref.as_deref(),
                key.owner_action_ref.as_deref(),
                key.owner_sensor_ref.as_deref(),
            ),
            "value": "***",
        }),
    );

    let response = ApiResponse::with_message(
        KeyResponse::from(key).with_schema(assignment.as_ref())?,
        "Key created successfully",
    );

    Ok((StatusCode::CREATED, Json(response)))
}

/// Update an existing key/secret
#[utoipa::path(
    put,
    path = "/api/v1/keys/{ref}",
    tag = "secrets",
    params(
        ("ref" = String, Path, description = "Key reference identifier")
    ),
    request_body = UpdateKeyRequest,
    responses(
        (status = 200, description = "Key updated successfully", body = inline(ApiResponse<KeyResponse>)),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Key not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_key(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(key_ref): Path<String>,
    Json(mut request): Json<UpdateKeyRequest>,
) -> ApiResult<impl IntoResponse> {
    require_key_mutation_token(&user)?;
    // Validate request
    request.validate()?;
    let delegated = delegated_key_grants(&state, &user).await?;

    // Verify key exists
    let mut transaction = state.db.begin().await?;
    SchemaTypeRepository::lock_selection(&mut transaction).await?;
    let recorded = user.0.recorded_runtime_authority(&mut transaction).await?;
    let existing = KeyRepository::find_by_ref_for_update(&mut transaction, &key_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Key '{}' not found", key_ref)))?;
    let (identity_id, grants) = match &recorded {
        Some(authority) => (authority.authority().owner.sponsor_identity_id, Vec::new()),
        None => delegated.ok_or_else(missing_key_authority)?,
    };
    let target = {
        Some(
            attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
                &mut transaction,
                attune_common::repositories::runtime_target::RuntimeTarget::Key(existing.id),
                identity_id,
            )
            .await?
            .ok_or_else(|| ApiError::NotFound("Key is not available".into()))?,
        )
    };
    let allows = |action| match (&recorded, &target) {
        (Some(authority), Some(target)) => authority.allows_new_effect(target, action),
        _ => key_action_allowed_in_pack(
            &grants,
            action,
            identity_id,
            &existing,
            target.as_ref().and_then(|target| target.pack_id()),
        ),
    };
    if !allows(Action::Update) {
        return Err(ApiError::Forbidden(
            "Insufficient permissions: keys:update".to_string(),
        ));
    }

    let stored_assignment = KeySchemaRepository::find(&mut *transaction, existing.id).await?;
    if let Some(authority) = &recorded {
        attune_common::repositories::runtime_read::RuntimeReadRepository::require_untainted(
            &mut transaction,
            authority.credential_binding().admission_id,
        )
        .await?;
    }
    let schema_changed = request.value_schema.as_ref().is_some_and(|schema| {
        if schema.is_null() {
            stored_assignment.is_some()
        } else {
            stored_assignment
                .as_ref()
                .is_none_or(|stored| stored.authored_schema != *schema)
        }
    });
    let assignment = if request
        .value_schema
        .as_ref()
        .is_some_and(serde_json::Value::is_null)
    {
        if request.rebind_schema {
            return Err(ApiError::BadRequest(
                "Cannot rebind a removed schema".into(),
            ));
        }
        None
    } else if schema_changed || request.rebind_schema {
        let schema = request
            .value_schema
            .as_ref()
            .or_else(|| {
                stored_assignment
                    .as_ref()
                    .map(|assignment| &assignment.authored_schema)
            })
            .ok_or_else(|| ApiError::BadRequest("Rebinding requires a value schema".into()))?;
        Some(assign_key_schema(&mut transaction, &existing.r#ref, schema).await?)
    } else {
        stored_assignment.clone()
    };
    compare_key_schema_preview(request.schema_digest.as_deref(), assignment.as_ref())?;
    let encryption_changed = request
        .encrypted
        .is_some_and(|encrypted| encrypted != existing.encrypted);
    let needs_value = request.value.is_some()
        || encryption_changed
        || (schema_changed || request.rebind_schema) && assignment.is_some();
    if needs_value {
        let plaintext = if let Some(value) = request.value.take() {
            value
        } else {
            if !allows(Action::Read) || existing.encrypted && !allows(Action::Decrypt) {
                return Err(ApiError::Forbidden("Inspecting preserved Key material for schema/encryption changes requires read and applicable decrypt authority".into()));
            }
            if existing.encrypted {
                let encryption_key =
                    state
                        .config
                        .security
                        .encryption_key
                        .as_deref()
                        .ok_or_else(|| {
                            ApiError::InternalServerError("Encryption key is not configured".into())
                        })?;
                attune_common::crypto::decrypt_json(&existing.value, encryption_key)?
            } else {
                existing.value.clone()
            }
        };
        let plaintext = if let Some(assignment) = &assignment {
            KeySchemaRepository::prepare_value(
                assignment,
                &plaintext,
                request.encrypted.unwrap_or(existing.encrypted),
            )?
        } else {
            plaintext
        };
        request.value = Some(plaintext);
    }

    // Handle value update with encryption
    let (value, encrypted, encryption_key_hash) = if let Some(new_value) = request.value {
        let should_encrypt = request.encrypted.unwrap_or(existing.encrypted);

        if should_encrypt {
            let encryption_key =
                state
                    .config
                    .security
                    .encryption_key
                    .as_ref()
                    .ok_or_else(|| {
                        ApiError::BadRequest(
                            "Cannot encrypt: encryption key not configured on server".to_string(),
                        )
                    })?;

            let encrypted_value = attune_common::crypto::encrypt_json(&new_value, encryption_key)
                .map_err(|e| {
                tracing::error!("Failed to encrypt key value: {}", e);
                ApiError::InternalServerError(format!("Failed to encrypt value: {}", e))
            })?;

            let key_hash = attune_common::crypto::hash_encryption_key(encryption_key);

            (Some(encrypted_value), Some(should_encrypt), Some(key_hash))
        } else {
            (Some(new_value), Some(false), None)
        }
    } else {
        (None, None, None)
    };

    // Create update input
    let update_input = UpdateKeyInput {
        name: request.name,
        value,
        encrypted,
        encryption_key_hash,
    };

    let mut updated_key =
        KeyRepository::update(&mut *transaction, existing.id, update_input).await?;
    if schema_changed || request.rebind_schema {
        KeySchemaRepository::set(&mut transaction, existing.id, assignment.as_ref()).await?;
    }
    let value_updated = updated_key.value != existing.value;

    let (can_read, can_decrypt) = if let Some(authority) = &recorded {
        let target = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
            &mut transaction,
            attune_common::repositories::runtime_target::RuntimeTarget::Key(updated_key.id),
            identity_id,
        )
        .await?
        .ok_or_else(|| ApiError::NotFound("Key is not available".into()))?;
        (
            authority.allows_target(&target, Action::Read),
            authority.allows_target(&target, Action::Decrypt),
        )
    } else {
        (
            key_action_allowed_in_pack(
                &grants,
                Action::Read,
                identity_id,
                &updated_key,
                target.as_ref().and_then(|target| target.pack_id()),
            ),
            key_action_allowed_in_pack(
                &grants,
                Action::Decrypt,
                identity_id,
                &updated_key,
                target.as_ref().and_then(|target| target.pack_id()),
            ),
        )
    };
    if can_read && (!updated_key.encrypted || can_decrypt) {
        if let Some(authority) = &recorded {
            attune_common::repositories::execution_key_read::ExecutionKeyReadRepository::record(
                &mut transaction,
                authority,
                &attune_common::key_access::key_origin(&updated_key),
            )
            .await?;
        }
    }
    if !can_read || updated_key.encrypted && !can_decrypt {
        updated_key.value = serde_json::Value::Null;
    } else if updated_key.encrypted {
        let encryption_key = state
            .config
            .security
            .encryption_key
            .as_ref()
            .ok_or_else(|| {
                ApiError::InternalServerError("Encryption key not configured on server".to_string())
            })?;

        updated_key.value = attune_common::crypto::decrypt_json(&updated_key.value, encryption_key)
            .map_err(|e| {
                tracing::error!("Failed to decrypt updated key '{}': {}", key_ref, e);
                ApiError::InternalServerError(format!("Failed to decrypt value: {}", e))
            })?;
    }
    if let Some(authority) = &recorded {
        authority.ensure_fresh_on(&mut transaction).await?;
    }
    transaction.commit().await?;

    emit_key_audit(
        &state,
        &user,
        event_type::secret::KEY_UPDATED,
        AuditOutcome::Success,
        &updated_key,
        serde_json::json!({
            "encrypted": updated_key.encrypted,
            "owner_type": updated_key.owner_type,
            "owner_ref": key_owner_ref(
                updated_key.owner_type,
                updated_key.owner.as_deref(),
                updated_key.owner_pack_ref.as_deref(),
                updated_key.owner_action_ref.as_deref(),
                updated_key.owner_sensor_ref.as_deref(),
            ),
            "value_updated": value_updated,
            "schema_changed": schema_changed || request.rebind_schema,
            "previous_schema_binding_digest": stored_assignment.as_ref().map(|assignment| &assignment.contract.binding_digest),
            "schema_binding_digest": assignment.as_ref().map(|assignment| &assignment.contract.binding_digest),
            "value": "***",
        }),
    );

    let response = ApiResponse::with_message(
        KeyResponse::from(updated_key).with_schema(assignment.as_ref())?,
        "Key updated successfully",
    );

    Ok((StatusCode::OK, Json(response)))
}

/// Delete a key/secret
#[utoipa::path(
    delete,
    path = "/api/v1/keys/{ref}",
    tag = "secrets",
    params(
        ("ref" = String, Path, description = "Key reference identifier")
    ),
    responses(
        (status = 200, description = "Key deleted successfully", body = SuccessResponse),
        (status = 404, description = "Key not found")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_key(
    user: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(key_ref): Path<String>,
) -> ApiResult<impl IntoResponse> {
    require_key_mutation_token(&user)?;
    let delegated = delegated_key_grants(&state, &user).await?;
    let mut transaction = state.db.begin().await?;
    SchemaTypeRepository::lock_selection(&mut transaction).await?;
    let recorded = user.0.recorded_runtime_authority(&mut transaction).await?;
    // Verify key exists
    let key = KeyRepository::find_by_ref_for_update(&mut transaction, &key_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Key '{}' not found", key_ref)))?;

    if let Some(authority) = &recorded {
        let target = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
            &mut transaction,
            attune_common::repositories::runtime_target::RuntimeTarget::Key(key.id),
            authority.authority().owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(|| ApiError::NotFound("Key is not available".into()))?;
        if !authority.allows_new_effect(&target, Action::Delete) {
            return Err(ApiError::Forbidden(
                "Insufficient recorded authority: keys:delete".into(),
            ));
        }
    } else if matches!(
        user.0.claims.token_type,
        TokenType::Access | TokenType::Execution
    ) {
        let (identity_id, grants) = delegated.ok_or_else(missing_key_authority)?;
        let target = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve(
            &mut transaction,
            attune_common::repositories::runtime_target::RuntimeTarget::Key(key.id),
            identity_id,
        )
        .await?
        .ok_or_else(|| ApiError::NotFound("Key is not available".into()))?;
        if !key_action_allowed_in_pack(&grants, Action::Delete, identity_id, &key, target.pack_id())
        {
            return Err(ApiError::Forbidden(
                "Insufficient permissions: keys:delete".to_string(),
            ));
        }
    }

    // Delete the key
    let deleted = KeyRepository::delete(&mut *transaction, key.id).await?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Key '{}' not found", key_ref)));
    }
    if let Some(authority) = &recorded {
        authority.ensure_fresh_on(&mut transaction).await?;
    }
    transaction.commit().await?;

    let response = SuccessResponse::new("Key deleted successfully");

    emit_key_audit(
        &state,
        &user,
        event_type::secret::KEY_DELETED,
        AuditOutcome::Success,
        &key,
        serde_json::json!({
            "encrypted": key.encrypted,
            "owner_type": key.owner_type,
            "owner_ref": key_owner_ref(
                key.owner_type,
                key.owner.as_deref(),
                key.owner_pack_ref.as_deref(),
                key.owner_action_ref.as_deref(),
                key.owner_sensor_ref.as_deref(),
            ),
        }),
    );

    Ok((StatusCode::OK, Json(response)))
}

/// Register key/secret routes
pub fn routes() -> Router<Arc<AppState>> {
    use axum::handler::Handler;
    Router::new()
        .route(
            "/keys/{ref}/sign-jwt",
            post(sign_key_jwt.layer(axum::Extension(
                crate::auth::middleware::RecordedRuntimeConsumer::KeySigning,
            ))),
        )
        .route(
            "/keys",
            get(list_keys.layer(axum::Extension(
                crate::auth::middleware::RecordedRuntimeConsumer::KeyList,
            )))
            .post(create_key.layer(axum::Extension(
                crate::auth::middleware::RecordedRuntimeConsumer::KeyCreate,
            ))),
        )
        .route(
            "/keys/{ref}",
            get(get_key.layer(axum::Extension(
                crate::auth::middleware::RecordedRuntimeConsumer::KeyRead,
            )))
            .put(update_key.layer(axum::Extension(
                crate::auth::middleware::RecordedRuntimeConsumer::KeyUpdate,
            )))
            .delete(delete_key.layer(axum::Extension(
                crate::auth::middleware::RecordedRuntimeConsumer::KeyDelete,
            ))),
        )
}

fn key_authorization_context(identity_id: i64, key: &Key) -> AuthorizationContext {
    let mut ctx = AuthorizationContext::new(identity_id);
    ctx.target_id = Some(key.id);
    ctx.target_ref = Some(key.r#ref.clone());
    ctx.owner_identity_id = key.owner_identity;
    ctx.owner_type = Some(key.owner_type);
    ctx.owner_ref = key_owner_ref(
        key.owner_type,
        key.owner.as_deref(),
        key.owner_pack_ref.as_deref(),
        key.owner_action_ref.as_deref(),
        key.owner_sensor_ref.as_deref(),
    );
    ctx.encrypted = Some(key.encrypted);
    ctx
}

fn key_action_allowed(grants: &[Grant], action: Action, identity_id: i64, key: &Key) -> bool {
    key_action_allowed_in_pack(grants, action, identity_id, key, None)
}

fn key_action_allowed_in_pack(
    grants: &[Grant],
    action: Action,
    identity_id: i64,
    key: &Key,
    managing_pack_id: Option<i64>,
) -> bool {
    let mut ctx = key_authorization_context(identity_id, key);
    ctx.managing_pack_id = managing_pack_id;
    if key.owner_type == OwnerType::Identity && key.owner_identity != Some(identity_id) {
        return constrained_key_grant_allows(grants, action, &ctx);
    }

    AuthorizationService::is_allowed(grants, Resource::Keys, action, &ctx)
}

fn constrained_key_grant_allows(
    grants: &[Grant],
    action: Action,
    ctx: &AuthorizationContext,
) -> bool {
    grants.iter().any(|grant| {
        let Some(constraints) = &grant.constraints else {
            return false;
        };
        let owner_scoped = constraints.owner.is_some()
            || constraints.owner_types.is_some()
            || constraints.owner_refs.is_some()
            || constraints.refs.is_some()
            || constraints.ids.is_some();
        grant.resource == Resource::Keys
            && grant.actions.contains(&action)
            && owner_scoped
            && grant.allows(Resource::Keys, action, ctx)
    })
}

#[derive(Default)]
struct ResolvedKeyOwner {
    owner_identity: Option<i64>,
    owner_pack: Option<i64>,
    owner_pack_ref: Option<String>,
    owner_action: Option<i64>,
    owner_action_ref: Option<String>,
    owner_sensor: Option<i64>,
    owner_sensor_ref: Option<String>,
}

fn requested_key_owner_ref(request: &CreateKeyRequest) -> ApiResult<Option<String>> {
    let no_component_refs = request.owner_pack_ref.is_none()
        && request.owner_action_ref.is_none()
        && request.owner_sensor_ref.is_none();

    match request.owner_type {
        OwnerType::System if request.owner_identity_login.is_none() && no_component_refs => {
            Ok(None)
        }
        OwnerType::Identity if no_component_refs => request
            .owner_identity_login
            .clone()
            .map(Some)
            .ok_or_else(|| {
                ApiError::BadRequest(
                    "owner_identity_login is required for identity keys".to_string(),
                )
            }),
        OwnerType::Pack
            if request.owner_identity_login.is_none()
                && request.owner_action_ref.is_none()
                && request.owner_sensor_ref.is_none() =>
        {
            request.owner_pack_ref.clone().map(Some).ok_or_else(|| {
                ApiError::BadRequest("owner_pack_ref is required for pack keys".to_string())
            })
        }
        OwnerType::Action
            if request.owner_identity_login.is_none()
                && request.owner_pack_ref.is_none()
                && request.owner_sensor_ref.is_none() =>
        {
            request.owner_action_ref.clone().map(Some).ok_or_else(|| {
                ApiError::BadRequest("owner_action_ref is required for action keys".to_string())
            })
        }
        OwnerType::Sensor
            if request.owner_identity_login.is_none()
                && request.owner_pack_ref.is_none()
                && request.owner_action_ref.is_none() =>
        {
            request.owner_sensor_ref.clone().map(Some).ok_or_else(|| {
                ApiError::BadRequest("owner_sensor_ref is required for sensor keys".to_string())
            })
        }
        _ => Err(ApiError::BadRequest(format!(
            "Owner fields do not match owner_type '{}'",
            owner_type_name(request.owner_type)
        ))),
    }
}

async fn resolve_key_owner(
    connection: &mut sqlx::PgConnection,
    request: &CreateKeyRequest,
) -> ApiResult<ResolvedKeyOwner> {
    let no_component_refs = request.owner_pack_ref.is_none()
        && request.owner_action_ref.is_none()
        && request.owner_sensor_ref.is_none();

    match request.owner_type {
        OwnerType::System if request.owner_identity_login.is_none() && no_component_refs => {
            Ok(ResolvedKeyOwner::default())
        }
        OwnerType::Identity if no_component_refs => {
            let identity_login = request.owner_identity_login.as_deref().ok_or_else(|| {
                ApiError::BadRequest(
                    "owner_identity_login is required for identity keys".to_string(),
                )
            })?;
            let identity = IdentityRepository::find_by_login(&mut *connection, identity_login)
                .await?
                .ok_or_else(|| {
                    ApiError::BadRequest(format!(
                        "Identity with login '{}' not found",
                        identity_login
                    ))
                })?;
            Ok(ResolvedKeyOwner {
                owner_identity: Some(identity.id),
                ..Default::default()
            })
        }
        OwnerType::Pack if request.owner_identity_login.is_none() => {
            let owner_ref = request.owner_pack_ref.as_deref().ok_or_else(|| {
                ApiError::BadRequest("owner_pack_ref is required for pack keys".to_string())
            })?;
            if request.owner_action_ref.is_some() || request.owner_sensor_ref.is_some() {
                return Err(ApiError::BadRequest(
                    "pack keys cannot include action or sensor owners".to_string(),
                ));
            }
            let owner = PackRepository::find_by_ref(&mut *connection, owner_ref)
                .await?
                .ok_or_else(|| {
                    ApiError::BadRequest(format!("Pack with ref '{}' not found", owner_ref))
                })?;
            Ok(ResolvedKeyOwner {
                owner_pack: Some(owner.id),
                owner_pack_ref: Some(owner.r#ref),
                ..Default::default()
            })
        }
        OwnerType::Action if request.owner_identity_login.is_none() => {
            let owner_ref = request.owner_action_ref.as_deref().ok_or_else(|| {
                ApiError::BadRequest("owner_action_ref is required for action keys".to_string())
            })?;
            if request.owner_pack_ref.is_some() || request.owner_sensor_ref.is_some() {
                return Err(ApiError::BadRequest(
                    "action keys cannot include pack or sensor owners".to_string(),
                ));
            }
            let owner = ActionRepository::find_by_ref(&mut *connection, owner_ref)
                .await?
                .ok_or_else(|| {
                    ApiError::BadRequest(format!("Action with ref '{}' not found", owner_ref))
                })?;
            Ok(ResolvedKeyOwner {
                owner_action: Some(owner.id),
                owner_action_ref: Some(owner.r#ref),
                ..Default::default()
            })
        }
        OwnerType::Sensor if request.owner_identity_login.is_none() => {
            let owner_ref = request.owner_sensor_ref.as_deref().ok_or_else(|| {
                ApiError::BadRequest("owner_sensor_ref is required for sensor keys".to_string())
            })?;
            if request.owner_pack_ref.is_some() || request.owner_action_ref.is_some() {
                return Err(ApiError::BadRequest(
                    "sensor keys cannot include pack or action owners".to_string(),
                ));
            }
            let owner = SensorRepository::find_by_ref(&mut *connection, owner_ref)
                .await?
                .ok_or_else(|| {
                    ApiError::BadRequest(format!("Sensor with ref '{}' not found", owner_ref))
                })?;
            Ok(ResolvedKeyOwner {
                owner_sensor: Some(owner.id),
                owner_sensor_ref: Some(owner.r#ref),
                ..Default::default()
            })
        }
        _ => Err(ApiError::BadRequest(format!(
            "Owner fields do not match owner_type '{}'",
            owner_type_name(request.owner_type)
        ))),
    }
}

fn owner_type_name(owner_type: OwnerType) -> &'static str {
    match owner_type {
        OwnerType::System => "system",
        OwnerType::Identity => "identity",
        OwnerType::Pack => "pack",
        OwnerType::Action => "action",
        OwnerType::Sensor => "sensor",
    }
}

fn key_owner_ref(
    owner_type: OwnerType,
    owner: Option<&str>,
    owner_pack_ref: Option<&str>,
    owner_action_ref: Option<&str>,
    owner_sensor_ref: Option<&str>,
) -> Option<String> {
    match owner_type {
        OwnerType::Pack => owner_pack_ref.map(str::to_string),
        OwnerType::Action => owner_action_ref.map(str::to_string),
        OwnerType::Sensor => owner_sensor_ref.map(str::to_string),
        _ => owner.map(str::to_string),
    }
}

fn emit_key_audit(
    state: &Arc<AppState>,
    user: &RequireAuth,
    event_type: &'static str,
    outcome: AuditOutcome,
    key: &Key,
    details: serde_json::Value,
) {
    state.audit_emitter.emit(build_key_audit_event(
        user, event_type, outcome, key, details,
    ));
}

fn build_key_audit_event(
    user: &RequireAuth,
    event_type: &'static str,
    outcome: AuditOutcome,
    key: &Key,
    details: serde_json::Value,
) -> PendingAuditEvent {
    let mut builder = AuditEventBuilder::new(AuditCategory::Secret, event_type, outcome)
        .resource("key")
        .resource_id(key.id)
        .resource_ref(key.r#ref.clone())
        .with_details(user.0.with_recorded_audit_context(details));

    if let Ok(identity_id) = user.0.identity_id() {
        builder = builder.actor_identity(identity_id);
    }
    builder = builder
        .actor_login(user.0.login().to_string())
        .actor_token_type(format!("{:?}", user.0.claims.token_type).to_lowercase());

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        jwt::{Claims, TokenType},
        middleware::AuthenticatedUser,
    };
    use chrono::Utc;

    fn test_user() -> RequireAuth {
        RequireAuth(AuthenticatedUser {
            claims: Claims {
                sub: "42".to_string(),
                login: "secret-reader@example.test".to_string(),
                iat: 1,
                exp: 999_999,
                token_type: TokenType::Access,
                scope: None,
                metadata: None,
            },
        })
    }

    fn test_key() -> Key {
        let now = Utc::now();
        Key {
            id: 123,
            r#ref: "finance.api_token".to_string(),
            local_ref: "api_token".to_string(),
            owner_type: OwnerType::Identity,
            owner: Some("finance".to_string()),
            owner_identity: Some(42),
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Finance API token".to_string(),
            encrypted: true,
            encryption_key_hash: Some("sha256:redacted".to_string()),
            value: serde_json::json!("super-secret-token"),
            created: now,
            updated: now,
        }
    }

    fn create_key_request(owner_type: OwnerType) -> CreateKeyRequest {
        CreateKeyRequest {
            local_ref: "api_token".to_string(),
            owner_type,
            owner_identity_login: None,
            owner_pack_ref: None,
            owner_action_ref: None,
            owner_sensor_ref: None,
            name: "API token".to_string(),
            value: serde_json::json!("secret"),
            encrypted: true,
            value_schema: None,
            schema_digest: None,
        }
    }

    #[test]
    fn requested_owner_ref_uses_typed_owner_without_database_lookup() {
        let mut request = create_key_request(OwnerType::Pack);
        request.owner_pack_ref = Some("core".to_string());

        assert_eq!(
            requested_key_owner_ref(&request).unwrap().as_deref(),
            Some("core")
        );
    }

    #[test]
    fn requested_owner_ref_rejects_mixed_owner_fields() {
        let mut request = create_key_request(OwnerType::Action);
        request.owner_action_ref = Some("core.echo".to_string());
        request.owner_pack_ref = Some("core".to_string());

        assert!(matches!(
            requested_key_owner_ref(&request),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn key_decrypt_audit_event_redacts_secret_value() {
        let event = build_key_audit_event(
            &test_user(),
            event_type::secret::KEY_DECRYPTED,
            AuditOutcome::Success,
            &test_key(),
            serde_json::json!({
                "encrypted": true,
                "decrypted": true,
                "owner_type": OwnerType::Identity,
                "owner_ref": "finance",
                "value": "***",
            }),
        );

        assert_eq!(event.category, AuditCategory::Secret);
        assert_eq!(event.event_type, event_type::secret::KEY_DECRYPTED);
        assert_eq!(event.outcome, AuditOutcome::Success);
        assert_eq!(event.actor_identity, Some(42));
        assert_eq!(event.resource_type.as_deref(), Some("key"));
        assert_eq!(event.resource_id, Some(123));
        assert_eq!(event.resource_ref.as_deref(), Some("finance.api_token"));

        let serialized = serde_json::to_string(&event.details.expect("details")).unwrap();
        assert!(serialized.contains("\"value\":\"***\""));
        assert!(!serialized.contains("super-secret-token"));
        assert!(!serialized.contains("sha256:redacted"));
    }

    // --- KeyGrantFilter compilation / SQL-predicate parity tests ---
    //
    // These tests validate that `compile_key_read_grant_filters` produces
    // filters whose semantics (simulated here in Rust, mirroring the SQL
    // built by `push_grant_clause`) match `key_action_allowed`'s in-memory
    // decision exactly, across representative grant/key combinations.

    use attune_common::rbac::{ExecutionScopeConstraint, GrantConstraints, OwnerConstraint};

    fn key_fixture(
        id: i64,
        r#ref: &str,
        owner_type: OwnerType,
        owner: Option<&str>,
        owner_identity: Option<i64>,
        owner_pack_ref: Option<&str>,
        encrypted: bool,
    ) -> Key {
        let now = Utc::now();
        Key {
            id,
            r#ref: r#ref.to_string(),
            local_ref: r#ref.rsplit('.').next().unwrap().to_string(),
            owner_type,
            owner: owner.map(str::to_string),
            owner_identity,
            owner_pack: owner_pack_ref.map(|_| 1),
            owner_pack_ref: owner_pack_ref.map(str::to_string),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "test key".to_string(),
            encrypted,
            encryption_key_hash: None,
            value: serde_json::json!(null),
            created: now,
            updated: now,
        }
    }

    fn read_grant(constraints: Option<GrantConstraints>) -> Grant {
        Grant {
            resource: Resource::Keys,
            actions: vec![Action::Read],
            constraints,
        }
    }

    /// Simulates the SQL predicate that `push_grant_clause` builds for a
    /// single compiled filter, operating purely in Rust for test purposes.
    fn grant_filter_matches(filter: &KeyGrantFilter, identity_id: i64, key: &Key) -> bool {
        if let Some(owner_types) = &filter.owner_types {
            if !owner_types.contains(&key.owner_type) {
                return false;
            }
        }
        if let Some(owner) = filter.owner {
            let owner_match = match owner {
                OwnerConstraint::SelfOnly => key.owner_identity == Some(identity_id),
                OwnerConstraint::Any => true,
                OwnerConstraint::None => key.owner_identity.is_none(),
            };
            if !owner_match {
                return false;
            }
        }
        if let Some(owner_refs) = &filter.owner_refs {
            let owner_ref = key_owner_ref(
                key.owner_type,
                key.owner.as_deref(),
                key.owner_pack_ref.as_deref(),
                key.owner_action_ref.as_deref(),
                key.owner_sensor_ref.as_deref(),
            );
            match owner_ref {
                Some(owner_ref) if owner_refs.contains(&owner_ref) => {}
                _ => return false,
            }
        }
        if let Some(refs) = &filter.refs {
            if !refs.contains(&key.r#ref) {
                return false;
            }
        }
        if let Some(ids) = &filter.ids {
            if !ids.contains(&key.id) {
                return false;
            }
        }
        if let Some(encrypted) = filter.encrypted {
            if encrypted != key.encrypted {
                return false;
            }
        }
        if !filter.owner_scoped
            && key.owner_type == OwnerType::Identity
            && key.owner_identity != Some(identity_id)
        {
            return false;
        }
        true
    }

    fn filters_allow(filters: &[KeyGrantFilter], identity_id: i64, key: &Key) -> bool {
        filters
            .iter()
            .any(|f| grant_filter_matches(f, identity_id, key))
    }

    fn assert_parity(grants: &[Grant], identity_id: i64, key: &Key, case: &str) {
        let compiled = compile_key_read_grant_filters(grants);
        let expected = key_action_allowed(grants, Action::Read, identity_id, key);
        let actual = filters_allow(&compiled, identity_id, key);
        assert_eq!(
            actual, expected,
            "case `{case}` mismatch: compiled filters gave {actual}, key_action_allowed gave {expected}"
        );
    }

    #[test]
    fn local_key_iam_needs_structural_context_and_keeps_narrower_selectors() {
        let key = key_fixture(
            8,
            "pack.local.secret",
            OwnerType::Pack,
            Some("local"),
            None,
            Some("local"),
            false,
        );
        let grants = vec![read_grant(Some(GrantConstraints {
            managing_pack_id: Some(17),
            ids: Some(vec![8]),
            refs: Some(vec![key.r#ref.clone()]),
            ..Default::default()
        }))];
        assert!(key_action_allowed_in_pack(
            &grants,
            Action::Read,
            42,
            &key,
            Some(17)
        ));
        for owner in [None, Some(18)] {
            assert!(!key_action_allowed_in_pack(
                &grants,
                Action::Read,
                42,
                &key,
                owner
            ));
        }
        let mut other = key.clone();
        other.id = 9;
        assert!(!key_action_allowed_in_pack(
            &grants,
            Action::Read,
            42,
            &other,
            Some(17)
        ));
        other = key.clone();
        other.owner_type = OwnerType::System;
        assert!(!key_action_allowed_in_pack(
            &grants,
            Action::Read,
            42,
            &other,
            Some(17)
        ));
    }

    #[test]
    fn unconstrained_grant_matches_everything_except_other_identity_owned_keys() {
        let identity_id = 42;
        let grants = vec![read_grant(None)];

        let own_identity_key = key_fixture(
            1,
            "me.token",
            OwnerType::Identity,
            None,
            Some(42),
            None,
            true,
        );
        let other_identity_key = key_fixture(
            2,
            "other.token",
            OwnerType::Identity,
            None,
            Some(99),
            None,
            true,
        );
        let pack_key = key_fixture(
            3,
            "pack.token",
            OwnerType::Pack,
            None,
            None,
            Some("some_pack"),
            false,
        );

        assert_parity(
            &grants,
            identity_id,
            &own_identity_key,
            "unconstrained/own-identity",
        );
        assert_parity(
            &grants,
            identity_id,
            &other_identity_key,
            "unconstrained/other-identity",
        );
        assert_parity(&grants, identity_id, &pack_key, "unconstrained/pack");
    }

    #[test]
    fn owner_types_constraint_matches_key_action_allowed() {
        let identity_id = 42;
        let grants = vec![read_grant(Some(GrantConstraints {
            owner_types: Some(vec![OwnerType::System, OwnerType::Pack]),
            ..Default::default()
        }))];

        let system_key = key_fixture(1, "sys.token", OwnerType::System, None, None, None, false);
        let pack_key = key_fixture(
            2,
            "pack.token",
            OwnerType::Pack,
            None,
            None,
            Some("p"),
            false,
        );
        let identity_key = key_fixture(
            3,
            "id.token",
            OwnerType::Identity,
            None,
            Some(99),
            None,
            false,
        );

        assert_parity(&grants, identity_id, &system_key, "owner_types/system");
        assert_parity(&grants, identity_id, &pack_key, "owner_types/pack");
        assert_parity(
            &grants,
            identity_id,
            &identity_key,
            "owner_types/identity-excluded",
        );
    }

    #[test]
    fn owner_refs_constraint_uses_owner_type_dependent_column() {
        let identity_id = 42;
        let grants = vec![read_grant(Some(GrantConstraints {
            owner_types: Some(vec![OwnerType::Pack]),
            owner_refs: Some(vec!["allowed_pack".to_string()]),
            ..Default::default()
        }))];

        let allowed = key_fixture(
            1,
            "allowed.token",
            OwnerType::Pack,
            None,
            None,
            Some("allowed_pack"),
            false,
        );
        let other = key_fixture(
            2,
            "other.token",
            OwnerType::Pack,
            None,
            None,
            Some("other_pack"),
            false,
        );

        assert_parity(&grants, identity_id, &allowed, "owner_refs/match");
        assert_parity(&grants, identity_id, &other, "owner_refs/no-match");
    }

    #[test]
    fn owner_self_only_constraint_scopes_to_requesting_identity() {
        let identity_id = 42;
        let grants = vec![read_grant(Some(GrantConstraints {
            owner: Some(OwnerConstraint::SelfOnly),
            ..Default::default()
        }))];

        let own_key = key_fixture(
            1,
            "me.token",
            OwnerType::Identity,
            None,
            Some(42),
            None,
            false,
        );
        let other_key = key_fixture(
            2,
            "other.token",
            OwnerType::Identity,
            None,
            Some(99),
            None,
            false,
        );

        assert_parity(&grants, identity_id, &own_key, "owner_self_only/own");
        assert_parity(&grants, identity_id, &other_key, "owner_self_only/other");
    }

    #[test]
    fn ids_and_refs_constraints_are_exact_match() {
        let identity_id = 42;
        let grants = vec![read_grant(Some(GrantConstraints {
            ids: Some(vec![5]),
            ..Default::default()
        }))];
        let matching = key_fixture(5, "a.token", OwnerType::System, None, None, None, false);
        let non_matching = key_fixture(6, "b.token", OwnerType::System, None, None, None, false);
        assert_parity(&grants, identity_id, &matching, "ids/match");
        assert_parity(&grants, identity_id, &non_matching, "ids/no-match");

        let grants = vec![read_grant(Some(GrantConstraints {
            refs: Some(vec!["a.token".to_string()]),
            ..Default::default()
        }))];
        assert_parity(&grants, identity_id, &matching, "refs/match");
        assert_parity(&grants, identity_id, &non_matching, "refs/no-match");
    }

    #[test]
    fn encrypted_constraint_matches_key_action_allowed() {
        let identity_id = 42;
        let grants = vec![read_grant(Some(GrantConstraints {
            encrypted: Some(false),
            ..Default::default()
        }))];
        let plain = key_fixture(1, "a.token", OwnerType::System, None, None, None, false);
        let secret = key_fixture(2, "b.token", OwnerType::System, None, None, None, true);
        assert_parity(
            &grants,
            identity_id,
            &plain,
            "encrypted/false-matches-plain",
        );
        assert_parity(
            &grants,
            identity_id,
            &secret,
            "encrypted/false-excludes-secret",
        );
    }

    #[test]
    fn grants_relying_on_unpopulated_context_fields_are_excluded_or_no_op() {
        // pack_refs is never satisfiable for keys (ctx.pack_ref is never
        // set), so the grant must be compiled away entirely.
        let pack_refs_grant = read_grant(Some(GrantConstraints {
            pack_refs: Some(vec!["some_pack".to_string()]),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&pack_refs_grant).is_none());

        // visibility is never satisfiable either.
        let visibility_grant = read_grant(Some(GrantConstraints {
            visibility: Some(vec![attune_common::models::ArtifactVisibility::Public]),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&visibility_grant).is_none());

        // execution_scope self/descendants can never match (keys have no
        // execution owner in their AuthorizationContext).
        let self_only_grant = read_grant(Some(GrantConstraints {
            execution_scope: Some(ExecutionScopeConstraint::SelfOnly),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&self_only_grant).is_none());

        // execution_scope::Any is a no-op and doesn't exclude the grant.
        let any_scope_grant = read_grant(Some(GrantConstraints {
            execution_scope: Some(ExecutionScopeConstraint::Any),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&any_scope_grant).is_some());

        // Non-empty attributes can never match (identity_attributes is
        // always empty for keys), but an empty attributes map is a no-op.
        let attrs_grant = read_grant(Some(GrantConstraints {
            attributes: Some(std::collections::HashMap::from([(
                "team".to_string(),
                serde_json::json!("platform"),
            )])),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&attrs_grant).is_none());

        let empty_attrs_grant = read_grant(Some(GrantConstraints {
            attributes: Some(std::collections::HashMap::new()),
            ..Default::default()
        }));
        assert!(compile_key_grant_filter(&empty_attrs_grant).is_some());
    }

    #[test]
    fn non_matching_resource_or_action_grants_are_dropped() {
        let wrong_resource = Grant {
            resource: Resource::Artifacts,
            actions: vec![Action::Read],
            constraints: None,
        };
        let wrong_action = Grant {
            resource: Resource::Keys,
            actions: vec![Action::Update],
            constraints: None,
        };
        let compiled = compile_key_read_grant_filters(&[wrong_resource, wrong_action]);
        assert!(compiled.is_empty());
    }

    #[test]
    fn empty_grants_yield_no_visibility() {
        let identity_id = 42;
        let key = key_fixture(1, "a.token", OwnerType::System, None, None, None, false);
        assert_parity(&[], identity_id, &key, "no-grants");
        assert!(compile_key_read_grant_filters(&[]).is_empty());
    }
}
