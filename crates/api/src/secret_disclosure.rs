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

#[cfg(test)]
#[path = "secret_disclosure_empty_root_tests.rs"]
mod empty_root_tests;

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
    let rows = secrets
        .iter()
        .map(|secret| SecretProvenanceRow {
            entity_type: entity_type.into(),
            entity_id,
            json_path: secret.json_path.clone(),
            source_kind: secret.source_kind.clone(),
            source_ref: secret.source_ref.clone(),
        })
        .collect::<Vec<_>>();
    let receipt_guard = crate::receipt_disclosure::HumanReceiptGuard::for_rows(state, user, &rows)
        .await?
        .shared();
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
    let mut runtime_budget = RuntimeOriginBudget::with_guard(receipt_guard.clone());
    let mut authorization = OriginAuthorization {
        runtime_budget: &mut runtime_budget,
        state,
        user,
        caller: &caller,
        grants: &grants,
        records: &mut records,
    };
    for secret in secrets {
        let mut visited = BTreeSet::new();
        if !authorization
            .allow_at(
                (&secret.source_kind, secret.source_ref.as_deref()),
                &mut visited,
                0,
                attune_common::repositories::immutable_input_origin::input_value_site(
                    entity_type,
                    entity_id,
                    &secret.json_path,
                )
                .ok(),
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
        } else {
            authorization
                .runtime_budget
                .file_denied(HumanFileOriginStage::RevealPathMissing);
        }
    }
    authorization
        .runtime_budget
        .receipt_freshness
        .ensure_fresh()?;
    drop(runtime_budget);
    let receipts =
        crate::receipt_disclosure::HumanReceiptGuard::finish_shared(receipt_guard).await?;
    receipts.ensure_fresh()?;
    if user.claims.exp <= chrono::Utc::now().timestamp() {
        return Err(ApiError::Unauthorized(
            "Disclosure credential expired".into(),
        ));
    }
    Ok(Some(value))
}

// Diagnostics are finite tags only. Do not attach origins, paths, IDs, values,
// credentials or error/provenance text, including on a nested source failure.
#[derive(Clone, Copy, Debug)]
enum HumanFileOriginStage {
    Snapshot,
    Frontier,
    Records,
    EvaluationError,
    EvaluationDenied,
    ReceiptChild,
    KeyIdentity,
    KeyGrant,
    ReadIdentity,
    ReadMissing,
    ReadBounds,
    ReadExecutionMissing,
    ReadExecutionRights,
    ReadExecutionBinding,
    ArtifactMissing,
    ArtifactOwner,
    ArtifactRead,
    ArtifactDecrypt,
    PrivateArtifactReadScope,
    PrivateArtifactDecryptScope,
    VersionMissing,
    VersionIdentity,
    VersionState,
    VersionManifest,
    ReadChildren,
    EntityMissing,
    EntityRights,
    EntityParent,
    LocalSource,
    ReceiptExecutionMissing,
    ReceiptExecutionRead,
    ReceiptExecutionDecrypt,
    ReceiptSchemaMissing,
    NativeTokenKind,
    NativeCycle,
    NativeParent,
    NativeBounds,
    NativeRead,
    NativeDecrypt,
    RevealPathMissing,
    CurrentIamSnapshot,
}

fn file_origin_error(stage: HumanFileOriginStage, error: ApiError) -> ApiError {
    tracing::warn!(human_file_origin_stage = ?stage, "File source authorization failed");
    error
}

/// File bodies retain a specific immutable graph row, never the producer's
/// later mutable output or execution-wide current read set.
/// File delivery callers retain the returned guard through their Ready/body
/// checks, then finish it once after all application waits.
pub(crate) async fn authorize_file_origin_with_receipts(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    origin_read: i64,
) -> ApiResult<crate::receipt_disclosure::SharedHumanReceiptGuard> {
    let snapshot = state
        .authorization_service()
        .load_snapshot(user)
        .await
        .map_err(|error| file_origin_error(HumanFileOriginStage::Snapshot, error))?
        .ok_or_else(|| {
            file_origin_error(
                HumanFileOriginStage::Snapshot,
                ApiError::Forbidden("File origin disclosure requires identity access".into()),
            )
        })?;
    let provenance = SecretProvenance {
        origins: vec![
            attune_common::repositories::runtime_read::RuntimeReadRepository::reference(
                origin_read,
            ),
        ],
        templates: vec![],
    };
    let encoded = serde_json::to_string(&provenance)
        .map_err(|_| ApiError::Forbidden("File origins unavailable".into()))?;
    let rows = [SecretProvenanceRow {
        entity_type: "artifact_file_body".into(),
        entity_id: 0,
        json_path: String::new(),
        source_kind: "provenance".into(),
        source_ref: Some(encoded.clone()),
    }];
    let receipts = crate::receipt_disclosure::HumanReceiptGuard::for_rows(state, user, &rows)
        .await
        .map_err(|error| file_origin_error(HumanFileOriginStage::Frontier, error))?
        .shared();
    let mut records = DisclosureRecords::default();
    records
        .prepare(
            &state.db,
            user,
            &snapshot.grants,
            [("provenance", Some(encoded.as_str()))],
        )
        .await
        .map_err(|error| file_origin_error(HumanFileOriginStage::Records, error))?;
    let mut caller = AuthorizationContext::new(snapshot.identity_id);
    caller.identity_attributes = snapshot.identity_attributes.clone();
    let mut budget = RuntimeOriginBudget::with_guard(receipts.clone());
    budget.trace_file_origins = true;
    let mut authorization = OriginAuthorization {
        runtime_budget: &mut budget,
        state,
        user,
        caller: &caller,
        grants: &snapshot.grants,
        records: &mut records,
    };
    if !authorization
        .allow(
            ("provenance", Some(encoded.as_str())),
            &mut BTreeSet::new(),
            0,
        )
        .await
        .map_err(|error| file_origin_error(HumanFileOriginStage::EvaluationError, error))?
    {
        return Err(file_origin_error(
            HumanFileOriginStage::EvaluationDenied,
            ApiError::Forbidden("File source grants are unavailable".into()),
        ));
    }
    drop(budget);
    Ok(receipts)
}

/// Preflight only; final protected-body callers retain the helper's shared guard.
pub(crate) async fn authorize_file_origin(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    origin_read: i64,
) -> ApiResult<()> {
    let receipts = authorize_file_origin_with_receipts(state, user, origin_read).await?;
    let receipts = crate::receipt_disclosure::HumanReceiptGuard::finish_shared(receipts).await?;
    receipts.ensure_fresh()?;
    if user.claims.exp <= chrono::Utc::now().timestamp() {
        return Err(ApiError::Unauthorized("File credential expired".into()));
    }
    Ok(())
}

