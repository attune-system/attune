//! Per-value disclosure checks against recorded, immutable secret origins.

use std::collections::BTreeSet;
use std::sync::Arc;

use attune_common::{
    crypto,
    rbac::{Action, AuthorizationContext, Grant},
    repositories::{
        execution::ExecutionAuthorizationRow,
        execution_secret_value::{ExecutionSecretValueRepository, SecretProvenanceRow},
        schema_contract::{ActionSchemaBinding, BindingOwner, SchemaContractRepository},
        FindById,
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT},
};
use serde_json::Value;

use crate::{
    auth::middleware::AuthenticatedUser,
    disclosure_records::DisclosureRecords,
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
    let mut records = DisclosureRecords::default();
    records
        .prepare(
            &state.db,
            user,
            &grants,
            secrets
                .iter()
                .map(|secret| (secret.source_kind.as_str(), secret.source_ref.as_deref())),
        )
        .await?;
    let mut authorization = OriginAuthorization {
        state,
        user,
        caller: &caller,
        grants: &grants,
        records: &mut records,
    };
    for secret in secrets {
        let mut visited = BTreeSet::new();
        if !authorization
            .allow(
                (&secret.source_kind, secret.source_ref.as_deref()),
                &mut visited,
                0,
            )
            .await?
        {
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
    let secrets = ExecutionSecretValueRepository::find_provenance_by_entities(
        &state.db,
        &[ENTITY_EXECUTION_CONFIG, ENTITY_EXECUTION_RESULT],
        &[execution.id],
    )
    .await?;
    let binding = SchemaContractRepository::find_with_executor(
        &state.db,
        BindingOwner::Execution,
        execution.id,
    )
    .await?
    .ok_or_else(|| {
        ApiError::Forbidden("Runtime log disclosure requires the original schema binding".into())
    })?;
    authorize_runtime_log_records(
        state,
        user,
        &ExecutionAuthorizationRow::from(execution),
        &binding,
        &secrets,
        None,
    )
    .await
}

pub(crate) fn runtime_log_requires_disclosure(
    binding: &ActionSchemaBinding,
    secrets: &[SecretProvenanceRow],
) -> ApiResult<bool> {
    let input_sensitive = attune_common::value_schema::CompiledContract::new(&binding.input)
        .map_err(|_| ApiError::Forbidden("Runtime log schema binding is unavailable".into()))?
        .declares_secrets();
    let output_sensitive = binding
        .output
        .as_ref()
        .map(attune_common::value_schema::CompiledContract::new)
        .transpose()
        .map_err(|_| ApiError::Forbidden("Runtime log schema binding is unavailable".into()))?
        .is_some_and(|contract| contract.declares_secrets());
    Ok(!secrets.is_empty() || input_sensitive || output_sensitive)
}

pub(crate) async fn authorize_runtime_log_records(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    execution: &ExecutionAuthorizationRow,
    binding: &ActionSchemaBinding,
    secrets: &[SecretProvenanceRow],
    snapshot: Option<&crate::authz::AuthorizationSnapshot>,
) -> ApiResult<()> {
    let mut combined = secrets.to_vec();
    combined.extend(runtime_key_read_provenance(&state.db, &[execution.id]).await?);
    let secrets = combined.as_slice();
    if !runtime_log_requires_disclosure(binding, secrets)? {
        return Ok(());
    }
    let loaded = if snapshot.is_none() {
        state.authorization_service().load_snapshot(user).await?
    } else {
        None
    };
    let snapshot = snapshot.or(loaded.as_ref()).ok_or_else(|| {
        ApiError::Forbidden("Runtime log secret disclosure requires identity access".into())
    })?;
    let mut records = DisclosureRecords::default();
    records.seed_executions([execution.clone()]);
    records.seed_execution_provenance(&[execution.id], secrets.iter().cloned());
    records.load_executions(&state.db, &[execution.id]).await?;
    records
        .prepare(
            &state.db,
            user,
            &snapshot.grants,
            secrets
                .iter()
                .map(|secret| (secret.source_kind.as_str(), secret.source_ref.as_deref())),
        )
        .await?;
    authorize_runtime_log_cached(state, user, execution, secrets, snapshot, &mut records).await
}

/// Log-only disclosure requirements, not synthetic encrypted value fields.
/// Historical Key ownership remains bound even after Key deletion or mutation.
pub(crate) async fn runtime_key_read_provenance(
    pool: &sqlx::PgPool,
    executions: &[i64],
) -> ApiResult<Vec<SecretProvenanceRow>> {
    let mut rows = Vec::new();
    for (execution, origins) in attune_common::repositories::execution_key_read::ExecutionKeyReadRepository::origins_for_executions(pool, executions).await? {
        for (index, origin) in origins.into_iter().enumerate() {
            rows.push(SecretProvenanceRow {
                entity_type: "execution_runtime_key_read".into(), entity_id: execution,
                json_path: format!("/{index}"), source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&attune_common::secret_provenance::SecretProvenance::key(origin))
                    .map_err(|_| ApiError::InternalServerError("Runtime Key origins could not be encoded".into()))?),
            });
        }
    }
    Ok(rows)
}

