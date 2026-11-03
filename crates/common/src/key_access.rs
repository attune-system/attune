//! Shared key-row authorization for reference resolution and historical disclosure.

use std::collections::BTreeMap;

use crate::{
    delegation::DelegationAuthority,
    models::{Id, Key, OwnerType},
    rbac::{Action, AuthorizationContext, Grant, Resource},
    repositories::{iam_ownership::VerifiedKeyOrigins, FindById},
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
    origin_action_allowed_verified(grants, caller, origin, action, &Default::default())
}

pub fn origin_action_allowed_verified(
    grants: &[Grant],
    caller: &AuthorizationContext,
    origin: &KeyOrigin,
    action: Action,
    ownership: &crate::repositories::iam_ownership::VerifiedKeyOrigins,
) -> bool {
    let mut context = origin_context(caller.identity_id, origin);
    context.managing_pack_id = ownership.managing_pack(origin);
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
    let (keys, ownership) = VerifiedKeyOrigins::keys_by_refs(executor, &refs).await?;
    if keys.len() != refs.len() {
        return Err(Error::PermissionDenied(
            "Referenced key is not available".into(),
        ));
    }
    for key in keys {
        let origin = key_origin(&key);
        if !authority.key_allows_verified(&origin, Action::Read, &ownership)
            || key.encrypted && !authority.key_allows_verified(&origin, Action::Decrypt, &ownership)
        {
            return Err(Error::PermissionDenied(
                "Referenced key value is not authorized".into(),
            ));
        }
        let value = if key.encrypted {
            crate::crypto::keystore::decrypt_json(
                &key.value,
                encryption_key
                    .ok_or_else(|| Error::invalid_state("Key decryption is not configured"))?,
                key.id,
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
    let mut connection = pool.acquire().await?;
    authorize_parameter_key_origins_with_conn(&mut connection, identity_id, secrets).await
}

pub async fn authorize_execution_parameter_key_origins(
    pool: &sqlx::PgPool,
    execution: i64,
    identity: Option<Id>,
    secrets: &[crate::secret_values::StoredSecretValue],
) -> Result<()> {
    let mut transaction = pool.begin().await?;
    authorize_execution_parameter_key_origins_with_conn(
        &mut transaction,
        execution,
        identity,
        secrets,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// The worker retains this transaction through decryption. Recorded input
/// snapshots remain separate contributors, never a union of their grants.
pub async fn authorize_execution_parameter_key_origins_with_conn(
    connection: &mut sqlx::PgConnection,
    execution: Id,
    identity: Option<Id>,
    secrets: &[crate::secret_values::StoredSecretValue],
) -> Result<()> {
    use crate::repositories::{
        execution_admission::ExecutionAdmissionRepository, runtime_authority::RuntimeOwner,
    };
    let recorded =
        ExecutionAdmissionRepository::find(connection, RuntimeOwner::Execution(execution))
            .await?
            .is_some();
    let authorities = ExecutionAdmissionRepository::input_authorities(
        connection,
        execution,
        if recorded { None } else { identity },
    )
    .await?;
    let mut visited = std::collections::BTreeSet::new();
    for secret in secrets {
        check_parameter_origins(
            connection,
            &authorities,
            recorded.then_some(execution),
            &secret.json_path,
            secret,
            &mut visited,
            0,
        )
        .await?;
    }
    Ok(())
}

pub async fn authorize_parameter_key_origins_with_conn(
    connection: &mut sqlx::PgConnection,
    identity_id: Option<Id>,
    secrets: &[crate::secret_values::StoredSecretValue],
) -> Result<()> {
    let authorities = match identity_id {
        Some(identity_id) => {
            vec![DelegationAuthority::load_for_share(connection, identity_id).await?]
        }
        None => Vec::new(),
    };
    for secret in secrets {
        check_parameter_origins(
            connection,
            &authorities,
            None,
            &secret.json_path,
            secret,
            &mut std::collections::BTreeSet::new(),
            0,
        )
        .await?;
    }
    Ok(())
}

fn check_parameter_origins<'a>(
    connection: &'a mut sqlx::PgConnection,
    authorities: &'a [DelegationAuthority],
    recorded_execution: Option<Id>,
    destination_path: &'a str,
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
        let ownership = if authorities
            .iter()
            .any(DelegationAuthority::needs_key_ownership)
        {
            let ids = provenance
                .origins
                .iter()
                .filter_map(|origin| match origin {
                    SecretOrigin::Key(origin) => Some(origin.key_id),
                    _ => None,
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let mut ownership = VerifiedKeyOrigins::default();
            for batch in ids.chunks(64) {
                ownership.extend(VerifiedKeyOrigins::load(&mut *connection, batch).await?);
            }
            ownership
        } else {
            VerifiedKeyOrigins::default()
        };
        for origin in provenance.origins {
            if let Some(execution) = recorded_execution {
                if crate::repositories::declared_input::DeclaredInputRepository::delivery_allowed(
                    connection,
                    execution,
                    destination_path,
                    &origin,
                )
                .await?
                {
                    continue;
                }
                if crate::repositories::immutable_input_origin::is_immutable_origin(&origin) {
                    if crate::repositories::execution_admission::ExecutionAdmissionRepository::input_runtime_origin_allowed(
                        connection, execution, &origin,
                    ).await? { continue; }
                    return Err(denied());
                }
            }
            match origin {
                SecretOrigin::Key(origin) => {
                    for action in [Action::Read]
                        .into_iter()
                        .chain(origin.encrypted.then_some(Action::Decrypt))
                    {
                        let allowed = if let Some(execution) = recorded_execution {
                            crate::repositories::execution_admission::ExecutionAdmissionRepository::input_key_origin_allowed(
                                connection, execution, &origin, action,
                            ).await?
                        } else {
                            !authorities.is_empty()
                                && authorities.iter().all(|authority| {
                                    authority.key_allows_verified(&origin, action, &ownership)
                                })
                        };
                        if !allowed {
                            return Err(denied());
                        }
                    }
                }
                SecretOrigin::QueueItem { .. } => {
                    // Caller-supplied queue material can pass through batches
                    // without granting API or Key rights. Disclosure separately
                    // requires the recorded queue's read/decrypt grants.
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
                            | crate::secret_values::ENTITY_INQUIRY_OPTIONS
                            | crate::secret_values::ENTITY_INQUIRY_RESPONSE
                            | crate::secret_values::ENTITY_EVENT_PAYLOAD
                            | crate::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG
                            | crate::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG
                            | crate::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG
                            | crate::repositories::component_configuration::ENTITY_COMPONENT_CONFIGURATION
                    ) || !visited.insert((entity_type.clone(), entity_id, path.clone()))
                    {
                        return Err(denied());
                    }
                    let parents = crate::repositories::execution_secret_value::ExecutionSecretValueRepository::find_stored_by_entity(&mut *connection, &entity_type, entity_id).await?;
                    if entity_type == crate::repositories::component_configuration::ENTITY_COMPONENT_CONFIGURATION {
                        let revision = crate::repositories::component_configuration::ComponentConfigurationRepository::find(&mut *connection, entity_id)
                            .await?.ok_or_else(denied)?;
                        if !crate::repositories::component_configuration::ComponentConfigurationRepository::owner_survives(connection, &revision).await? {
                            return Err(denied());
                        }
                        if authorities.is_empty() {
                            return Err(denied());
                        }
                        let managing_pack = match revision.owner_kind.as_str() {
                            "pack" => Some(revision.owner_id),
                            "sensor" => crate::repositories::iam_ownership::IamOwnershipRepository::component_pack(
                                &mut *connection, crate::repositories::runtime_target::ComponentTarget::Sensor, revision.owner_id,
                            ).await?,
                            _ => None,
                        };
                        for authority in authorities {
                            let mut context = AuthorizationContext::new(authority.identity_id);
                            context.managing_pack_id = managing_pack;
                            context.target_id = Some(revision.owner_id);
                            context.target_ref = Some(revision.owner_ref.clone());
                            context.pack_ref = Some(revision.pack_ref.clone());
                            let (resource, read) = if revision.owner_kind == "pack" {
                                (Resource::Packs, Action::Read)
                            } else {
                                (Resource::Triggers, Action::Update)
                            };
                            for action in [read, Action::Decrypt] {
                                if !authority.allows(resource, action, context.clone()) {
                                    return Err(denied());
                                }
                            }
                        }
                    }
                    if entity_type
                        == crate::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG
                    {
                        let (generation, _, _, _) = crate::repositories::sensor_schema::SensorSchemaRepository::find_delivery_owner(&mut *connection, entity_id)
                            .await?.ok_or_else(denied)?;
                        crate::repositories::sensor_schema::SensorSchemaRepository::find_by_binding_id(&mut *connection, generation).await?.ok_or_else(denied)?;
                    }
                    if matches!(entity_type.as_str(),
                        crate::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG
                        | crate::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG)
                    {
                        crate::repositories::sensor_schema::SensorSchemaRepository::find_by_binding_id(&mut *connection, entity_id)
                            .await?.ok_or_else(denied)?;
                    }
                    if entity_type == crate::secret_values::ENTITY_EVENT_PAYLOAD {
                        let owner = crate::repositories::event::EventRepository::find_by_id(
                            &mut *connection,
                            entity_id,
                        )
                        .await?
                        .ok_or_else(denied)?;
                        let binding =
                            crate::repositories::event_schema::EventSchemaRepository::find(
                                &mut *connection,
                                entity_id,
                            )
                            .await?
                            .ok_or_else(denied)?;
                        if !binding.matches_owner(&owner) {
                            return Err(denied());
                        }
                    }
                    let parent = parents
                        .iter()
                        .find(|parent| parent.json_path == path)
                        .ok_or_else(denied)?;
                    check_parameter_origins(
                        &mut *connection,
                        authorities,
                        recorded_execution,
                        destination_path,
                        parent,
                        visited,
                        depth + 1,
                    )
                    .await?;
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
