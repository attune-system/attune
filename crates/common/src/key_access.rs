//! Shared key-row authorization for reference resolution and historical disclosure.

use std::collections::BTreeMap;

use crate::{
    delegation::DelegationAuthority,
    models::{Id, Key, OwnerType},
    rbac::{Action, AuthorizationContext, Grant, Resource},
    repositories::key::KeyRepository,
    secret_provenance::KeyOrigin,
    Error, Result,
};

pub fn key_origin(key: &Key) -> KeyOrigin {
    KeyOrigin {
        key_id: key.id,
        key_ref: key.r#ref.clone(),
        owner_type: key.owner_type,
        owner_identity: key.owner_identity,
        owner_ref: match key.owner_type {
            OwnerType::System => None,
            OwnerType::Identity => key.owner.clone(),
            OwnerType::Pack => key.owner_pack_ref.clone().or_else(|| key.owner.clone()),
            OwnerType::Action => key.owner_action_ref.clone().or_else(|| key.owner.clone()),
            OwnerType::Sensor => key.owner_sensor_ref.clone().or_else(|| key.owner.clone()),
        },
        encrypted: key.encrypted,
    }
}

pub fn origin_context(identity_id: Id, origin: &KeyOrigin) -> AuthorizationContext {
    let mut context = AuthorizationContext::new(identity_id);
    context.target_id = Some(origin.key_id);
    context.target_ref = Some(origin.key_ref.clone());
    context.owner_type = Some(origin.owner_type);
    context.owner_identity_id = origin.owner_identity;
    context.owner_ref = origin.owner_ref.clone();
    context.encrypted = Some(origin.encrypted);
    context
}

pub fn origin_action_allowed(
    grants: &[Grant],
    caller: &AuthorizationContext,
    origin: &KeyOrigin,
    action: Action,
) -> bool {
    let mut context = origin_context(caller.identity_id, origin);
    context.identity_attributes = caller.identity_attributes.clone();
    grants.iter().any(|grant| {
        if origin.owner_type == OwnerType::Identity
            && origin.owner_identity != Some(caller.identity_id)
        {
            let Some(scope) = &grant.constraints else {
                return false;
            };
            if scope.owner.is_none()
                && scope.owner_types.is_none()
                && scope.owner_refs.is_none()
                && scope.refs.is_none()
                && scope.ids.is_none()
            {
                return false;
            }
        }
        grant.allows(Resource::Keys, action, &context)
    })
}

pub struct ResolvedKey {
    pub origin: KeyOrigin,
    pub value: serde_json::Value,
}

pub async fn resolve_explicit_keys<'e, E>(
    executor: E,
    authority: &DelegationAuthority,
    refs: &[String],
    encryption_key: Option<&str>,
) -> Result<BTreeMap<String, ResolvedKey>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres> + 'e,
{
    let mut values = BTreeMap::new();
    let refs: Vec<String> = refs
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let keys = KeyRepository::find_by_refs(executor, &refs).await?;
    if keys.len() != refs.len() {
        return Err(Error::PermissionDenied(
            "Referenced key is not available".into(),
        ));
    }
    for key in keys {
        let origin = key_origin(&key);
        if !authority.key_allows(&origin, Action::Read)
            || key.encrypted && !authority.key_allows(&origin, Action::Decrypt)
        {
            return Err(Error::PermissionDenied(
                "Referenced key value is not authorized".into(),
            ));
        }
        let value = if key.encrypted {
            crate::crypto::decrypt_json(
                &key.value,
                encryption_key
                    .ok_or_else(|| Error::invalid_state("Key decryption is not configured"))?,
            )?
        } else {
            key.value
        };
        values.insert(key.r#ref, ResolvedKey { origin, value });
    }
    Ok(values)
}

pub async fn authorize_parameter_key_origins(
    pool: &sqlx::PgPool,
    identity_id: Option<Id>,
    secrets: &[crate::secret_values::StoredSecretValue],
) -> Result<()> {
    let authority = match identity_id {
        Some(identity_id) => Some(DelegationAuthority::load(pool, identity_id).await?),
        None => None,
    };
    for secret in secrets {
        check_parameter_origins(
            pool,
            authority.as_ref(),
            secret,
            &mut std::collections::BTreeSet::new(),
            0,
        )
        .await?;
    }
    Ok(())
}

fn check_parameter_origins<'a>(
    pool: &'a sqlx::PgPool,
    authority: Option<&'a DelegationAuthority>,
    secret: &'a crate::secret_values::StoredSecretValue,
    visited: &'a mut std::collections::BTreeSet<(String, Id, String)>,
    depth: usize,
) -> futures::future::BoxFuture<'a, Result<()>> {
    Box::pin(async move {
        use crate::secret_provenance::{SecretOrigin, SecretProvenance};
        let denied =
            || Error::PermissionDenied("Execution secret origins are not authorized".into());
        if depth >= 32 || secret.source_kind != "provenance" {
            return Err(denied());
        }
        let provenance: SecretProvenance = secret
            .source_ref
            .as_deref()
            .and_then(|reference| serde_json::from_str(reference).ok())
            .ok_or_else(denied)?;
        if provenance.origins.is_empty() {
            return Err(denied());
        }
        for origin in provenance.origins {
            match origin {
                SecretOrigin::Key(origin) => {
                    let authority = authority.ok_or_else(denied)?;
                    if !authority.key_allows(&origin, Action::Read)
                        || origin.encrypted && !authority.key_allows(&origin, Action::Decrypt)
                    {
                        return Err(denied());
                    }
                }
                SecretOrigin::Entity {
                    entity_type,
                    entity_id,
                    path,
                } => {
                    if !matches!(
                        entity_type.as_str(),
                        crate::secret_values::ENTITY_EXECUTION_CONFIG
                            | crate::secret_values::ENTITY_EXECUTION_RESULT
                    ) || !visited.insert((entity_type.clone(), entity_id, path.clone()))
                    {
                        return Err(denied());
                    }
                    let parents = crate::repositories::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(pool, &entity_type, entity_id).await?;
                    let parent = parents
                        .iter()
                        .find(|parent| parent.json_path == path)
                        .ok_or_else(denied)?;
                    check_parameter_origins(pool, authority, parent, visited, depth + 1).await?;
                    visited.remove(&(entity_type, entity_id, path));
                }
                SecretOrigin::Local { source_kind, .. } => {
                    if !matches!(
                        source_kind.as_str(),
                        "parameter_schema" | "pack_config" | "queue_item" | "trigger_schema"
                    ) {
                        return Err(denied());
                    }
                }
            }
        }
        Ok(())
    })
}