/// The caller has validated a retained binding and its sensitivity. Reuse source
/// records and lineage across sensitive executions without reusing recursive
/// allow/deny results or an active traversal path.
pub(crate) async fn authorize_runtime_log_cached(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    execution: &ExecutionAuthorizationRow,
    secrets: &[SecretProvenanceRow],
    snapshot: &crate::authz::AuthorizationSnapshot,
    records: &mut DisclosureRecords,
) -> ApiResult<()> {
    for action in [Action::Read, Action::Decrypt] {
        crate::routes::executions::authorize_execution_projection(
            state,
            user,
            execution,
            action,
            Some(snapshot),
            &mut records.visibility,
        )
        .await?;
    }
    let mut caller = AuthorizationContext::new(snapshot.identity_id);
    caller.identity_attributes = snapshot.identity_attributes.clone();
    let mut authorization = OriginAuthorization {
        state,
        user,
        caller: &caller,
        grants: &snapshot.grants,
        records,
    };
    for secret in secrets {
        if !authorization
            .allow(
                (&secret.source_kind, secret.source_ref.as_deref()),
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

pub async fn authorize_sensor_runtime_log(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    artifact_version: i64,
    sensor_ref: &str,
) -> ApiResult<()> {
    use attune_common::repositories::sensor_schema::{
        SensorSchemaRepository, ENTITY_SENSOR_GENERATION_CONFIG,
        ENTITY_SENSOR_GENERATION_RULE_CONFIG,
    };
    let (id, expected_digest) =
        SensorSchemaRepository::find_log_generation(&state.db, artifact_version)
            .await?
            .ok_or_else(|| {
                ApiError::Forbidden("Sensor log disclosure requires its admitted generation".into())
            })?;
    let (binding, _) = SensorSchemaRepository::find_by_binding_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Sensor log generation has expired".into()))?;
    if binding.executable.sensor.r#ref != sensor_ref || binding.binding_digest()? != expected_digest
    {
        return Err(ApiError::Forbidden("Sensor log generation mismatch".into()));
    }
    let mut secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &state.db,
        ENTITY_SENSOR_GENERATION_CONFIG,
        id,
    )
    .await?;
    let rule_secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &state.db,
        ENTITY_SENSOR_GENERATION_RULE_CONFIG,
        id,
    )
    .await?;
    let deliveries =
        SensorSchemaRepository::log_delivery_origins(&state.db, artifact_version).await?;
    for delivery in &deliveries {
        let (generation, _, _, _) =
            SensorSchemaRepository::find_delivery_owner(&state.db, *delivery)
                .await?
                .ok_or_else(|| {
                    ApiError::Forbidden("Sensor log delivery origin has expired".into())
                })?;
        if generation != id {
            return Err(ApiError::Forbidden(
                "Sensor log delivery origin mismatch".into(),
            ));
        }
        secrets.extend(
            ExecutionSecretValueRepository::find_stored_by_entity(
                &state.db,
                attune_common::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
                *delivery,
            )
            .await?,
        );
    }
    let key_origins = SensorSchemaRepository::key_read_origins(&state.db, id).await?;
    if secrets.is_empty()
        && rule_secrets.is_empty()
        && key_origins.is_empty()
        && !binding.requires_protected_logs()?
    {
        return Ok(());
    }
    let snapshot = state
        .authorization_service()
        .load_snapshot(user)
        .await?
        .ok_or_else(|| {
            ApiError::Forbidden("Sensor log secret disclosure requires identity access".into())
        })?;
    let mut caller = AuthorizationContext::new(snapshot.identity_id);
    caller.identity_attributes = snapshot.identity_attributes;
    for origin in key_origins {
        if !attune_common::key_access::origin_action_allowed(
            &snapshot.grants,
            &caller,
            &origin,
            Action::Read,
        ) || origin.encrypted
            && !attune_common::key_access::origin_action_allowed(
                &snapshot.grants,
                &caller,
                &origin,
                Action::Decrypt,
            )
        {
            return Err(ApiError::Forbidden(
                "Sensor log runtime Key read origins are not authorized".into(),
            ));
        }
    }
    for delivery in deliveries {
        if !sensor_delivery_origin_scope(state, &caller, &snapshot.grants, delivery).await? {
            return Err(ApiError::Forbidden(
                "Sensor log rule delivery origin access is unavailable".into(),
            ));
        }
    }
    let mut context = caller.clone();
    context.target_id = Some(binding.executable.sensor.id);
    context.target_ref = Some(sensor_ref.to_string());
    context.pack_ref = binding.executable.sensor.pack_ref.clone();
    // Sensors use Triggers/Update for visibility in the existing API contract.
    for action in [Action::Update, Action::Decrypt] {
        if !crate::authz::AuthorizationService::is_allowed(
            &snapshot.grants,
            attune_common::rbac::Resource::Triggers,
            action,
            &context,
        ) {
            return Err(ApiError::Forbidden(
                "Sensor log plaintext requires sensor visibility and decrypt authority".into(),
            ));
        }
    }
    let initial_rules = SensorSchemaRepository::find_initial_rules(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Sensor initial rule binding has expired".into()))?;
    let mut checked_rules = BTreeSet::new();
    for secret in &rule_secrets {
        let rule_id = secret
            .json_path
            .split('/')
            .nth(1)
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| ApiError::Forbidden("Invalid sensor rule secret origin".into()))?;
        if !checked_rules.insert(rule_id) {
            continue;
        }
        let recorded = initial_rules.get(rule_id.to_string()).ok_or_else(|| {
            ApiError::Forbidden("Sensor rule secret origin is unavailable".into())
        })?;
        let rule =
            attune_common::repositories::rule::RuleRepository::find_by_id(&state.db, rule_id)
                .await?
                .filter(|rule| {
                    recorded.get("ref").and_then(Value::as_str) == Some(rule.r#ref.as_str())
                })
                .ok_or_else(|| {
                    ApiError::Forbidden("Sensor rule secret source is unavailable".into())
                })?;
        let mut context = caller.clone();
        context.target_id = Some(rule.id);
        context.target_ref = Some(rule.r#ref);
        context.pack_ref = Some(
            recorded
                .get("pack_ref")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ApiError::Forbidden("Sensor rule origin ownership is unavailable".into())
                })?
                .to_string(),
        );
        for action in [Action::Read, Action::Decrypt] {
            if !crate::authz::AuthorizationService::is_allowed(
                &snapshot.grants,
                attune_common::rbac::Resource::Rules,
                action,
                &context,
            ) {
                return Err(ApiError::Forbidden(
                    "Sensor log plaintext requires original rule read and decrypt authority".into(),
                ));
            }
        }
    }
    secrets.extend(rule_secrets);
    let mut records = DisclosureRecords::default();
    records
        .prepare(
            &state.db,
            user,
            &snapshot.grants,
            secrets
                .iter()
                .map(|secret| (secret.source_kind.as_str(), secret.source_ref.as_deref())),
        )
        .await?;
    let mut authorization = OriginAuthorization {
        state,
        user,
        caller: &caller,
        grants: &snapshot.grants,
        records: &mut records,
    };
    for secret in secrets {
        if !authorization
            .allow(
                (&secret.source_kind, secret.source_ref.as_deref()),
                &mut BTreeSet::new(),
                0,
            )
            .await?
        {
            return Err(ApiError::Forbidden(
                "Sensor log secret origin access is unavailable".into(),
            ));
        }
    }
    Ok(())
}

struct OriginAuthorization<'a> {
    state: &'a Arc<AppState>,
    user: &'a AuthenticatedUser,
    caller: &'a AuthorizationContext,
    grants: &'a [Grant],
    records: &'a mut DisclosureRecords,
}