pub async fn authorize_runtime_log(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    execution: &attune_common::models::Execution,
) -> ApiResult<()> {
    let secrets = ExecutionSecretValueRepository::find_provenance_by_entities(
        &state.db,
        &[
            ENTITY_EXECUTION_CONFIG,
            ENTITY_EXECUTION_RESULT,
            "workflow_variables",
        ],
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
    for (execution, origins) in
        attune_common::repositories::runtime_read::RuntimeReadRepository::references_for_executions(
            pool, executions,
        )
        .await?
    {
        for (index, origin) in origins.into_iter().enumerate() {
            rows.push(SecretProvenanceRow {
                entity_type: attune_common::repositories::runtime_read::ENTITY_RUNTIME_READ.into(),
                entity_id: execution,
                json_path: format!("/{index}"),
                source_kind: "provenance".into(),
                source_ref: Some(
                    serde_json::to_string(&attune_common::secret_provenance::SecretProvenance {
                        origins: vec![origin],
                        templates: vec![],
                    })
                    .map_err(|_| ApiError::Forbidden("Runtime read evidence unavailable".into()))?,
                ),
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
    let receipts = crate::receipt_disclosure::HumanReceiptGuard::for_rows(state, user, secrets)
        .await?
        .shared();
    authorize_runtime_log_cached_impl(
        state,
        user,
        execution,
        secrets,
        snapshot,
        records,
        Some(receipts.clone()),
    )
    .await?;
    let receipts = crate::receipt_disclosure::HumanReceiptGuard::finish_shared(receipts).await?;
    receipts.ensure_fresh()?;
    if user.claims.exp <= chrono::Utc::now().timestamp() {
        return Err(ApiError::Unauthorized(
            "Runtime log credential expired".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn authorize_runtime_log_cached_guarded(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    execution: &ExecutionAuthorizationRow,
    secrets: &[SecretProvenanceRow],
    snapshot: &crate::authz::AuthorizationSnapshot,
    records: &mut DisclosureRecords,
    guard: crate::receipt_disclosure::SharedHumanReceiptGuard,
) -> ApiResult<()> {
    authorize_runtime_log_cached_impl(
        state,
        user,
        execution,
        secrets,
        snapshot,
        records,
        Some(guard),
    )
    .await
}

async fn authorize_runtime_log_cached_impl(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    execution: &ExecutionAuthorizationRow,
    secrets: &[SecretProvenanceRow],
    snapshot: &crate::authz::AuthorizationSnapshot,
    records: &mut DisclosureRecords,
    receipt_guard: Option<crate::receipt_disclosure::SharedHumanReceiptGuard>,
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
    let mut runtime_budget = RuntimeOriginBudget {
        receipt_guard,
        ..Default::default()
    };
    let mut authorization = OriginAuthorization {
        runtime_budget: &mut runtime_budget,
        state,
        user,
        caller: &caller,
        grants: &snapshot.grants,
        records,
    };
    for secret in secrets {
        if !authorization
            .allow_at(
                (&secret.source_kind, secret.source_ref.as_deref()),
                &mut BTreeSet::new(),
                0,
                attune_common::repositories::immutable_input_origin::input_value_site(
                    &secret.entity_type,
                    secret.entity_id,
                    &secret.json_path,
                )
                .ok(),
            )
            .await?
        {
            return Err(ApiError::Forbidden(
                "Runtime log secret origins are not authorized".into(),
            ));
        }
    }
    let freshness = std::mem::take(&mut authorization.runtime_budget.receipt_freshness);
    authorization.records.retain_receipt_freshness(freshness)?;
    Ok(())
}

pub async fn authorize_sensor_runtime_log(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    artifact_version: i64,
    sensor_ref: &str,
) -> ApiResult<()> {
    let receipts =
        authorize_sensor_runtime_log_with_receipts(state, user, artifact_version, sensor_ref)
            .await?;
    crate::receipt_disclosure::HumanReceiptGuard::finish_shared(receipts)
        .await?
        .ensure_fresh()?;
    if user.claims.exp <= chrono::Utc::now().timestamp() {
        return Err(ApiError::Unauthorized(
            "Sensor log credential expired".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn authorize_sensor_runtime_log_with_receipts(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    artifact_version: i64,
    sensor_ref: &str,
) -> ApiResult<crate::receipt_disclosure::SharedHumanReceiptGuard> {
    let mut receipts =
        crate::receipt_disclosure::HumanReceiptGuard::for_rows(state, user, &[]).await?;
    receipts
        .prepare_sensor_logs(state, &[artifact_version])
        .await?;
    let receipts = receipts.shared();
    authorize_sensor_runtime_log_prepared(state, user, artifact_version, sensor_ref, &receipts)
        .await?;
    Ok(receipts)
}

pub(crate) async fn authorize_sensor_runtime_log_prepared(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    artifact_version: i64,
    sensor_ref: &str,
    receipt_guard: &crate::receipt_disclosure::SharedHumanReceiptGuard,
) -> ApiResult<()> {
    receipt_guard.lock().await.close_sources();
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
    let mut records = DisclosureRecords::default();
    // Use the same prepared generation proof as recursive sensor origins.
    // Seeding a decoded row alone would skip its structural owner evidence.
    records.load_generations(&state.db, &[id]).await?;
    let binding = records
        .generations
        .get(&id)
        .and_then(Option::as_ref)
        .map(|generation| generation.binding.clone())
        .ok_or_else(|| ApiError::Forbidden("Sensor log generation has expired".into()))?;
    if binding.executable.sensor.r#ref != sensor_ref || binding.binding_digest()? != expected_digest
    {
        return Err(ApiError::Forbidden("Sensor log generation mismatch".into()));
    }
    let mut secrets = Vec::new();
    let mut rule_secrets = Vec::new();
    for row in ExecutionSecretValueRepository::find_provenance_by_entities(
        &state.db,
        &[
            ENTITY_SENSOR_GENERATION_CONFIG,
            ENTITY_SENSOR_GENERATION_RULE_CONFIG,
        ],
        &[id],
    )
    .await?
    {
        if row.entity_type == ENTITY_SENSOR_GENERATION_RULE_CONFIG {
            rule_secrets.push(row);
        } else {
            secrets.push(row);
        }
    }
    let deliveries =
        SensorSchemaRepository::log_delivery_origins(&state.db, artifact_version).await?;
    for batch in deliveries.chunks(64) {
        let rows = SensorSchemaRepository::find_delivery_disclosure_rows(&state.db, batch).await?;
        for row in rows {
            if row.generation_binding != id {
                return Err(ApiError::Forbidden(
                    "Sensor log delivery origin mismatch".into(),
                ));
            }
            records.deliveries.insert(row.id, Some(row));
        }
        if batch.iter().any(|id| !records.deliveries.contains_key(id)) {
            return Err(ApiError::Forbidden(
                "Sensor log delivery origin has expired".into(),
            ));
        }
        secrets.extend(
            ExecutionSecretValueRepository::find_provenance_by_entities(
                &state.db,
                &[attune_common::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG],
                batch,
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
    let held_snapshot = receipt_guard.lock().await.iam_snapshot();
    let snapshot = match held_snapshot {
        Some(snapshot) => snapshot,
        None => state
            .authorization_service()
            .load_snapshot(user)
            .await?
            .ok_or_else(|| {
                ApiError::Forbidden("Sensor log secret disclosure requires identity access".into())
            })?,
    };
    let mut caller = AuthorizationContext::new(snapshot.identity_id);
    caller.identity_attributes = snapshot.identity_attributes;
    let mut key_ownership =
        attune_common::repositories::iam_ownership::VerifiedKeyOrigins::default();
    if snapshot.grants.iter().any(|grant| {
        grant.resource == attune_common::rbac::Resource::Keys
            && grant
                .constraints
                .as_ref()
                .is_some_and(|scope| scope.managing_pack_id.is_some())
    }) {
        let ids = key_origins
            .iter()
            .map(|origin| origin.key_id)
            .collect::<Vec<_>>();
        for batch in ids.chunks(64) {
            key_ownership.extend(
                attune_common::repositories::iam_ownership::VerifiedKeyOrigins::load(
                    &state.db, batch,
                )
                .await?,
            );
        }
    }
    for origin in key_origins {
        if !attune_common::key_access::origin_action_allowed_verified(
            &snapshot.grants,
            &caller,
            &origin,
            Action::Read,
            &key_ownership,
        ) || origin.encrypted
            && !attune_common::key_access::origin_action_allowed_verified(
                &snapshot.grants,
                &caller,
                &origin,
                Action::Decrypt,
                &key_ownership,
            )
        {
            return Err(ApiError::Forbidden(
                "Sensor log runtime Key read origins are not authorized".into(),
            ));
        }
    }
    let mut sources = std::collections::HashSet::from([(
        ENTITY_SENSOR_GENERATION_CONFIG.to_string(),
        id,
        String::new(),
    )]);
    for delivery in &deliveries {
        sources.insert((
            attune_common::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG
                .to_string(),
            *delivery,
            String::new(),
        ));
    }
    for secret in &rule_secrets {
        sources.insert((
            attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG
                .to_string(),
            id,
            secret.json_path.clone(),
        ));
    }
    records
        .load_sensor_sources(&state.db, user, &snapshot.grants, &sources)
        .await?;
    records
        .prepare(
            &state.db,
            user,
            &snapshot.grants,
            secrets
                .iter()
                .chain(rule_secrets.iter())
                .map(|secret| (secret.source_kind.as_str(), secret.source_ref.as_deref())),
        )
        .await?;
    for delivery in deliveries {
        if !sensor_delivery_cached_scope(&records, &caller, &snapshot.grants, delivery) {
            return Err(ApiError::Forbidden(
                "Sensor log rule delivery origin access is unavailable".into(),
            ));
        }
    }
    if !sensor_generation_cached_scope(
        &records,
        &caller,
        &snapshot.grants,
        ENTITY_SENSOR_GENERATION_CONFIG,
        id,
        "",
    ) {
        return Err(ApiError::Forbidden(
            "Sensor log plaintext requires sensor visibility and decrypt authority".into(),
        ));
    }
    let mut checked_rules = BTreeSet::new();
    for secret in &rule_secrets {
        let rule = crate::sensor_origin_iam::generation_rule_id(&secret.json_path)
            .ok_or_else(|| ApiError::Forbidden("Invalid sensor rule secret origin".into()))?;
        if !checked_rules.insert(rule) {
            continue;
        }
        if !sensor_generation_cached_scope(
            &records,
            &caller,
            &snapshot.grants,
            attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            id,
            &secret.json_path,
        ) {
            return Err(ApiError::Forbidden(
                "Sensor log plaintext requires original rule read and decrypt authority".into(),
            ));
        }
    }
    secrets.extend(rule_secrets);
    let mut runtime_budget = RuntimeOriginBudget::with_guard(receipt_guard.clone());
    let mut authorization = OriginAuthorization {
        runtime_budget: &mut runtime_budget,
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
    let freshness = std::mem::take(&mut authorization.runtime_budget.receipt_freshness);
    freshness.ensure_fresh()?;
    drop(runtime_budget);
    Ok(())
}

#[derive(Default)]
struct RuntimeOriginBudget {
    trace_file_origins: bool,
    trace_native_origins: bool,
    iam_checked: bool,
    wait_scopes: BTreeSet<(String, i64, String)>,
    charged_wait_outputs: BTreeSet<i64>,
    success_scopes: BTreeSet<(String, i64, String)>,
    charged_success_outputs: BTreeSet<i64>,
    nodes: usize,
    bytes: usize,
    receipt_freshness: attune_common::repositories::immutable_input_origin::ReceiptFreshness,
    receipt_guard: Option<crate::receipt_disclosure::SharedHumanReceiptGuard>,
}

async fn receipt_keys_current(
    conn: &mut sqlx::PgConnection,
    principal: i64,
    origins: &[SecretOrigin],
    ownership: &mut attune_common::repositories::iam_ownership::VerifiedKeyOrigins,
) -> ApiResult<bool> {
    for source in origins {
        if let SecretOrigin::Key(original) = source {
            if ownership
                .retain_recorded(conn, principal, original)
                .await?
                .is_none()
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

impl RuntimeOriginBudget {
    fn file_error(&self, stage: HumanFileOriginStage, error: ApiError) -> ApiError {
        if self.trace_native_origins {
            tracing::warn!(human_native_origin_stage=?stage,"Native source authorization failed");
        }
        if self.trace_file_origins {
            return file_origin_error(stage, error);
        }
        error
    }
    fn file_denied(&self, stage: HumanFileOriginStage) -> bool {
        if self.trace_native_origins {
            tracing::warn!(human_native_origin_stage=?stage,"Native source authorization failed");
        }
        if self.trace_file_origins {
            tracing::warn!(human_file_origin_stage = ?stage, "File source authorization failed");
        }
        false
    }
    fn with_guard(guard: crate::receipt_disclosure::SharedHumanReceiptGuard) -> Self {
        Self {
            receipt_guard: Some(guard),
            ..Default::default()
        }
    }
}

struct OriginAuthorization<'a> {
    runtime_budget: &'a mut RuntimeOriginBudget,
    state: &'a Arc<AppState>,
    user: &'a AuthenticatedUser,
    caller: &'a AuthorizationContext,
    grants: &'a [Grant],
    records: &'a mut DisclosureRecords,
}

impl OriginAuthorization<'_> {
    async fn authorize_success_output(
        &mut self,
        guard: &mut crate::receipt_disclosure::HumanReceiptGuard,
        source: &attune_common::repositories::native_input_disclosure::PreparedSuccessSource,
    ) -> ApiResult<bool> {
        if self.user.claims.token_type != attune_common::auth::jwt::TokenType::Access {
            return Ok(self
                .runtime_budget
                .file_denied(HumanFileOriginStage::NativeTokenKind));
        }
        if source
            .parents()
            .iter()
            .any(|parent| !self.records.frozen_parent_matches(parent))
        {
            return Ok(self
                .runtime_budget
                .file_denied(HumanFileOriginStage::NativeParent));
        }
        if self
            .runtime_budget
            .charged_success_outputs
            .insert(source.metadata().snapshot())
        {
            self.runtime_budget.nodes = self
                .runtime_budget
                .nodes
                .checked_add(source.retained_nodes())
                .ok_or_else(|| {
                    ApiError::Forbidden("Receipt disclosure exceeds its bound".into())
                })?;
            self.runtime_budget.bytes = self
                .runtime_budget
                .bytes
                .checked_add(source.retained_bytes())
                .ok_or_else(|| {
                    ApiError::Forbidden("Receipt disclosure exceeds its bound".into())
                })?;
            if self.runtime_budget.nodes > 256 || self.runtime_budget.bytes > 1024 * 1024 {
                return Ok(self
                    .runtime_budget
                    .file_denied(HumanFileOriginStage::NativeBounds));
            }
        }
        for target in attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve_native_success_source(
            guard.connection(), source, self.caller.identity_id).await? {
            let mut context = target.context().clone();
            context.identity_attributes = self.caller.identity_attributes.clone();
            for action in [Action::Read, Action::Decrypt] {
                if !self.grants.iter().any(|grant| grant.allows(target.resource(), action, &context)) {
                    return Ok(self.runtime_budget.file_denied(if action == Action::Read { HumanFileOriginStage::NativeRead } else { HumanFileOriginStage::NativeDecrypt }));
                }
            }
        }
        source.ensure_fresh()?;
        guard.frontier().ensure_fresh()?;
        Ok(true)
    }

    async fn authorize_wait_output(
        &mut self,
        guard: &mut crate::receipt_disclosure::HumanReceiptGuard,
        source: &attune_common::repositories::native_input_disclosure::PreparedWaitCompletionSource,
    ) -> ApiResult<bool> {
        if self.user.claims.token_type != attune_common::auth::jwt::TokenType::Access {
            return Ok(self
                .runtime_budget
                .file_denied(HumanFileOriginStage::NativeTokenKind));
        }
        for parent in source.parents() {
            if !self.records.frozen_parent_matches(parent) {
                return Ok(self
                    .runtime_budget
                    .file_denied(HumanFileOriginStage::NativeParent));
            }
        }
        if self
            .runtime_budget
            .charged_wait_outputs
            .insert(source.metadata().snapshot())
        {
            self.runtime_budget.nodes = self
                .runtime_budget
                .nodes
                .checked_add(source.retained_nodes())
                .ok_or_else(|| {
                    ApiError::Forbidden("Receipt disclosure exceeds its bound".into())
                })?;
            self.runtime_budget.bytes = self
                .runtime_budget
                .bytes
                .checked_add(source.retained_bytes())
                .ok_or_else(|| {
                    ApiError::Forbidden("Receipt disclosure exceeds its bound".into())
                })?;
            if self.runtime_budget.nodes > 256 || self.runtime_budget.bytes > 1024 * 1024 {
                return Ok(self
                    .runtime_budget
                    .file_denied(HumanFileOriginStage::NativeBounds));
            }
        }
        let targets = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve_native_wait_completion_source(
            guard.connection(), source, self.caller.identity_id).await?;
        for target in targets {
            let mut context = target.context().clone();
            context.identity_attributes = self.caller.identity_attributes.clone();
            for action in [Action::Read, Action::Decrypt] {
                if !self
                    .grants
                    .iter()
                    .any(|grant| grant.allows(target.resource(), action, &context))
                {
                    return Ok(self.runtime_budget.file_denied(if action == Action::Read {
                        HumanFileOriginStage::NativeRead
                    } else {
                        HumanFileOriginStage::NativeDecrypt
                    }));
                }
            }
        }
        source.ensure_fresh()?;
        guard.frontier().ensure_fresh()?;
        Ok(true)
    }

    fn allow<'a>(
        &'a mut self,
        source: (&'a str, Option<&'a str>),
        visited: &'a mut BTreeSet<(String, i64, String)>,
        depth: usize,
    ) -> futures::future::BoxFuture<'a, ApiResult<bool>> {
        self.allow_at(source, visited, depth, None)
    }

    fn allow_at<'a>(
        &'a mut self,
        source: (&'a str, Option<&'a str>),
        visited: &'a mut BTreeSet<(String, i64, String)>,
        depth: usize,
        site: Option<attune_common::repositories::inherited_input::InputSite>,
    ) -> futures::future::BoxFuture<'a, ApiResult<bool>> {
        self.allow_classified(source, visited, depth, site, None)
    }

    fn allow_classified<'a>(
        &'a mut self,
        source: (&'a str, Option<&'a str>),
        visited: &'a mut BTreeSet<(String, i64, String)>,
        depth: usize,
        site: Option<attune_common::repositories::inherited_input::InputSite>,
        leaves: Option<
            attune_common::repositories::immutable_input_origin::ReceiptDisclosureLeaves,
        >,
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
            if !self.runtime_budget.iam_checked {
                if let Some(shared) = self.runtime_budget.receipt_guard.clone() {
                    if !shared.lock().await.matches_iam(caller, grants) {
                        tracing::warn!(human_access_origin_stage=?HumanFileOriginStage::CurrentIamSnapshot,"Current disclosure snapshot changed");
                        return Ok(self
                            .runtime_budget
                            .file_denied(HumanFileOriginStage::CurrentIamSnapshot));
                    }
                    self.runtime_budget.iam_checked = true;
                }
            }
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
                if attune_common::repositories::immutable_input_origin::is_input_receipt(&origin) {
                    self.runtime_budget.nodes += 1;
                    self.runtime_budget.bytes = self
                        .runtime_budget
                        .bytes
                        .checked_add(reference.len())
                        .ok_or_else(|| {
                            ApiError::Forbidden("Receipt disclosure exceeds its bound".into())
                        })?;
                    if self.runtime_budget.nodes > 256 || self.runtime_budget.bytes > 1024 * 1024 {
                        return Ok(false);
                    }
                    let source = if site.is_none() {
                        Some(
                            leaves
                                .as_ref()
                                .ok_or_else(|| {
                                    ApiError::Forbidden(
                                        "Receipt source descriptor unavailable".into(),
                                    )
                                })?
                                .source_receipt(&origin)?
                                .clone(),
                        )
                    } else {
                        None
                    };
                    let Some(shared) = self.runtime_budget.receipt_guard.clone() else {
                        return Ok(false);
                    };
                    let (underlying, classification) = {
                        let guard = shared.lock().await;
                        if let Some(source) = &source {
                            let context = guard.frontier().prepared_source(source)?;
                            (SecretProvenance { origins: context.source_origins().to_vec(), templates: vec![] },
                             attune_common::repositories::immutable_input_origin::ReceiptDisclosureLeaves::from_context(context)?)
                        } else {
                            let site = site.as_ref().ok_or_else(|| {
                                ApiError::Forbidden("Receipt site unavailable".into())
                            })?;
                            (
                                SecretProvenance {
                                    origins: guard
                                        .frontier()
                                        .source_origins(site, &origin)?
                                        .to_vec(),
                                    templates: vec![],
                                },
                                guard.frontier().disclosure_leaves(site, &origin)?,
                            )
                        }
                    };
                    self.runtime_budget.trace_native_origins |= classification.has_native_sources();
                    self.runtime_budget.bytes = self
                        .runtime_budget
                        .bytes
                        .checked_add(classification.source_descriptor_bytes()?)
                        .ok_or_else(|| {
                            ApiError::Forbidden("Receipt disclosure exceeds its bound".into())
                        })?;
                    if self.runtime_budget.bytes > 1024 * 1024 {
                        return Ok(false);
                    }
                    let wait_output = if let Some(source) = &source {
                        let guard = shared.lock().await;
                        guard.frontier().prepared_wait_completion_receipt_source(
                            leaves.as_ref().unwrap_or(&classification),
                            &self.runtime_budget.wait_scopes,
                            source,
                        )?
                    } else {
                        None
                    };
                    let mut added_wait_scope = None;
                    let success_output = if let Some(source) = &source {
                        shared
                            .lock()
                            .await
                            .frontier()
                            .prepared_success_receipt_source(
                                leaves.as_ref().unwrap_or(&classification),
                                &self.runtime_budget.success_scopes,
                                source,
                            )?
                    } else {
                        None
                    };
                    let mut added_success_scope = None;
                    if let Some(native) = &wait_output {
                        let mut guard = shared.lock().await;
                        if !self.authorize_wait_output(&mut guard, native).await? {
                            return Ok(false);
                        }
                        let scope = native.scope_node();
                        if self.runtime_budget.wait_scopes.insert(scope.clone()) {
                            added_wait_scope = Some(scope);
                        }
                    } else if let Some(native) = &success_output {
                        let mut guard = shared.lock().await;
                        if !self.authorize_success_output(&mut guard, native).await? {
                            return Ok(false);
                        }
                        let scope = native.scope_node();
                        if self.runtime_budget.success_scopes.insert(scope.clone()) {
                            added_success_scope = Some(scope);
                        }
                    } else if let Some(source) = &source {
                        let execution_id = source.site().execution;
                        self.records
                            .load_executions(&state.db, &[execution_id])
                            .await?;
                        let Some(execution) = self.records.execution(execution_id) else {
                            return Ok(self
                                .runtime_budget
                                .file_denied(HumanFileOriginStage::ReceiptExecutionMissing));
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
                                    return Ok(self.runtime_budget.file_denied(
                                        if action == Action::Read {
                                            HumanFileOriginStage::ReceiptExecutionRead
                                        } else {
                                            HumanFileOriginStage::ReceiptExecutionDecrypt
                                        },
                                    ))
                                }
                                Err(error) => return Err(error),
                            }
                        }
                        let mut guard = shared.lock().await;
                        if SchemaContractRepository::find(
                            guard.connection(),
                            BindingOwner::Execution,
                            execution_id,
                        )
                        .await?
                        .is_none()
                        {
                            return Ok(self
                                .runtime_budget
                                .file_denied(HumanFileOriginStage::ReceiptSchemaMissing));
                        }
                    }
                    // Release the guard mutex before recursion, but retain its
                    // transaction and prepared source locks through the handoff.
                    if !underlying.origins.is_empty() {
                        let reference = serde_json::to_string(&underlying).map_err(|_| {
                            ApiError::Forbidden("Receipt origins unavailable".into())
                        })?;
                        let mut records = DisclosureRecords::default();
                        records.inherit_frozen_parents(self.records)?;
                        records
                            .prepare(
                                &state.db,
                                user,
                                grants,
                                [("provenance", Some(reference.as_str()))],
                            )
                            .await?;
                        let mut child = OriginAuthorization {
                            state,
                            user,
                            caller,
                            grants,
                            records: &mut records,
                            runtime_budget: &mut *self.runtime_budget,
                        };
                        if !child
                            .allow_classified(
                                ("provenance", Some(&reference)),
                                visited,
                                depth + 1,
                                None,
                                Some(classification),
                            )
                            .await?
                        {
                            return Ok(child
                                .runtime_budget
                                .file_denied(HumanFileOriginStage::ReceiptChild));
                        }
                    }
                    {
                        let mut guard = shared.lock().await;
                        if !receipt_keys_current(
                            guard.connection(),
                            caller.identity_id,
                            &underlying.origins,
                            &mut self.records.key_ownership,
                        )
                        .await?
                        {
                            return Ok(self
                                .runtime_budget
                                .file_denied(HumanFileOriginStage::KeyIdentity));
                        }
                        guard.frontier().ensure_fresh()?;
                    }
                    if let Some(scope) = added_wait_scope {
                        self.runtime_budget.wait_scopes.remove(&scope);
                    }
                    if let Some(scope) = added_success_scope {
                        self.runtime_budget.success_scopes.remove(&scope);
                    }
                    continue;
                }
                match origin {
                    SecretOrigin::Key(origin) => {
                        if let Some(shared) = self.runtime_budget.receipt_guard.clone() {
                            let mut guard = shared.lock().await;
                            if !receipt_keys_current(
                                guard.connection(),
                                caller.identity_id,
                                &[SecretOrigin::Key(origin.clone())],
                                &mut self.records.key_ownership,
                            )
                            .await?
                            {
                                return Ok(self
                                    .runtime_budget
                                    .file_denied(HumanFileOriginStage::KeyIdentity));
                            }
                        }
                        if !attune_common::key_access::origin_action_allowed_verified(
                            grants,
                            caller,
                            &origin,
                            Action::Read,
                            &self.records.key_ownership,
                        ) || origin.encrypted
                            && !attune_common::key_access::origin_action_allowed_verified(
                                grants,
                                caller,
                                &origin,
                                Action::Decrypt,
                                &self.records.key_ownership,
                            )
                        {
                            return Ok(self
                                .runtime_budget
                                .file_denied(HumanFileOriginStage::KeyGrant));
                        }
                    }
                    SecretOrigin::QueueItem {
                        queue_id,
                        queue_ref,
                        pack_ref,
                        ..
                    } => {
                        let mut context = caller.clone();
                        context.managing_pack_id = pack_ref
                            .as_ref()
                            .and_then(|pack| {
                                self.records.queue_origin_packs.get(&(
                                    queue_id,
                                    queue_ref.clone(),
                                    pack.clone(),
                                ))
                            })
                            .copied();
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
                        if let (Some(leaves), Some(shared)) =
                            (&leaves, self.runtime_budget.receipt_guard.clone())
                        {
                            let origin = SecretOrigin::Entity {
                                entity_type: entity_type.clone(),
                                entity_id,
                                path: path.clone(),
                            };
                            let mut guard = shared.lock().await;
                            if let Some(source) = guard.frontier().prepared_wait_completion_source(
                                Some(leaves),
                                &self.runtime_budget.wait_scopes,
                                &origin,
                            )? {
                                let node = (entity_type.clone(), entity_id, path.clone());
                                if !visited.insert(node.clone()) {
                                    return Ok(false);
                                }
                                if !self.authorize_wait_output(&mut guard, &source).await? {
                                    return Ok(false);
                                }
                                let scope = source.scope_node();
                                let added_scope =
                                    self.runtime_budget.wait_scopes.insert(scope.clone());
                                let rows = source.rows_for(&origin)?;
                                drop(guard);
                                for row in rows {
                                    let site = source.row_site(&row)?;
                                    let mut records = DisclosureRecords::default();
                                    records.inherit_frozen_parents(self.records)?;
                                    records
                                        .prepare(
                                            &state.db,
                                            user,
                                            grants,
                                            [(&*row.source_kind, row.source_ref.as_deref())],
                                        )
                                        .await?;
                                    let mut child = OriginAuthorization {
                                        state,
                                        user,
                                        caller,
                                        grants,
                                        records: &mut records,
                                        runtime_budget: &mut *self.runtime_budget,
                                    };
                                    if !child
                                        .allow_classified(
                                            (&row.source_kind, row.source_ref.as_deref()),
                                            visited,
                                            depth + 1,
                                            site,
                                            Some(leaves.clone()),
                                        )
                                        .await?
                                    {
                                        return Ok(false);
                                    }
                                }
                                if added_scope {
                                    self.runtime_budget.wait_scopes.remove(&scope);
                                }
                                visited.remove(&node);
                                source.ensure_fresh()?;
                                continue;
                            }
                            if let Some(source) = guard.frontier().prepared_success_source(
                                Some(leaves),
                                &self.runtime_budget.success_scopes,
                                &origin,
                            )? {
                                let node = (entity_type.clone(), entity_id, path.clone());
                                if !visited.insert(node.clone()) {
                                    return Ok(false);
                                }
                                if !self.authorize_success_output(&mut guard, &source).await? {
                                    return Ok(false);
                                }
                                let scope = source.scope_node();
                                let added =
                                    self.runtime_budget.success_scopes.insert(scope.clone());
                                let rows = source.rows_for(&origin)?;
                                drop(guard);
                                for row in rows {
                                    let site = source.row_site(&row)?;
                                    let mut records = DisclosureRecords::default();
                                    records.inherit_frozen_parents(self.records)?;
                                    records
                                        .prepare(
                                            &state.db,
                                            user,
                                            grants,
                                            [(&*row.source_kind, row.source_ref.as_deref())],
                                        )
                                        .await?;
                                    let mut child = OriginAuthorization {
                                        state,
                                        user,
                                        caller,
                                        grants,
                                        records: &mut records,
                                        runtime_budget: &mut *self.runtime_budget,
                                    };
                                    if !child
                                        .allow_classified(
                                            (&row.source_kind, row.source_ref.as_deref()),
                                            visited,
                                            depth + 1,
                                            site,
                                            Some(leaves.clone()),
                                        )
                                        .await?
                                    {
                                        return Ok(false);
                                    }
                                }
                                if added {
                                    self.runtime_budget.success_scopes.remove(&scope);
                                }
                                visited.remove(&node);
                                source.ensure_fresh()?;
                                continue;
                            }
                            if let Some(source) =
                                guard.frontier().prepared_cache_source(leaves, &origin)?
                            {
                                if user.claims.token_type
                                    != attune_common::auth::jwt::TokenType::Access
                                {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::NativeTokenKind));
                                }
                                if !visited.insert((entity_type.clone(), entity_id, path.clone())) {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::NativeCycle));
                                }
                                if !self.records.frozen_parent_matches(&source.frozen_parent()) {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::NativeParent));
                                }
                                self.runtime_budget.nodes += 1;
                                self.runtime_budget.bytes = self
                                    .runtime_budget
                                    .bytes
                                    .checked_add(source.retained_bytes())
                                    .ok_or_else(|| {
                                        ApiError::Forbidden(
                                            "Receipt disclosure exceeds its bound".into(),
                                        )
                                    })?;
                                if self.runtime_budget.nodes > 256
                                    || self.runtime_budget.bytes > 1024 * 1024
                                {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::NativeBounds));
                                }
                                let target = attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve_native_cache_source(
                                    guard.connection(), &source, caller.identity_id).await?;
                                let mut context = target.context().clone();
                                context.identity_attributes = caller.identity_attributes.clone();
                                for action in [Action::Read, Action::Decrypt] {
                                    if !grants.iter().any(|grant| {
                                        grant.allows(target.resource(), action, &context)
                                    }) {
                                        return Ok(self.runtime_budget.file_denied(
                                            if action == Action::Read {
                                                HumanFileOriginStage::NativeRead
                                            } else {
                                                HumanFileOriginStage::NativeDecrypt
                                            },
                                        ));
                                    }
                                }
                                source.ensure_fresh()?;
                                guard.frontier().ensure_fresh()?;
                                visited.remove(&(entity_type, entity_id, path));
                                continue;
                            }
                            if let Some(source) = guard
                                .frontier()
                                .prepared_execution_wait(leaves, &origin)?
                                .or(guard.frontier().prepared_related_wait(leaves, &origin)?)
                            {
                                if user.claims.token_type
                                    != attune_common::auth::jwt::TokenType::Access
                                {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::NativeTokenKind));
                                }
                                if !visited.insert((entity_type.clone(), entity_id, path.clone())) {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::NativeCycle));
                                }
                                if !self.records.frozen_parent_matches(&source.frozen_parent()) {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::NativeParent));
                                }
                                self.runtime_budget.nodes += source.retained_nodes();
                                self.runtime_budget.bytes = self
                                    .runtime_budget
                                    .bytes
                                    .checked_add(source.retained_bytes())
                                    .ok_or_else(|| {
                                        ApiError::Forbidden(
                                            "Receipt disclosure exceeds its bound".into(),
                                        )
                                    })?;
                                if self.runtime_budget.nodes > 256
                                    || self.runtime_budget.bytes > 1024 * 1024
                                {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::NativeBounds));
                                }
                                let target = if source.metadata().source_kind()
                                    == "execution_result"
                                {
                                    attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve_execution_wait_source(
                                        guard.connection(), &source, caller.identity_id).await?
                                } else {
                                    attune_common::repositories::runtime_target::RuntimeTargetRepository::resolve_related_wait_source(
                                        guard.connection(), &source, caller.identity_id).await?
                                };
                                let mut context = target.context().clone();
                                context.identity_attributes = caller.identity_attributes.clone();
                                for action in [Action::Read, Action::Decrypt] {
                                    if !grants.iter().any(|grant| {
                                        grant.allows(target.resource(), action, &context)
                                    }) {
                                        return Ok(self.runtime_budget.file_denied(
                                            if action == Action::Read {
                                                HumanFileOriginStage::NativeRead
                                            } else {
                                                HumanFileOriginStage::NativeDecrypt
                                            },
                                        ));
                                    }
                                }
                                // This is the exact authenticated native source
                                // kind/site, not execution_config or an arbitrary
                                // Local/public classification. Other
                                // original field/receipt origins are still in
                                // the same enclosing canonical provenance loop.
                                guard.frontier().ensure_fresh()?;
                                visited.remove(&(entity_type, entity_id, path));
                                continue;
                            }
                        }
                        if matches!(
                            entity_type.as_str(),
                            attune_common::repositories::event_origin::EVENT_SOURCE_PAYLOAD
                                | attune_common::repositories::event_origin::EVENT_SOURCE_CONFIG
                        ) {
                            if user.claims.token_type != attune_common::auth::jwt::TokenType::Access
                                || !visited.insert((entity_type.clone(), entity_id, path.clone()))
                            {
                                return Ok(false);
                            }
                            let Some(scope) = self.records.exact_event_scopes.get(&entity_id)
                            else {
                                return Ok(false);
                            };
                            if !scope.allows(caller, grants) {
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
                        if entity_type
                            == attune_common::repositories::runtime_read::ENTITY_RUNTIME_READ
                        {
                            use attune_common::repositories::{
                                artifact::ArtifactVersionRepository,
                                runtime_read::{RuntimeReadOwner, RuntimeReadTarget},
                                runtime_target::{RuntimeTarget, RuntimeTargetRepository},
                            };
                            if user.claims.token_type != attune_common::auth::jwt::TokenType::Access
                                || !path.is_empty()
                                || !visited.insert((entity_type.clone(), entity_id, path.clone()))
                            {
                                return Ok(self
                                    .runtime_budget
                                    .file_denied(HumanFileOriginStage::ReadIdentity));
                            }
                            let Some(read) = self.records.runtime_reads.get(&entity_id).cloned()
                            else {
                                return Ok(self
                                    .runtime_budget
                                    .file_denied(HumanFileOriginStage::ReadMissing));
                            };
                            self.runtime_budget.nodes += 1
                                + read.parents.len()
                                + read
                                    .provenance
                                    .iter()
                                    .map(|parent| parent.origins.len())
                                    .sum::<usize>();
                            self.runtime_budget.bytes += serde_json::to_vec(&read)
                                .map_err(|_| {
                                    ApiError::Forbidden(
                                        "Runtime read evidence is unavailable".into(),
                                    )
                                })?
                                .len();
                            if self.runtime_budget.nodes > 256
                                || self.runtime_budget.bytes > 1024 * 1024
                            {
                                return Ok(self
                                    .runtime_budget
                                    .file_denied(HumanFileOriginStage::ReadBounds));
                            }
                            let executions = match &read.target {
                                RuntimeReadTarget::Execution { execution }
                                | RuntimeReadTarget::ExecutionMetadata { execution }
                                | RuntimeReadTarget::LiveLog { execution, .. } => vec![*execution],
                                RuntimeReadTarget::ArtifactVersion { execution, .. } => {
                                    execution.iter().copied().collect()
                                }
                                RuntimeReadTarget::Artifact { executions, .. } => {
                                    executions.clone()
                                }
                            };
                            for id in executions {
                                let Some(execution) = self.records.execution(id) else {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::ReadExecutionMissing));
                                };
                                let actions: &[Action] = if matches!(
                                    &read.target,
                                    RuntimeReadTarget::ExecutionMetadata { .. }
                                ) {
                                    &[Action::Read]
                                } else {
                                    &[Action::Read, Action::Decrypt]
                                };
                                for &action in actions {
                                    crate::routes::executions::authorize_execution_projection(
                                        state,
                                        user,
                                        &execution,
                                        action,
                                        None,
                                        &mut self.records.visibility,
                                    )
                                    .await
                                    .map_err(|error| {
                                        self.runtime_budget.file_error(
                                            HumanFileOriginStage::ReadExecutionRights,
                                            error,
                                        )
                                    })?;
                                }
                                if matches!(
                                    &read.target,
                                    RuntimeReadTarget::ExecutionMetadata { .. }
                                ) {
                                    continue;
                                }
                                let binding = {
                                    let mut connection = state.db.acquire().await?;
                                    attune_common::repositories::schema_contract::SchemaContractRepository::find(
                                        &mut connection,
                                        attune_common::repositories::schema_contract::BindingOwner::Execution,
                                        id,
                                    ).await?
                                };
                                if binding.is_none() {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::ReadExecutionBinding));
                                }
                            }
                            let artifact = match &read.target {
                                RuntimeReadTarget::Artifact {
                                    artifact, owner, ..
                                } => Some((*artifact, owner)),
                                RuntimeReadTarget::ArtifactVersion {
                                    artifact, owner, ..
                                } => Some((*artifact, owner)),
                                RuntimeReadTarget::LiveLog {
                                    artifact, owner, ..
                                } => Some((*artifact, owner)),
                                RuntimeReadTarget::Execution { .. }
                                | RuntimeReadTarget::ExecutionMetadata { .. } => None,
                            };
                            if let Some((id, owner)) = artifact {
                                let shared = self.runtime_budget.receipt_guard.clone();
                                let mut held = if let Some(shared) = &shared {
                                    Some(shared.lock().await)
                                } else {
                                    None
                                };
                                let mut transaction = if held.is_none() {
                                    Some(state.db.begin().await?)
                                } else {
                                    None
                                };
                                let conn = if let Some(held) = &mut held {
                                    held.connection()
                                } else {
                                    &mut **transaction.as_mut().ok_or_else(|| {
                                        ApiError::Forbidden(
                                            "Disclosure transaction unavailable".into(),
                                        )
                                    })?
                                };
                                let Some(target) = RuntimeTargetRepository::resolve(
                                    conn,
                                    RuntimeTarget::Artifact(id),
                                    caller.identity_id,
                                )
                                .await?
                                else {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::ArtifactMissing));
                                };
                                if RuntimeReadOwner::of(&target) != *owner {
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::ArtifactOwner));
                                }
                                let mut context = target.context().clone();
                                context.identity_attributes = caller.identity_attributes.clone();
                                let actions: &[Action] = match &read.target {
                                    RuntimeReadTarget::Artifact { .. } => &[Action::Read],
                                    _ => &[Action::Read, Action::Decrypt],
                                };
                                for &action in actions {
                                    if !grants.iter().any(|grant| grant.allows(target.resource(), action, &context)
                                        && (context.visibility != Some(attune_common::models::ArtifactVisibility::Private) || grant.constraints.is_some())) {
                                        let unscoped_private = context.visibility == Some(attune_common::models::ArtifactVisibility::Private)
                                            && grants.iter().any(|grant| grant.constraints.is_none() && grant.allows(target.resource(), action, &context));
                                        let stage = match (action, unscoped_private) {
                                            (Action::Read, true) => HumanFileOriginStage::PrivateArtifactReadScope,
                                            (Action::Decrypt, true) => HumanFileOriginStage::PrivateArtifactDecryptScope,
                                            (Action::Read, false) => HumanFileOriginStage::ArtifactRead,
                                            _ => HumanFileOriginStage::ArtifactDecrypt,
                                        };
                                        return Ok(self.runtime_budget.file_denied(stage));
                                    }
                                }
                                if let RuntimeReadTarget::ArtifactVersion {
                                    version: selected_id,
                                    execution,
                                    encrypted_manifest,
                                    ..
                                } = &read.target
                                {
                                    let Some(version) =
                                        ArtifactVersionRepository::find_pinned_for_share(
                                            conn,
                                            *selected_id,
                                        )
                                        .await?
                                    else {
                                        return Ok(self
                                            .runtime_budget
                                            .file_denied(HumanFileOriginStage::VersionMissing));
                                    };
                                    if version.artifact != id || version.execution != *execution {
                                        return Ok(self
                                            .runtime_budget
                                            .file_denied(HumanFileOriginStage::VersionIdentity));
                                    }
                                    if matches!(version.body_state, Some(attune_common::models::ArtifactBodyState::Pending
                                        | attune_common::models::ArtifactBodyState::Deleting
                                        | attune_common::models::ArtifactBodyState::CleanupClaimed)) { return Ok(self.runtime_budget.file_denied(HumanFileOriginStage::VersionState)); }
                                    if let Some(manifest) = encrypted_manifest {
                                        if version
                                            .meta
                                            .as_ref()
                                            .and_then(|meta| meta.get("encrypted_log_manifest"))
                                            .and_then(Value::as_str)
                                            != Some(manifest.as_str())
                                            || version
                                                .meta
                                                .as_ref()
                                                .and_then(|meta| meta.get("log_encoding"))
                                                .and_then(Value::as_str)
                                                != Some(attune_common::runtime_read_log::ENCODING)
                                            || !ArtifactVersionRepository::log_is_sealed(
                                                conn,
                                                *selected_id,
                                            )
                                            .await?
                                        {
                                            return Ok(self.runtime_budget.file_denied(
                                                HumanFileOriginStage::VersionManifest,
                                            ));
                                        }
                                    }
                                }
                                if let RuntimeReadTarget::LiveLog {
                                    artifact,
                                    version,
                                    execution,
                                    stream,
                                    ..
                                } = &read.target
                                {
                                    attune_common::repositories::runtime_read::RuntimeReadRepository::validate_log_target(conn, *artifact, *version, *execution, *stream).await?;
                                }
                                if let Some(transaction) = transaction {
                                    transaction.commit().await?;
                                }
                            }
                            if !read.provenance.is_empty() {
                                // Each immutable read has its own parent snapshot.
                                // Never overwrite a request-global mutable row and
                                // reuse it for another read's different revision.
                                let mut child_records = DisclosureRecords::default();
                                let execution_ids = read
                                    .parents
                                    .iter()
                                    .filter(|parent| {
                                        matches!(
                                            parent.entity_type.as_str(),
                                            ENTITY_EXECUTION_CONFIG
                                                | ENTITY_EXECUTION_RESULT
                                                | "workflow_variables"
                                        )
                                    })
                                    .map(|parent| parent.entity_id)
                                    .collect::<Vec<_>>();
                                let mut rows = Vec::new();
                                for parent in &read.parents {
                                    rows.push(SecretProvenanceRow {
                                        entity_type: parent.entity_type.clone(),
                                        entity_id: parent.entity_id,
                                        json_path: parent.path.clone(),
                                        source_kind: "provenance".into(),
                                        source_ref: Some(
                                            serde_json::to_string(&parent.provenance).map_err(
                                                |_| {
                                                    ApiError::Forbidden(
                                                        "Runtime read evidence is unavailable"
                                                            .into(),
                                                    )
                                                },
                                            )?,
                                        ),
                                    });
                                }
                                child_records.seed_execution_provenance(&execution_ids, rows);
                                child_records.retain_frozen_parents(&read.parents)?;
                                let references = read
                                    .provenance
                                    .iter()
                                    .map(serde_json::to_string)
                                    .collect::<Result<Vec<_>, _>>()
                                    .map_err(|_| {
                                        ApiError::Forbidden(
                                            "Runtime read evidence is unavailable".into(),
                                        )
                                    })?;
                                child_records
                                    .prepare(
                                        &state.db,
                                        user,
                                        grants,
                                        references.iter().map(|reference| {
                                            ("provenance", Some(reference.as_str()))
                                        }),
                                    )
                                    .await?;
                                let mut child = OriginAuthorization {
                                    state,
                                    user,
                                    caller,
                                    grants,
                                    records: &mut child_records,
                                    runtime_budget: &mut *self.runtime_budget,
                                };
                                for reference in references {
                                    if !child
                                        .allow(("provenance", Some(&reference)), visited, depth + 1)
                                        .await?
                                    {
                                        return Ok(child
                                            .runtime_budget
                                            .file_denied(HumanFileOriginStage::ReadChildren));
                                    }
                                }
                            }
                            visited.remove(&(entity_type, entity_id, path));
                            continue;
                        }
                        if visited.iter().any(|(kind, _, _)| kind == attune_common::repositories::runtime_read::ENTITY_RUNTIME_READ)
                            && matches!(entity_type.as_str(),
                                attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_CONFIG
                                | attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG
                                | attune_common::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG)
                        {
                            if !visited.insert((entity_type.clone(), entity_id, path.clone()))
                                || !generic_sensor_origin_allowed(self.records, caller, grants, &entity_type, entity_id, &path)
                            { return Ok(false); }
                            let Some(parent) = self.records.parent(&entity_type, entity_id, &path) else { return Ok(false); };
                            if !self.allow((&parent.source_kind, parent.source_ref.as_deref()), visited, depth + 1).await? { return Ok(false); }
                            visited.remove(&(entity_type, entity_id, path));
                            continue;
                        }
                        if entity_type == attune_common::repositories::component_configuration::ENTITY_COMPONENT_CONFIGURATION {
                         if !visited.insert((entity_type.clone(), entity_id, path.clone())) {return Ok(false);}
                         let Some(revision) = self.records.components.get(&entity_id).and_then(Option::as_ref) else {return Ok(false)};
                         if !revision.owner_survives {return Ok(false);}
                        let mut context = caller.clone();
                        context.target_id = Some(revision.owner_id);
                        context.managing_pack_id = revision.managing_pack_id;
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
                            // The old ID-only source cannot establish a native
                            // partition identity, even if today's lookup happens
                            // to return a single row. There is no automatic rebind.
                            return Ok(false);
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
                                    self.records
                                        .visibility
                                        .managing_pack(inquiry.created_by_execution),
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
                            && entity_type != "workflow_variables"
                        {
                            return Ok(false);
                        }
                        if !visited.insert((entity_type.clone(), entity_id, path.clone())) {
                            return Ok(false);
                        }
                        let Some(execution) = self.records.execution(entity_id) else {
                            return Ok(self
                                .runtime_budget
                                .file_denied(HumanFileOriginStage::EntityMissing));
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
                                    return Ok(self
                                        .runtime_budget
                                        .file_denied(HumanFileOriginStage::EntityRights))
                                }
                                Err(error) => return Err(error),
                            }
                        }
                        if leaves.as_ref().is_some_and(|leaves| {
                            leaves.is_public_source(&entity_type, entity_id, &path)
                                || leaves.is_outcome_metadata(&entity_type, entity_id, &path)
                        }) {
                            visited.remove(&(entity_type, entity_id, path));
                            continue;
                        }
                        let Some(parent) = self.records.parent(&entity_type, entity_id, &path)
                        else {
                            return Ok(self
                                .runtime_budget
                                .file_denied(HumanFileOriginStage::EntityParent));
                        };
                        if !self
                            .allow_at(
                                (&parent.source_kind, parent.source_ref.as_deref()),
                                visited,
                                depth + 1,
                                attune_common::repositories::immutable_input_origin::input_value_site(
                                    &entity_type, entity_id, &path,
                                ).ok(),
                            )
                            .await?
                        {
                            return Ok(false);
                        }
                        visited.remove(&(entity_type, entity_id, path));
                    }
                    SecretOrigin::Local { source_kind, .. } => {
                        if visited.iter().any(|(kind, _, _)| {
                            kind == attune_common::repositories::runtime_read::ENTITY_RUNTIME_READ
                        }) && !crate::recorded_disclosure::local_origin_allowed(
                            &source_kind,
                            visited,
                        ) {
                            return Ok(self
                                .runtime_budget
                                .file_denied(HumanFileOriginStage::LocalSource));
                        }
                        // Entity-level decrypt was checked by the route. Unknown sources,
                        // including unbound key references, never establish key disclosure.
                        if !matches!(
                            source_kind.as_str(),
                            "parameter_schema" | "pack_config" | "queue_item" | "trigger_schema"
                        ) {
                            return Ok(self
                                .runtime_budget
                                .file_denied(HumanFileOriginStage::LocalSource));
                        }
                    }
                }
            }
            self.runtime_budget.receipt_freshness.ensure_fresh()?;
            Ok(true)
        })
    }
}

