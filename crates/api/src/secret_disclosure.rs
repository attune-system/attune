//! Per-value disclosure checks against recorded, immutable secret origins.

use std::collections::BTreeSet;
use std::sync::Arc;

use attune_common::{
    crypto,
    rbac::{Action, AuthorizationContext, Grant},
    repositories::{
        execution::ExecutionRepository, execution_secret_value::ExecutionSecretValueRepository,
        FindById,
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{StoredSecretValue, ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT},
};
use serde_json::Value;

use crate::{
    auth::middleware::AuthenticatedUser,
    middleware::{ApiError, ApiResult},
    state::AppState,
};

pub async fn reveal_authorized(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    value: Option<Value>,
    entity_type: &str,
    entity_id: i64,
) -> ApiResult<Option<Value>> {
    let Some(mut value) = value else {
        return Ok(None);
    };
    let identity_id = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid user identity".into()))?;
    let snapshot = state
        .authorization_service()
        .load_snapshot(user)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Secret disclosure requires identity access".into()))?;
    let mut caller = AuthorizationContext::new(identity_id);
    caller.identity_attributes = snapshot.identity_attributes;
    let grants = snapshot.grants;
    let secrets =
        ExecutionSecretValueRepository::find_stored_by_entity(&state.db, entity_type, entity_id)
            .await?;
    for secret in secrets {
        let mut visited = BTreeSet::new();
        if !origin_allows(state, user, &caller, &grants, &secret, &mut visited, 0).await? {
            continue;
        }
        let encryption_key = state
            .config
            .security
            .encryption_key
            .as_ref()
            .ok_or_else(|| {
                ApiError::InternalServerError("Secret disclosure is not configured".into())
            })?;
        if secret
            .encryption_key_hash
            .as_ref()
            .is_some_and(|hash| *hash != crypto::hash_encryption_key(encryption_key))
        {
            return Err(ApiError::InternalServerError(
                "Secret encryption key does not match".into(),
            ));
        }
        let plaintext =
            crypto::decrypt_json(&secret.encrypted_value, encryption_key).map_err(|_| {
                ApiError::InternalServerError("Secret value could not be opened".into())
            })?;
        if let Some(target) = value.pointer_mut(&secret.json_path) {
            *target = plaintext;
        }
    }
    Ok(Some(value))
}

pub async fn authorize_runtime_log(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    execution: &attune_common::models::Execution,
) -> ApiResult<()> {
    let mut secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &state.db,
        ENTITY_EXECUTION_CONFIG,
        execution.id,
    )
    .await?;
    secrets.extend(
        ExecutionSecretValueRepository::find_stored_by_entity(
            &state.db,
            ENTITY_EXECUTION_RESULT,
            execution.id,
        )
        .await?,
    );
    let output_schema = execution
        .executable_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.executable.action.out_schema.as_ref());
    if secrets.is_empty()
        && attune_common::secret_values::secret_paths_from_schema(output_schema).is_empty()
    {
        return Ok(());
    }
    let snapshot = state
        .authorization_service()
        .load_snapshot(user)
        .await?
        .ok_or_else(|| {
            ApiError::Forbidden("Runtime log secret disclosure requires identity access".into())
        })?;
    let mut visibility = crate::routes::executions::ExecutionVisibilityCache::default();
    for action in [Action::Read, Action::Decrypt] {
        crate::routes::executions::authorize_execution_access(
            state,
            user,
            execution,
            action,
            Some(&snapshot),
            &mut visibility,
        )
        .await?;
    }
    let mut caller = AuthorizationContext::new(snapshot.identity_id);
    caller.identity_attributes = snapshot.identity_attributes;
    for secret in secrets {
        if !origin_allows(
            state,
            user,
            &caller,
            &snapshot.grants,
            &secret,
            &mut BTreeSet::new(),
            0,
        )
        .await?
        {
            return Err(ApiError::Forbidden(
                "Runtime log secret origins are not authorized".into(),
            ));
        }
    }
    Ok(())
}

fn origin_allows<'a>(
    state: &'a Arc<AppState>,
    user: &'a AuthenticatedUser,
    caller: &'a AuthorizationContext,
    grants: &'a [Grant],
    secret: &'a StoredSecretValue,
    visited: &'a mut BTreeSet<(String, i64, String)>,
    depth: usize,
) -> futures::future::BoxFuture<'a, ApiResult<bool>> {
    Box::pin(async move {
        if depth >= 32 || secret.source_kind != "provenance" {
            return Ok(false);
        }
        let Some(reference) = &secret.source_ref else {
            return Ok(false);
        };
        let Ok(provenance) = serde_json::from_str::<SecretProvenance>(reference) else {
            return Ok(false);
        };
        if provenance.origins.is_empty() {
            return Ok(false);
        }
        for origin in provenance.origins {
            match origin {
                SecretOrigin::Key(origin) => {
                    if !attune_common::key_access::origin_action_allowed(
                        grants,
                        caller,
                        &origin,
                        Action::Read,
                    ) || origin.encrypted
                        && !attune_common::key_access::origin_action_allowed(
                            grants,
                            caller,
                            &origin,
                            Action::Decrypt,
                        )
                    {
                        return Ok(false);
                    }
                }
                SecretOrigin::Entity {
                    entity_type,
                    entity_id,
                    path,
                } => {
                    if entity_type != ENTITY_EXECUTION_CONFIG
                        && entity_type != ENTITY_EXECUTION_RESULT
                    {
                        return Ok(false);
                    }
                    if !visited.insert((entity_type.clone(), entity_id, path.clone())) {
                        return Ok(false);
                    }
                    let Some(execution) =
                        ExecutionRepository::find_by_id(&state.db, entity_id).await?
                    else {
                        return Ok(false);
                    };
                    let snapshot = crate::authz::AuthorizationSnapshot {
                        identity_id: caller.identity_id,
                        identity_attributes: caller.identity_attributes.clone(),
                        grants: grants.to_vec(),
                    };
                    let mut visibility =
                        crate::routes::executions::ExecutionVisibilityCache::default();
                    for action in [Action::Read, Action::Decrypt] {
                        match crate::routes::executions::authorize_execution_access(
                            state,
                            user,
                            &execution,
                            action,
                            Some(&snapshot),
                            &mut visibility,
                        )
                        .await
                        {
                            Ok(()) => {}
                            Err(ApiError::Forbidden(_) | ApiError::NotFound(_)) => {
                                return Ok(false)
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    let parents = ExecutionSecretValueRepository::find_stored_by_entity(
                        &state.db,
                        &entity_type,
                        entity_id,
                    )
                    .await?;
                    let Some(parent) = parents.iter().find(|parent| parent.json_path == path)
                    else {
                        return Ok(false);
                    };
                    if !origin_allows(state, user, caller, grants, parent, visited, depth + 1)
                        .await?
                    {
                        return Ok(false);
                    }
                    visited.remove(&(entity_type, entity_id, path));
                }
                SecretOrigin::Local { source_kind, .. } => {
                    // Entity-level decrypt was checked by the route. Unknown sources,
                    // including unbound key references, never establish key disclosure.
                    if !matches!(
                        source_kind.as_str(),
                        "parameter_schema" | "pack_config" | "queue_item" | "trigger_schema"
                    ) {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    })
}