impl OriginAuthorization<'_> {
    fn allow<'a>(
        &'a mut self,
        source: (&'a str, Option<&'a str>),
        visited: &'a mut BTreeSet<(String, i64, String)>,
        depth: usize,
    ) -> futures::future::BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let Self {
                state,
                user,
                caller,
                grants,
                ..
            } = self;
            let (state, user, caller, grants) = (*state, *user, *caller, *grants);
            let (source_kind, source_ref) = source;
            if depth >= 32 || source_kind != "provenance" {
                return Ok(false);
            }
            let Some(reference) = source_ref else {
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
                    SecretOrigin::QueueItem {
                        queue_id,
                        queue_ref,
                        pack_ref,
                        ..
                    } => {
                        let mut context = caller.clone();
                        context.target_id = Some(queue_id);
                        context.target_ref = Some(queue_ref);
                        context.pack_ref = pack_ref;
                        for action in [Action::Read, Action::Decrypt] {
                            if !crate::authz::AuthorizationService::is_allowed(
                                grants,
                                attune_common::rbac::Resource::Queues,
                                action,
                                &context,
                            ) {
                                return Ok(false);
                            }
                        }
                    }
                    SecretOrigin::Entity {
                        entity_type,
                        entity_id,
                        path,
                    } => {
                        if entity_type == attune_common::repositories::component_configuration::ENTITY_COMPONENT_CONFIGURATION {
                         if !visited.insert((entity_type.clone(), entity_id, path.clone())) {return Ok(false);}
                         let Some(revision) = self.records.components.get(&entity_id).and_then(Option::as_ref) else {return Ok(false)};
                         if !revision.owner_survives {return Ok(false);}
                        let mut context = caller.clone();
                        context.target_id = Some(revision.owner_id);
                         context.target_ref = Some(revision.owner_ref.clone());
                         context.pack_ref = Some(revision.pack_ref.clone());
                        let (resource, read) = if revision.owner_kind == "pack" {(attune_common::rbac::Resource::Packs, Action::Read)}
                            else {(attune_common::rbac::Resource::Triggers, Action::Update)};
                        for action in [read, Action::Decrypt] {
                            if !crate::authz::AuthorizationService::is_allowed(grants, resource, action, &context) {return Ok(false);}
                        }
                        let Some(parent) = self.records.parent(&entity_type, entity_id, &path) else {return Ok(false)};
                        if !self.allow((&parent.source_kind, parent.source_ref.as_deref()), visited, depth + 1).await? {return Ok(false);}
                        visited.remove(&(entity_type, entity_id, path));
                        continue;
                    }
                        if entity_type == attune_common::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG {
                        if !visited.insert((entity_type.clone(), entity_id, path.clone()))
                             || !sensor_delivery_cached_scope(self.records, caller, grants, entity_id) {return Ok(false);}
                        let Some(parent) = self.records.parent(&entity_type, entity_id, &path) else {return Ok(false)};
                        if !self.allow((&parent.source_kind, parent.source_ref.as_deref()), visited, depth + 1).await? {return Ok(false);}
                        visited.remove(&(entity_type, entity_id, path));
                        continue;
                    }
                        if matches!(entity_type.as_str(),
                        attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG
                        | attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG) {
                        if !visited.insert((entity_type.clone(), entity_id, path.clone()))
                             || !sensor_generation_cached_scope(self.records, caller, grants, &entity_type, entity_id, &path) {
                            return Ok(false);
                        }
                        let Some(parent) = self.records.parent(&entity_type, entity_id, &path) else {return Ok(false)};
                        if !self.allow((&parent.source_kind, parent.source_ref.as_deref()), visited, depth + 1).await? {return Ok(false);}
                        visited.remove(&(entity_type, entity_id, path));
                        continue;
                    }
                        if entity_type == attune_common::secret_values::ENTITY_EVENT_PAYLOAD {
                            if !visited.insert((entity_type.clone(), entity_id, path.clone())) {
                                return Ok(false);
                            }
                            let Some(event) =
                                self.records.events.get(&entity_id).and_then(Option::as_ref)
                            else {
                                return Ok(false);
                            };
                            let Some(binding) = self.records.event_bindings.get(&entity_id) else {
                                return Ok(false);
                            };
                            if !binding.matches_identity(event.trigger, &event.trigger_ref) {
                                return Ok(false);
                            }
                            let mut visibility = None;
                            for action in [Action::Read, Action::Decrypt] {
                                match crate::routes::events::event_action_visibility_filter(
                                    user, grants, action,
                                ) {
                                    Ok(None) => {}
                                    Ok(Some(_)) => {
                                        let public = action == Action::Read
                                            && user.claims.token_type
                                                == attune_common::auth::jwt::TokenType::Access;
                                        let allowed = *visibility.get_or_insert_with(|| {
                                            self.records
                                                .event_visibility
                                                .get(&(entity_id, public))
                                                .copied()
                                                .unwrap_or(false)
                                        });
                                        if !allowed {
                                            return Ok(false);
                                        }
                                    }
                                    Err(ApiError::Forbidden(_) | ApiError::NotFound(_)) => {
                                        return Ok(false)
                                    }
                                    Err(error) => return Err(error),
                                }
                            }
                            let Some(parent) = self.records.parent(&entity_type, entity_id, &path)
                            else {
                                return Ok(false);
                            };
                            if !self
                                .allow(
                                    (&parent.source_kind, parent.source_ref.as_deref()),
                                    visited,
                                    depth + 1,
                                )
                                .await?
                            {
                                return Ok(false);
                            }
                            visited.remove(&(entity_type, entity_id, path));
                            continue;
                        }
                        if matches!(
                            entity_type.as_str(),
                            attune_common::secret_values::ENTITY_INQUIRY_OPTIONS
                                | attune_common::secret_values::ENTITY_INQUIRY_RESPONSE
                        ) {
                            if !visited.insert((entity_type.clone(), entity_id, path.clone())) {
                                return Ok(false);
                            }
                            let Some(inquiry) = self
                                .records
                                .inquiries
                                .get(&entity_id)
                                .and_then(Option::as_ref)
                            else {
                                return Ok(false);
                            };
                            let creator = self.records.execution(inquiry.created_by_execution);
                            let Some(inquiry_caller) = self.records.inquiry_caller.as_ref() else {
                                return Ok(false);
                            };
                            if !self.records.inquiry_bindings.contains(&entity_id)
                                || !crate::routes::inquiries::inquiry_disclosure_scope_allows(
                                    inquiry,
                                    creator.as_ref(),
                                    inquiry_caller,
                                    grants,
                                )
                            {
                                return Ok(false);
                            }
                            let Some(parent) = self.records.parent(&entity_type, entity_id, &path)
                            else {
                                return Ok(false);
                            };
                            if !self
                                .allow(
                                    (&parent.source_kind, parent.source_ref.as_deref()),
                                    visited,
                                    depth + 1,
                                )
                                .await?
                            {
                                return Ok(false);
                            }
                            visited.remove(&(entity_type, entity_id, path));
                            continue;
                        }
                        if entity_type != ENTITY_EXECUTION_CONFIG
                            && entity_type != ENTITY_EXECUTION_RESULT
                        {
                            return Ok(false);
                        }
                        if !visited.insert((entity_type.clone(), entity_id, path.clone())) {
                            return Ok(false);
                        }
                        let Some(execution) = self.records.execution(entity_id) else {
                            return Ok(false);
                        };
                        let snapshot = crate::authz::AuthorizationSnapshot {
                            identity_id: caller.identity_id,
                            identity_attributes: caller.identity_attributes.clone(),
                            grants: grants.to_vec(),
                        };
                        for action in [Action::Read, Action::Decrypt] {
                            match crate::routes::executions::authorize_execution_projection(
                                state,
                                user,
                                &execution,
                                action,
                                Some(&snapshot),
                                &mut self.records.visibility,
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
                        let Some(parent) = self.records.parent(&entity_type, entity_id, &path)
                        else {
                            return Ok(false);
                        };
                        if !self
                            .allow(
                                (&parent.source_kind, parent.source_ref.as_deref()),
                                visited,
                                depth + 1,
                            )
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
}

fn sensor_generation_cached_scope(
    records: &DisclosureRecords,
    caller: &AuthorizationContext,
    grants: &[Grant],
    entity_type: &str,
    entity_id: i64,
    path: &str,
) -> bool {
    use attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG;
    let Some(generation) = records.generations.get(&entity_id).and_then(Option::as_ref) else {
        return false;
    };
    let binding = &generation.binding;
    let mut context = caller.clone();
    context.target_id = Some(binding.executable.sensor.id);
    context.target_ref = Some(binding.executable.sensor.r#ref.clone());
    context.pack_ref = binding.executable.sensor.pack_ref.clone();
    for action in [Action::Update, Action::Decrypt] {
        if !crate::authz::AuthorizationService::is_allowed(
            grants,
            attune_common::rbac::Resource::Triggers,
            action,
            &context,
        ) {
            return false;
        }
    }
    if entity_type == ENTITY_SENSOR_GENERATION_RULE_CONFIG {
        let Some(rule_id) = path
            .split('/')
            .nth(1)
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|id| *id > 0)
        else {
            return false;
        };
        let Some(recorded) = generation.initial_rules.get(rule_id.to_string()) else {
            return false;
        };
        let Some(reference) =
            records
                .rules
                .get(&rule_id)
                .and_then(Option::as_ref)
                .filter(|reference| {
                    recorded.get("ref").and_then(Value::as_str) == Some(reference.as_str())
                })
        else {
            return false;
        };
        context.target_id = Some(rule_id);
        context.target_ref = Some(reference.clone());
        let Some(pack_ref) = recorded.get("pack_ref").and_then(Value::as_str) else {
            return false;
        };
        context.pack_ref = Some(pack_ref.to_string());
        for action in [Action::Read, Action::Decrypt] {
            if !crate::authz::AuthorizationService::is_allowed(
                grants,
                attune_common::rbac::Resource::Rules,
                action,
                &context,
            ) {
                return false;
            }
        }
    }
    true
}

fn sensor_delivery_cached_scope(
    records: &DisclosureRecords,
    caller: &AuthorizationContext,
    grants: &[Grant],
    id: i64,
) -> bool {
    let Some(delivery) = records.deliveries.get(&id).and_then(Option::as_ref) else {
        return false;
    };
    let Some(generation) = records
        .generations
        .get(&delivery.generation_binding)
        .and_then(Option::as_ref)
    else {
        return false;
    };
    if records.rules.get(&delivery.rule).and_then(Option::as_deref)
        != Some(delivery.rule_ref.as_str())
    {
        return false;
    }
    sensor_delivery_scope_allows(
        caller,
        grants,
        &generation.binding,
        delivery.rule,
        &delivery.rule_ref,
        &delivery.pack_ref,
    )
}

fn sensor_delivery_scope_allows(
    caller: &AuthorizationContext,
    grants: &[Grant],
    binding: &attune_common::sensor_schema::SensorGenerationBinding,
    rule: i64,
    reference: &str,
    pack: &str,
) -> bool {
    let mut context = caller.clone();
    context.target_id = Some(binding.executable.sensor.id);
    context.target_ref = Some(binding.executable.sensor.r#ref.clone());
    context.pack_ref = binding.executable.sensor.pack_ref.clone();
    if ![Action::Update, Action::Decrypt].into_iter().all(|action| {
        crate::authz::AuthorizationService::is_allowed(
            grants,
            attune_common::rbac::Resource::Triggers,
            action,
            &context,
        )
    }) {
        return false;
    }
    context.target_id = Some(rule);
    context.target_ref = Some(reference.into());
    context.pack_ref = Some(pack.into());
    [Action::Read, Action::Decrypt].into_iter().all(|action| {
        crate::authz::AuthorizationService::is_allowed(
            grants,
            attune_common::rbac::Resource::Rules,
            action,
            &context,
        )
    })
}

async fn sensor_delivery_origin_scope(
    state: &Arc<AppState>,
    caller: &AuthorizationContext,
    grants: &[Grant],
    delivery: i64,
) -> ApiResult<bool> {
    use attune_common::repositories::sensor_schema::SensorSchemaRepository;
    let Some((generation, rule_id, rule_ref, pack_ref)) =
        SensorSchemaRepository::find_delivery_owner(&state.db, delivery).await?
    else {
        return Ok(false);
    };
    let Some((binding, _)) =
        SensorSchemaRepository::find_by_binding_id(&state.db, generation).await?
    else {
        return Ok(false);
    };
    let mut context = caller.clone();
    context.target_id = Some(binding.executable.sensor.id);
    context.target_ref = Some(binding.executable.sensor.r#ref);
    context.pack_ref = binding.executable.sensor.pack_ref;
    for action in [Action::Update, Action::Decrypt] {
        if !crate::authz::AuthorizationService::is_allowed(
            grants,
            attune_common::rbac::Resource::Triggers,
            action,
            &context,
        ) {
            return Ok(false);
        }
    }
    if attune_common::repositories::rule::RuleRepository::find_by_id(&state.db, rule_id)
        .await?
        .is_none_or(|rule| rule.r#ref != rule_ref)
    {
        return Ok(false);
    }
    context.target_id = Some(rule_id);
    context.target_ref = Some(rule_ref);
    context.pack_ref = Some(pack_ref);
    for action in [Action::Read, Action::Decrypt] {
        if !crate::authz::AuthorizationService::is_allowed(
            grants,
            attune_common::rbac::Resource::Rules,
            action,
            &context,
        ) {
            return Ok(false);
        }
    }
    Ok(true)
}