/// The generic wrapper keeps its own immutable parents and budgets, but uses G's
/// prepared exact human source proofs instead of a competing per-origin loader.
fn generic_sensor_origin_allowed(
    records: &DisclosureRecords,
    caller: &AuthorizationContext,
    grants: &[Grant],
    entity: &str,
    source: i64,
    path: &str,
) -> bool {
    use attune_common::repositories::sensor_schema::ENTITY_SENSOR_RULE_DELIVERY_CONFIG;
    let generation = if entity == ENTITY_SENSOR_RULE_DELIVERY_CONFIG {
        let Some(delivery) = records.deliveries.get(&source).and_then(Option::as_ref) else {
            return false;
        };
        delivery.generation_binding
    } else {
        source
    };
    let Some(generation) = records
        .generations
        .get(&generation)
        .and_then(Option::as_ref)
    else {
        return false;
    };
    // This is an explicit compile-time dependency on G's prepared-proof cache,
    // not a fallback to the old display-ref-only ordinary reader.
    if !records.sensor_rule_proofs.generation_matches(generation) {
        return false;
    }
    if entity == ENTITY_SENSOR_RULE_DELIVERY_CONFIG {
        sensor_delivery_cached_scope(records, caller, grants, source)
    } else {
        sensor_generation_cached_scope(records, caller, grants, entity, source, path)
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
    let Some(caller) = records
        .sensor_caller
        .as_ref()
        .filter(|fresh| fresh.identity_id == caller.identity_id)
    else {
        return false;
    };
    use attune_common::repositories::sensor_schema::ENTITY_SENSOR_GENERATION_RULE_CONFIG;
    let Some(generation) = records.generations.get(&entity_id).and_then(Option::as_ref) else {
        return false;
    };
    let binding = &generation.binding;
    let mut context = caller.clone();
    context.managing_pack_id = generation.managing_pack_id;
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
        let Some(source) = crate::sensor_origin_iam::RuleOriginSource::generation(generation, path)
        else {
            return false;
        };
        if records.rules.get(&source.rule()).and_then(Option::as_deref) != Some(source.reference())
        {
            return false;
        }
        let Some(pack) = records
            .sensor_rule_proofs
            .pack_for(&source, records.rule_pack_ids.get(&source.rule()).copied())
        else {
            return false;
        };
        context.target_id = Some(source.rule());
        context.managing_pack_id = Some(pack);
        context.target_ref = Some(source.reference().into());
        context.pack_ref = Some(source.pack_ref().into());
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
    let Some(caller) = records
        .sensor_caller
        .as_ref()
        .filter(|fresh| fresh.identity_id == caller.identity_id)
    else {
        return false;
    };
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
    let source = crate::sensor_origin_iam::RuleOriginSource::delivery(delivery);
    let Some(pack) = records
        .sensor_rule_proofs
        .pack_for(&source, records.rule_pack_ids.get(&delivery.rule).copied())
    else {
        return false;
    };
    sensor_delivery_scope_allows(
        caller,
        grants,
        &generation.binding,
        delivery.rule,
        &delivery.rule_ref,
        &delivery.pack_ref,
        generation.managing_pack_id,
        Some(pack),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Compare every recorded sensor and rule ownership field against current caller grants"
)]
fn sensor_delivery_scope_allows(
    caller: &AuthorizationContext,
    grants: &[Grant],
    binding: &attune_common::sensor_schema::SensorGenerationBinding,
    rule: i64,
    reference: &str,
    pack: &str,
    sensor_managing_pack: Option<i64>,
    rule_managing_pack: Option<i64>,
) -> bool {
    let mut context = caller.clone();
    context.managing_pack_id = sensor_managing_pack;
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
    context.managing_pack_id = rule_managing_pack;
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
