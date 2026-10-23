//! Immutable configuration revisions and secret-safe mutable projections.
use serde_json::Value;
use sqlx::{Executor, PgConnection, Postgres};

use super::{
    execution_secret_value::ExecutionSecretValueRepository, pack_release::PackReleaseRepository,
    schema_contract::SchemaContractRepository, schema_type::SchemaTypeRepository,
};
use crate::{
    secret_values::{
        merge_contract_secret_redactions, prepare_secret_values, restore_secret_values,
    },
    value_schema::{CompiledContract, Contract, SchemaContext, SourceIdentity},
    Error, Result,
};

pub const ENTITY_COMPONENT_CONFIGURATION: &str = "component_configuration";
pub struct ComponentConfigurationRepository;

#[derive(sqlx::FromRow)]
struct ConfigurationLayers {
    defaults_revision: i64,
    override_fields: Vec<String>,
}

/// These selectors grant capabilities to sensor code, not ordinary values.
pub fn sensor_authority_field(field: &str) -> bool {
    matches!(
        field,
        "input_permission_set_refs" | "cache_permission_set_refs" | "signing_permission_set_refs"
    )
}

fn contains_configuration_template(value: &Value) -> bool {
    match value {
        Value::String(value) => value.contains("{{") || value.contains("{%"),
        Value::Array(values) => values.iter().any(contains_configuration_template),
        Value::Object(values) => values.values().any(contains_configuration_template),
        _ => false,
    }
}

/// Each entry replaces one complete field. Null is a value, never a reset.
fn overlay_fields(defaults: &Value, overrides: &Value, fields: &[String]) -> Result<Value> {
    let mut effective = defaults
        .as_object()
        .cloned()
        .ok_or_else(|| Error::validation("Configuration must be an object"))?;
    let overrides = overrides
        .as_object()
        .ok_or_else(|| Error::validation("Configuration must be an object"))?;
    for field in fields {
        if let Some(value) = overrides.get(field) {
            effective.insert(field.clone(), value.clone());
        }
    }
    Ok(Value::Object(effective))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConfigurationRevision {
    pub id: i64,
    pub owner_kind: String,
    pub owner_id: i64,
    pub owner_ref: String,
    pub pack_ref: String,
    pub contract: Value,
    pub binding_digest: String,
    pub config: Value,
}

const COLUMNS: &str = "r.id, r.owner_kind, r.owner_id, r.owner_ref, r.pack_ref, r.contract, r.binding_digest, r.config";

#[derive(Debug, Clone)]
pub struct ConfigurationDisclosureRow {
    pub managing_pack_id: Option<i64>,
    pub id: i64,
    pub owner_kind: String,
    pub owner_id: i64,
    pub owner_ref: String,
    pub pack_ref: String,
    pub owner_survives: bool,
}

#[derive(sqlx::FromRow)]
struct StoredConfigurationDisclosure {
    managing_pack_id: Option<i64>,
    id: i64,
    owner_kind: String,
    owner_id: i64,
    owner_ref: String,
    pack_ref: String,
    contract: Value,
    binding_digest: String,
    owner_survives: bool,
}

fn validate_configuration_contract(
    body: &Value,
    binding_digest: &str,
    owner_ref: &str,
) -> Result<Contract> {
    let contract: Contract = serde_json::from_value(body.clone())
        .map_err(|_| Error::validation("Invalid configuration contract"))?;
    CompiledContract::new(&contract)
        .map_err(|_| Error::validation("Invalid configuration contract"))?;
    if contract.binding_digest != binding_digest
        || contract.source.component_ref() != owner_ref
        || contract.context != SchemaContext::FlatDocument
    {
        return Err(Error::validation(
            "Configuration contract ownership mismatch",
        ));
    }
    Ok(contract)
}

impl ConfigurationRevision {
    pub fn contract(&self) -> Result<Contract> {
        validate_configuration_contract(&self.contract, &self.binding_digest, &self.owner_ref)
    }
}

impl ComponentConfigurationRepository {
    pub async fn find_disclosure_rows<'e, E>(
        executor: E,
        ids: &[i64],
    ) -> Result<Vec<ConfigurationDisclosureRow>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let rows: Vec<StoredConfigurationDisclosure> = sqlx::query_as(
            "SELECT r.id, r.owner_kind, r.owner_id, r.owner_ref, r.pack_ref, r.contract, r.binding_digest, \
             CASE r.owner_kind \
               WHEN 'pack' THEN EXISTS (SELECT 1 FROM pack p WHERE p.id = r.owner_id AND p.ref = r.owner_ref) \
               WHEN 'sensor' THEN EXISTS (SELECT 1 FROM sensor s WHERE s.id = r.owner_id AND s.ref = r.owner_ref AND s.retired_at IS NULL) \
               ELSE FALSE END AS owner_survives \
             , CASE r.owner_kind \
               WHEN 'pack' THEN (SELECT p.id FROM pack p WHERE p.id = r.owner_id AND p.ref = r.owner_ref) \
               WHEN 'sensor' THEN (SELECT s.pack FROM sensor s JOIN pack p ON p.id = s.pack WHERE s.id = r.owner_id AND s.ref = r.owner_ref AND s.management_origin = 'pack') \
               END AS managing_pack_id \
             FROM component_configuration_revision r WHERE r.id = ANY($1)")
            .bind(ids).fetch_all(executor).await?;
        rows.into_iter()
            .map(|row| {
                validate_configuration_contract(
                    &row.contract,
                    &row.binding_digest,
                    &row.owner_ref,
                )?;
                Ok(ConfigurationDisclosureRow {
                    managing_pack_id: row.managing_pack_id,
                    id: row.id,
                    owner_kind: row.owner_kind,
                    owner_id: row.owner_id,
                    owner_ref: row.owner_ref,
                    pack_ref: row.pack_ref,
                    owner_survives: row.owner_survives,
                })
            })
            .collect()
    }

    pub async fn cleanup_retired(pool: &sqlx::PgPool, limit: u32) -> Result<(usize, usize)> {
        let mut tx = pool.begin().await?;
        SchemaTypeRepository::lock_reclamation(&mut tx).await?;
        let limit = i64::from(limit.min(1000));
        let generations: Vec<i64> = sqlx::query_scalar("WITH expired AS ( \
            SELECT b.id FROM sensor_generation_schema_binding b \
            WHERE NOT EXISTS (SELECT 1 FROM sensor_workload_assignment a WHERE a.workload = b.workload AND a.generation = b.generation AND a.worker IS NOT NULL AND a.lease_expires_at > clock_timestamp()) \
              AND NOT EXISTS (SELECT 1 FROM sensor_log_generation_binding l WHERE l.generation_binding = b.id) \
              AND NOT file_evidence_pinned('sensor_generation_schema_binding',b.id) \
              AND NOT EXISTS (SELECT 1 FROM runtime_secret_origin_reference p WHERE p.source_id = b.id \
                AND p.source_entity IN ('sensor_generation_config','sensor_generation_rule_config')) \
              AND NOT EXISTS (SELECT 1 FROM sensor_rule_config_delivery d JOIN runtime_secret_origin_reference p \
                ON p.source_entity = 'sensor_rule_delivery_config' AND p.source_id = d.id WHERE d.generation_binding = b.id) \
            ORDER BY b.created, b.id LIMIT $1 \
        ) DELETE FROM sensor_generation_schema_binding b USING expired e WHERE b.id = e.id RETURNING b.id")
            .bind(limit).fetch_all(&mut *tx).await?;
        if !generations.is_empty() {
            sqlx::query("DELETE FROM execution_secret_value WHERE entity_type IN ('sensor_generation_config','sensor_generation_rule_config') AND entity_id = ANY($1)")
                .bind(&generations).execute(&mut *tx).await?;
        }
        super::managed_sensor_metadata::ManagedSensorMetadataRepository::cleanup_with_conn(
            &mut tx,
            limit as u32,
        )
        .await?;
        let configurations: Vec<i64> = sqlx::query_scalar("WITH expired AS ( \
             SELECT r.id FROM component_configuration_revision r WHERE r.created < NOW() - INTERVAL '1 hour' \
               AND NOT EXISTS (SELECT 1 FROM component_configuration_current c WHERE c.revision = r.id) \
               AND NOT EXISTS (SELECT 1 FROM component_configuration_layers l WHERE l.defaults_revision = r.id) \
               AND NOT EXISTS (SELECT 1 FROM managed_sensor_metadata_revision m WHERE m.configuration_revision = r.id) \
               AND NOT EXISTS (SELECT 1 FROM runtime_secret_origin_reference p WHERE p.source_entity = 'component_configuration' AND p.source_id = r.id) \
               AND NOT file_evidence_pinned('component_configuration_revision',r.id) \
                AND NOT EXISTS (SELECT 1 FROM runtime_input_configuration_reference p WHERE p.revision = r.id) \
            ORDER BY r.created, r.id LIMIT $1 \
        ) DELETE FROM component_configuration_revision r USING expired e WHERE r.id = e.id RETURNING r.id")
            .bind(limit).fetch_all(&mut *tx).await?;
        if !configurations.is_empty() {
            sqlx::query("DELETE FROM execution_secret_value WHERE entity_type = 'component_configuration' AND entity_id = ANY($1)")
                .bind(&configurations).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok((generations.len(), configurations.len()))
    }
    pub async fn owner_survives(
        connection: &mut PgConnection,
        revision: &ConfigurationRevision,
    ) -> Result<bool> {
        use super::FindById;
        revision.contract()?;
        match revision.owner_kind.as_str() {
            "pack" => Ok(
                super::pack::PackRepository::find_by_id(connection, revision.owner_id)
                    .await?
                    .is_some_and(|owner| owner.r#ref == revision.owner_ref),
            ),
            "sensor" => Ok(super::trigger::SensorRepository::find_by_id(
                connection,
                revision.owner_id,
            )
            .await?
            .is_some_and(|owner| owner.r#ref == revision.owner_ref)),
            _ => Ok(false),
        }
    }
    pub async fn pack_template_values(
        connection: &mut PgConnection,
        pack: &crate::models::Pack,
        authority: Option<&crate::delegation::DelegationAuthority>,
        encryption_key: Option<&str>,
    ) -> Result<(Value, Vec<String>, Option<i64>)> {
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        if let Some(revision) = Self::current(&mut *connection, "pack", pack.id).await? {
            let contract = revision.contract()?;
            let compiled = CompiledContract::new(&contract)
                .map_err(|_| Error::validation("Invalid configuration contract"))?;
            let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
                &mut *connection,
                ENTITY_COMPONENT_CONFIGURATION,
                revision.id,
            )
            .await?;
            if !secrets.is_empty() {
                let mut context = crate::rbac::AuthorizationContext::new(
                    authority
                        .map(|authority| authority.identity_id)
                        .unwrap_or_default(),
                );
                context.target_id = Some(pack.id);
                context.target_ref = Some(pack.r#ref.clone());
                context.pack_ref = Some(pack.r#ref.clone());
                if ![crate::rbac::Action::Read, crate::rbac::Action::Decrypt]
                    .into_iter()
                    .all(|action| {
                        authority.is_some_and(|authority| {
                            authority.allows(crate::rbac::Resource::Packs, action, context.clone())
                        })
                    })
                {
                    let paths = compiled
                        .secret_paths(&revision.config)
                        .map_err(|_| Error::validation("Invalid configuration projection"))?;
                    return Ok((revision.config.clone(), paths, Some(revision.id)));
                }
            }
            let value = Self::restore(connection, &revision, encryption_key).await?;
            let paths = compiled
                .secret_paths(&value)
                .map_err(|_| Error::validation("Invalid bound configuration"))?;
            Ok((value, paths, Some(revision.id)))
        } else {
            let contract = crate::value_schema::compile_inline(
                &pack.r#ref,
                SchemaContext::FlatDocument,
                &pack.conf_schema,
            )
            .map_err(|_| Error::validation("Pack configuration requires its original binding"))?;
            let compiled = CompiledContract::new(&contract)
                .map_err(|_| Error::validation("Invalid pack configuration"))?;
            if compiled.declares_secrets() {
                return Err(Error::PermissionDenied(
                    "Secret pack configuration requires its original binding".into(),
                ));
            }
            compiled
                .validate(&pack.config)
                .map_err(|_| Error::validation("Invalid pack configuration"))?;
            Ok((pack.config.clone(), Vec::new(), None))
        }
    }
    pub async fn bind_deployment(
        connection: &mut PgConnection,
        pack_id: i64,
        authored_pack_config: &Value,
        encryption_key: Option<&str>,
        authority: &crate::delegation::DelegationAuthority,
    ) -> Result<()> {
        use super::FindById;
        let pack = super::pack::PackRepository::find_by_id(&mut *connection, pack_id)
            .await?
            .ok_or_else(|| Error::validation("Configuration pack is unavailable"))?;
        Self::authorize_preserved_migration(
            connection,
            "pack",
            pack.id,
            &pack.r#ref,
            &pack,
            &pack.conf_schema,
            authority,
        )
        .await?;
        Self::bind_release_pack(connection, &pack, authored_pack_config, encryption_key).await?;
        for sensor in
            super::trigger::SensorRepository::find_by_pack(&mut *connection, pack_id).await?
        {
            let declared_for_release: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sensor WHERE id = $1 AND management_origin = 'pack' AND managed_release = $2)",
            ).bind(sensor.id).bind(pack.active_release).fetch_one(&mut *connection).await?;
            {
                Self::authorize_preserved_migration(
                    connection,
                    "sensor",
                    sensor.id,
                    &sensor.r#ref,
                    &pack,
                    sensor
                        .param_schema
                        .as_ref()
                        .unwrap_or(&serde_json::json!({})),
                    authority,
                )
                .await?;
            }
            if !declared_for_release
                || sensor
                    .config
                    .as_ref()
                    .is_some_and(|value| !crate::secret_values::redacted_paths(value).is_empty())
            {
                // Retained/disabled declarations may still carry the public projection.
                // It is not authored defaults and must never replace their encrypted source.
                Self::bind_sensor(connection, &sensor, None, encryption_key).await?;
            } else {
                Self::bind_release_sensor(
                    connection,
                    &sensor,
                    sensor.config.as_ref().unwrap_or(&serde_json::json!({})),
                    encryption_key,
                )
                .await?;
            }
            if declared_for_release {
                super::managed_sensor_metadata::ManagedSensorMetadataRepository::readmit(
                    connection,
                    &sensor,
                    authority.identity_id,
                    encryption_key,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Run under deployment admission before loader projection mutation. This is
    /// the conservative conversion boundary for installations without layer history.
    pub async fn preserve_before_deployment(
        connection: &mut PgConnection,
        pack_id: i64,
        encryption_key: Option<&str>,
    ) -> Result<()> {
        use super::FindById;
        let pack = super::pack::PackRepository::find_by_id(&mut *connection, pack_id)
            .await?
            .ok_or_else(|| Error::validation("Configuration pack is unavailable"))?;
        if Self::current(&mut *connection, "pack", pack.id)
            .await?
            .is_none()
        {
            Self::bind_pack(connection, &pack, None, encryption_key).await?;
        }
        for sensor in
            super::trigger::SensorRepository::find_by_pack(&mut *connection, pack_id).await?
        {
            if Self::current(&mut *connection, "sensor", sensor.id)
                .await?
                .is_none()
            {
                Self::bind_sensor(connection, &sensor, None, encryption_key).await?;
            }
        }
        Ok(())
    }

    pub async fn authorize_preserved_migration(
        connection: &mut PgConnection,
        kind: &str,
        owner: i64,
        reference: &str,
        pack: &crate::models::Pack,
        schema: &Value,
        authority: &crate::delegation::DelegationAuthority,
    ) -> Result<()> {
        let Some(previous) = Self::current(&mut *connection, kind, owner).await? else {
            return Ok(());
        };
        let old = previous.contract()?;
        let selected = SchemaContractRepository::compile_owned_assignment(
            connection,
            SourceIdentity::Authoring {
                component_ref: reference.into(),
            },
            SchemaContext::FlatDocument,
            schema,
            pack,
        )
        .await?;
        if old
            .semantic_digest()
            .map_err(|_| Error::validation("Invalid configuration contract"))?
            == selected
                .semantic_digest()
                .map_err(|_| Error::validation("Invalid configuration contract"))?
        {
            return Ok(());
        };
        if ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *connection,
            ENTITY_COMPONENT_CONFIGURATION,
            previous.id,
        )
        .await?
        .is_empty()
        {
            return Ok(());
        };
        let mut context = crate::rbac::AuthorizationContext::new(authority.identity_id);
        context.target_id = Some(owner);
        context.target_ref = Some(reference.into());
        context.pack_ref = Some(pack.r#ref.clone());
        context.managing_pack_id = match kind {
            "pack" => Some(pack.id),
            "sensor" => super::runtime_target::RuntimeTargetRepository::resolve(
                connection,
                super::runtime_target::RuntimeTarget::Component(
                    super::runtime_target::ComponentTarget::Sensor,
                    owner,
                ),
                authority.identity_id,
            )
            .await?
            .and_then(|target| target.pack_id()),
            _ => None,
        };
        let (resource, read) = match kind {
            "pack" => (crate::rbac::Resource::Packs, crate::rbac::Action::Read),
            "sensor" => (crate::rbac::Resource::Triggers, crate::rbac::Action::Update),
            _ => return Err(Error::validation("Invalid configuration owner")),
        };
        if ![read, crate::rbac::Action::Decrypt]
            .into_iter()
            .all(|action| authority.allows(resource, action, context.clone()))
        {
            return Err(Error::PermissionDenied(
                "Migrating preserved secret configuration requires read and decrypt authority"
                    .into(),
            ));
        }
        Ok(())
    }
    pub async fn find<'e, E>(executor: E, id: i64) -> Result<Option<ConfigurationRevision>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM component_configuration_revision r WHERE r.id = $1"
        ))
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }

    pub async fn current<'e, E>(
        executor: E,
        kind: &str,
        owner: i64,
    ) -> Result<Option<ConfigurationRevision>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM component_configuration_revision r JOIN component_configuration_current c \
            ON c.revision = r.id WHERE c.owner_kind = $1 AND c.owner_id = $2 AND r.owner_kind = c.owner_kind AND r.owner_id = c.owner_id"))
            .bind(kind).bind(owner).fetch_optional(executor).await.map_err(Into::into)
    }

    /// Callers take shared graph admission before locking the configuration owner.
    pub async fn bind_pack(
        connection: &mut PgConnection,
        pack: &crate::models::Pack,
        supplied: Option<&Value>,
        encryption_key: Option<&str>,
    ) -> Result<ConfigurationRevision> {
        Self::bind(
            connection,
            "pack",
            pack.id,
            &pack.r#ref,
            &pack.r#ref,
            pack.active_release,
            &pack.conf_schema,
            &pack.config,
            supplied,
            encryption_key,
        )
        .await
    }

    pub async fn bind_sensor(
        connection: &mut PgConnection,
        sensor: &crate::models::trigger::Sensor,
        supplied: Option<&Value>,
        encryption_key: Option<&str>,
    ) -> Result<ConfigurationRevision> {
        let pack_ref = sensor
            .pack_ref
            .as_deref()
            .ok_or_else(|| Error::validation("Sensor configuration requires a pack"))?;
        let release =
            PackReleaseRepository::find_active_by_pack_ref(&mut *connection, pack_ref).await?;
        Self::bind(
            connection,
            "sensor",
            sensor.id,
            &sensor.r#ref,
            pack_ref,
            release.map(|release| release.id),
            sensor
                .param_schema
                .as_ref()
                .unwrap_or(&serde_json::json!({})),
            sensor.config.as_ref().unwrap_or(&serde_json::json!({})),
            supplied,
            encryption_key,
        )
        .await
    }

    /// Deployment must pass authored candidate defaults, not the preserved pack projection.
    pub async fn bind_release_pack(
        connection: &mut PgConnection,
        pack: &crate::models::Pack,
        defaults: &Value,
        encryption_key: Option<&str>,
    ) -> Result<ConfigurationRevision> {
        Self::bind_layers(
            connection,
            "pack",
            pack.id,
            &pack.r#ref,
            &pack.r#ref,
            pack.active_release,
            &pack.conf_schema,
            &pack.config,
            None,
            &[],
            Some(defaults),
            encryption_key,
        )
        .await
    }

    pub async fn bind_release_sensor(
        connection: &mut PgConnection,
        sensor: &crate::models::trigger::Sensor,
        defaults: &Value,
        encryption_key: Option<&str>,
    ) -> Result<ConfigurationRevision> {
        let pack_ref = sensor
            .pack_ref
            .as_deref()
            .ok_or_else(|| Error::validation("Sensor configuration requires a pack"))?;
        let release =
            PackReleaseRepository::find_active_by_pack_ref(&mut *connection, pack_ref).await?;
        Self::bind_layers(
            connection,
            "sensor",
            sensor.id,
            &sensor.r#ref,
            pack_ref,
            release.map(|release| release.id),
            sensor
                .param_schema
                .as_ref()
                .unwrap_or(&serde_json::json!({})),
            sensor.config.as_ref().unwrap_or(&serde_json::json!({})),
            None,
            &[],
            Some(defaults),
            encryption_key,
        )
        .await
    }

    pub async fn patch_sensor(
        connection: &mut PgConnection,
        sensor: &crate::models::trigger::Sensor,
        supplied: Option<&Value>,
        reset_fields: &[String],
        encryption_key: Option<&str>,
    ) -> Result<ConfigurationRevision> {
        let pack_ref = sensor
            .pack_ref
            .as_deref()
            .ok_or_else(|| Error::validation("Sensor configuration requires a pack"))?;
        let release =
            PackReleaseRepository::find_active_by_pack_ref(&mut *connection, pack_ref).await?;
        Self::bind_layers(
            connection,
            "sensor",
            sensor.id,
            &sensor.r#ref,
            pack_ref,
            release.map(|release| release.id),
            sensor
                .param_schema
                .as_ref()
                .unwrap_or(&serde_json::json!({})),
            sensor.config.as_ref().unwrap_or(&serde_json::json!({})),
            supplied,
            reset_fields,
            None,
            encryption_key,
        )
        .await
    }

    pub async fn patch_pack(
        connection: &mut PgConnection,
        pack: &crate::models::Pack,
        supplied: Option<&Value>,
        reset_fields: &[String],
        encryption_key: Option<&str>,
    ) -> Result<ConfigurationRevision> {
        Self::bind_layers(
            connection,
            "pack",
            pack.id,
            &pack.r#ref,
            &pack.r#ref,
            pack.active_release,
            &pack.conf_schema,
            &pack.config,
            supplied,
            reset_fields,
            None,
            encryption_key,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Keep owner, release, schema, and secret inputs explicit at the binding boundary"
    )]
    async fn bind(
        connection: &mut PgConnection,
        kind: &str,
        owner: i64,
        reference: &str,
        pack_ref: &str,
        release_id: Option<i64>,
        schema: &Value,
        projection: &Value,
        supplied: Option<&Value>,
        encryption_key: Option<&str>,
    ) -> Result<ConfigurationRevision> {
        Self::bind_layers(
            connection,
            kind,
            owner,
            reference,
            pack_ref,
            release_id,
            schema,
            projection,
            supplied,
            &[],
            None,
            encryption_key,
        )
        .await
    }

    /// Caller holds graph admission and the owner mutation lock in a transaction.
    /// `release_defaults` replaces authored defaults, never operator overrides.
    #[expect(
        clippy::too_many_arguments,
        reason = "Authored defaults, overrides, resets, and release identity are independent binding inputs"
    )]
    pub async fn bind_layers(
        connection: &mut PgConnection,
        kind: &str,
        owner: i64,
        reference: &str,
        pack_ref: &str,
        release_id: Option<i64>,
        schema: &Value,
        projection: &Value,
        supplied: Option<&Value>,
        reset_fields: &[String],
        release_defaults: Option<&Value>,
        encryption_key: Option<&str>,
    ) -> Result<ConfigurationRevision> {
        Self::bind_admitted_layers(
            connection,
            kind,
            owner,
            reference,
            pack_ref,
            release_id,
            schema,
            projection,
            supplied,
            reset_fields,
            release_defaults,
            encryption_key,
            false,
        )
        .await
    }

    /// Only the atomic managed-sensor readmission route may use this path; its
    /// transaction must capture metadata/purpose admission before committing.
    pub async fn bind_sensor_readmission(
        connection: &mut PgConnection,
        sensor: &crate::models::trigger::Sensor,
        supplied: Option<&Value>,
        reset: &[String],
        key: Option<&str>,
    ) -> Result<ConfigurationRevision> {
        let pack_ref = sensor
            .pack_ref
            .as_deref()
            .ok_or_else(|| Error::validation("Sensor pack is unavailable"))?;
        let release =
            PackReleaseRepository::find_active_by_pack_ref(&mut *connection, pack_ref).await?;
        Self::bind_admitted_layers(
            connection,
            "sensor",
            sensor.id,
            &sensor.r#ref,
            pack_ref,
            release.map(|release| release.id),
            sensor
                .param_schema
                .as_ref()
                .unwrap_or(&serde_json::json!({})),
            sensor.config.as_ref().unwrap_or(&serde_json::json!({})),
            supplied,
            reset,
            None,
            key,
            true,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Forward independent configuration layers under the caller's admitted transaction"
    )]
    async fn bind_admitted_layers(
        connection: &mut PgConnection,
        kind: &str,
        owner: i64,
        reference: &str,
        pack_ref: &str,
        release_id: Option<i64>,
        schema: &Value,
        projection: &Value,
        supplied: Option<&Value>,
        reset_fields: &[String],
        release_defaults: Option<&Value>,
        encryption_key: Option<&str>,
        readmission: bool,
    ) -> Result<ConfigurationRevision> {
        use super::FindByRef;
        let previous = Self::current(&mut *connection, kind, owner).await?;
        let declaring_pack = super::pack::PackRepository::find_by_ref(&mut *connection, pack_ref)
            .await?
            .ok_or_else(|| Error::validation("Configuration pack is unavailable"))?;
        let defaults_contract = SchemaContractRepository::compile_owned_assignment(
            connection,
            SourceIdentity::Assigned {
                component_ref: reference.into(),
                assignment_id: uuid::Uuid::new_v4(),
            },
            SchemaContext::FlatDocument,
            schema,
            &declaring_pack,
        )
        .await?;
        let layers: Option<ConfigurationLayers> = sqlx::query_as(
            "SELECT defaults_revision, override_fields FROM component_configuration_layers WHERE owner_kind = $1 AND owner_id = $2 FOR UPDATE")
            .bind(kind).bind(owner).fetch_optional(&mut *connection).await?;
        // Preserve exact ciphertext and origins for equivalent metadata-only writes.
        if supplied.is_none() && reset_fields.is_empty() && release_defaults.is_none() {
            if let Some(previous) = &previous {
                if previous
                    .contract()?
                    .semantic_digest()
                    .map_err(|_| Error::validation("Invalid configuration contract"))?
                    == defaults_contract
                        .semantic_digest()
                        .map_err(|_| Error::validation("Invalid configuration contract"))?
                {
                    return Self::bind_value(
                        connection,
                        kind,
                        owner,
                        reference,
                        pack_ref,
                        release_id,
                        schema,
                        projection,
                        None,
                        encryption_key,
                    )
                    .await;
                }
            }
        }
        let previous_value = match &previous {
            Some(previous) => Self::restore(connection, previous, encryption_key).await?,
            None => {
                if !crate::secret_values::redacted_paths(projection).is_empty() {
                    return Err(Error::PermissionDenied(
                        "Configuration requires its original encrypted binding".into(),
                    ));
                }
                projection.clone()
            }
        };
        let mut fields = match &layers {
            Some(layers) => layers.override_fields.clone(),
            None if previous.is_some() || release_defaults.is_some() || supplied.is_none() => {
                previous_value
                    .as_object()
                    .ok_or_else(|| Error::validation("Configuration must be an object"))?
                    .keys()
                    .cloned()
                    .collect()
            }
            None => Vec::new(),
        };
        // A fresh release has no historical operator values to preserve.
        if previous.is_none() && release_defaults.is_some() {
            fields.clear();
        }
        if kind == "sensor" && release_defaults.is_some() {
            // Selectors belong to the newly admitted release, never stale operator values.
            fields.retain(|field| !sensor_authority_field(field));
        }
        let defaults = match release_defaults {
            Some(value) => value.clone(),
            None => match &layers {
                Some(layers) => {
                    let revision = Self::find(&mut *connection, layers.defaults_revision)
                        .await?
                        .ok_or_else(|| {
                            Error::validation("Configuration defaults are unavailable")
                        })?;
                    Self::restore_defaults(connection, &revision, encryption_key).await?
                }
                None => serde_json::json!({}),
            },
        };
        let defaults_compiled = CompiledContract::new(&defaults_contract)
            .map_err(|_| Error::validation("Invalid configuration contract"))?;
        let defaults = defaults_compiled
            .apply_defaults(&defaults)
            .map_err(|_| Error::validation("Invalid configuration defaults"))?;
        let mut overrides = previous_value
            .as_object()
            .cloned()
            .ok_or_else(|| Error::validation("Configuration must be an object"))?;
        if let Some(supplied) = supplied {
            let supplied = supplied
                .as_object()
                .ok_or_else(|| Error::validation("Configuration overrides must be an object"))?;
            for (field, value) in supplied {
                if schema.as_object().is_some_and(|schema| !schema.is_empty())
                    && schema.get(field).is_none()
                    && defaults.get(field).is_none()
                    && previous_value.get(field).is_none()
                    && !(kind == "sensor" && sensor_authority_field(field))
                {
                    return Err(Error::validation("Unknown configuration override field"));
                }
                if reset_fields.contains(field) {
                    return Err(Error::validation(
                        "A configuration field cannot be set and reset together",
                    ));
                }
                if !readmission
                    && kind == "sensor"
                    && sensor_authority_field(field)
                    && previous.is_some()
                    && previous_value.get(field) != Some(value)
                {
                    return Err(Error::validation(
                        "Sensor capability selectors require deployment readmission",
                    ));
                }
                if !readmission
                    && kind == "sensor"
                    && previous.is_some()
                    && previous_value.get(field) != Some(value)
                    && (contains_configuration_template(value)
                        || previous_value
                            .get(field)
                            .is_some_and(contains_configuration_template))
                {
                    return Err(Error::validation(
                        "Sensor configuration template edits require deployment readmission",
                    ));
                }
                // Redacted editor round-trips preserve the original complete field.
                if crate::secret_values::redacted_paths(value).is_empty() {
                    overrides.insert(field.clone(), value.clone());
                } else if previous_value.get(field) != Some(value)
                    && previous
                        .as_ref()
                        .and_then(|revision| revision.config.get(field))
                        != Some(value)
                {
                    return Err(Error::validation("Invalid redacted configuration field"));
                }
                if !fields.contains(field) {
                    fields.push(field.clone());
                }
            }
        }
        for field in reset_fields {
            if !readmission && kind == "sensor" && sensor_authority_field(field) {
                return Err(Error::validation(
                    "Sensor capability selectors require deployment readmission",
                ));
            }
            if !readmission
                && kind == "sensor"
                && (previous_value
                    .get(field)
                    .is_some_and(contains_configuration_template)
                    || defaults
                        .get(field)
                        .is_some_and(contains_configuration_template))
            {
                return Err(Error::validation(
                    "Sensor configuration template reset requires deployment readmission",
                ));
            }
            if !fields.contains(field)
                && schema.get(field).is_none()
                && defaults.get(field).is_none()
                && !(kind == "sensor" && sensor_authority_field(field))
            {
                return Err(Error::validation("Unknown configuration reset field"));
            }
            fields.retain(|candidate| candidate != field);
        }
        fields.sort();
        fields.dedup();
        let effective = overlay_fields(&defaults, &Value::Object(overrides), &fields)?;
        // Defaults need not satisfy required fields that are supplied by overrides.
        // Store them under the same contract without adding missing required values.
        let reusable_defaults = if let Some(layers) = &layers {
            let revision = Self::find(&mut *connection, layers.defaults_revision)
                .await?
                .ok_or_else(|| Error::validation("Configuration defaults are unavailable"))?;
            if revision
                .contract()?
                .semantic_digest()
                .map_err(|_| Error::validation("Invalid configuration contract"))?
                == defaults_contract
                    .semantic_digest()
                    .map_err(|_| Error::validation("Invalid configuration contract"))?
                && Self::restore_defaults(connection, &revision, encryption_key).await? == defaults
            {
                Some(revision.id)
            } else {
                None
            }
        } else {
            None
        };
        let defaults_revision = if let Some(id) = reusable_defaults {
            id
        } else {
            Self::store_defaults(
                connection,
                kind,
                owner,
                reference,
                pack_ref,
                release_id,
                schema,
                &defaults,
                encryption_key,
            )
            .await?
        };
        let revision = Self::bind_value(
            connection,
            kind,
            owner,
            reference,
            pack_ref,
            release_id,
            schema,
            projection,
            Some(&effective),
            encryption_key,
        )
        .await?;
        sqlx::query("INSERT INTO component_configuration_layers (owner_kind,owner_id,defaults_revision,override_fields) VALUES ($1,$2,$3,$4) ON CONFLICT (owner_kind,owner_id) DO UPDATE SET defaults_revision = EXCLUDED.defaults_revision, override_fields = EXCLUDED.override_fields")
            .bind(kind).bind(owner).bind(defaults_revision).bind(&fields).execute(&mut *connection).await?;
        Ok(revision)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Persist defaults with explicit owner, release, schema, and encryption context"
    )]
    async fn store_defaults(
        connection: &mut PgConnection,
        kind: &str,
        owner: i64,
        reference: &str,
        pack_ref: &str,
        _release_id: Option<i64>,
        schema: &Value,
        defaults: &Value,
        encryption_key: Option<&str>,
    ) -> Result<i64> {
        use super::FindByRef;
        let pack = super::pack::PackRepository::find_by_ref(&mut *connection, pack_ref)
            .await?
            .ok_or_else(|| Error::validation("Configuration pack is unavailable"))?;
        let contract = SchemaContractRepository::compile_owned_assignment(
            connection,
            SourceIdentity::Assigned {
                component_ref: reference.into(),
                assignment_id: uuid::Uuid::new_v4(),
            },
            SchemaContext::FlatDocument,
            schema,
            &pack,
        )
        .await?;
        let compiled = CompiledContract::new(&contract)
            .map_err(|_| Error::validation("Invalid configuration contract"))?;
        let (public, secrets) = merge_contract_secret_redactions(defaults.clone(), &[], &compiled)?;
        let secrets = if secrets.is_empty() {
            Vec::new()
        } else {
            prepare_secret_values(
                secrets,
                encryption_key
                    .ok_or_else(|| Error::validation("Secret configuration requires encryption"))?,
            )?
        };
        let id = sqlx::query_scalar("INSERT INTO component_configuration_revision (owner_kind,owner_id,owner_ref,pack_ref,contract,binding_digest,config) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id")
            .bind(kind).bind(owner).bind(reference).bind(pack_ref).bind(serde_json::to_value(&contract)?)
            .bind(&contract.binding_digest).bind(public).fetch_one(&mut *connection).await?;
        ExecutionSecretValueRepository::upsert_many_with_conn(
            connection,
            ENTITY_COMPONENT_CONFIGURATION,
            id,
            &secrets,
        )
        .await?;
        Ok(id)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Keep owner, release, schema, and encryption context explicit when recording a value"
    )]
    async fn bind_value(
        connection: &mut PgConnection,
        kind: &str,
        owner: i64,
        reference: &str,
        pack_ref: &str,
        release_id: Option<i64>,
        schema: &Value,
        projection: &Value,
        supplied: Option<&Value>,
        encryption_key: Option<&str>,
    ) -> Result<ConfigurationRevision> {
        use super::FindByRef;
        let pack = super::pack::PackRepository::find_by_ref(&mut *connection, pack_ref)
            .await?
            .filter(|pack| pack.active_release == release_id)
            .ok_or_else(|| Error::validation("Configuration declaring pack is unavailable"))?;
        let contract = SchemaContractRepository::compile_owned_assignment(
            connection,
            SourceIdentity::Assigned {
                component_ref: reference.to_string(),
                assignment_id: uuid::Uuid::new_v4(),
            },
            SchemaContext::FlatDocument,
            schema,
            &pack,
        )
        .await?;
        SchemaTypeRepository::replace_consumer_references(
            connection,
            pack.id,
            kind,
            reference,
            &[(
                if kind == "pack" {
                    "conf_schema"
                } else {
                    "param_schema"
                },
                Some(schema),
            )],
        )
        .await?;
        let value = match supplied {
            Some(value) => value.clone(),
            None => match Self::current(&mut *connection, kind, owner).await? {
                Some(previous) => {
                    if previous
                        .contract()?
                        .semantic_digest()
                        .map_err(|_| Error::validation("Invalid configuration contract"))?
                        == contract
                            .semantic_digest()
                            .map_err(|_| Error::validation("Invalid configuration contract"))?
                    {
                        let query = if kind == "pack" {
                            "UPDATE pack SET config = $1 WHERE id = $2"
                        } else {
                            "UPDATE sensor SET config = $1 WHERE id = $2"
                        };
                        sqlx::query(query)
                            .bind(&previous.config)
                            .bind(owner)
                            .execute(&mut *connection)
                            .await?;
                        return Ok(previous);
                    }
                    Self::restore(connection, &previous, encryption_key).await?
                }
                None => projection.clone(),
            },
        };
        let compiled = CompiledContract::new(&contract)
            .map_err(|_| Error::validation("Invalid configuration contract"))?;
        // Layer composition already applied defaults before complete-field replacement.
        let value = if supplied.is_none() {
            compiled
                .apply_defaults(&value)
                .map_err(|_| Error::validation("Invalid configuration defaults"))?
        } else {
            value
        };
        if kind == "sensor" {
            compiled
                .validate_authoring(&value)
                .map_err(|_| Error::validation("Invalid sensor configuration fields"))?;
        } else {
            compiled
                .validate(&value)
                .map_err(|_| Error::validation("Invalid configuration fields"))?;
        }
        if let Some(previous) = Self::current(&mut *connection, kind, owner).await? {
            if previous
                .contract()?
                .semantic_digest()
                .map_err(|_| Error::validation("Invalid configuration contract"))?
                == contract
                    .semantic_digest()
                    .map_err(|_| Error::validation("Invalid configuration contract"))?
                && Self::restore(connection, &previous, encryption_key).await? == value
            {
                let query = if kind == "pack" {
                    "UPDATE pack SET config = $1 WHERE id = $2"
                } else {
                    "UPDATE sensor SET config = $1 WHERE id = $2"
                };
                sqlx::query(query)
                    .bind(&previous.config)
                    .bind(owner)
                    .execute(&mut *connection)
                    .await?;
                return Ok(previous);
            }
        }
        let (public, secrets) = merge_contract_secret_redactions(value, &[], &compiled)?;
        let secrets = if secrets.is_empty() {
            Vec::new()
        } else {
            prepare_secret_values(
                secrets,
                encryption_key
                    .ok_or_else(|| Error::validation("Secret configuration requires encryption"))?,
            )?
        };
        let id: i64 = sqlx::query_scalar("INSERT INTO component_configuration_revision \
            (owner_kind,owner_id,owner_ref,pack_ref,contract,binding_digest,config) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id")
            .bind(kind).bind(owner).bind(reference).bind(pack_ref).bind(serde_json::to_value(&contract)?)
            .bind(&contract.binding_digest).bind(&public).fetch_one(&mut *connection).await?;
        ExecutionSecretValueRepository::upsert_many_with_conn(
            connection,
            ENTITY_COMPONENT_CONFIGURATION,
            id,
            &secrets,
        )
        .await?;
        sqlx::query("INSERT INTO component_configuration_current (owner_kind,owner_id,revision) VALUES ($1,$2,$3) \
            ON CONFLICT (owner_kind,owner_id) DO UPDATE SET revision = EXCLUDED.revision")
            .bind(kind).bind(owner).bind(id).execute(&mut *connection).await?;
        let query = match kind {
            "pack" => "UPDATE pack SET config = $1 WHERE id = $2",
            "sensor" => "UPDATE sensor SET config = $1 WHERE id = $2",
            _ => return Err(Error::validation("Invalid configuration owner")),
        };
        sqlx::query(query)
            .bind(public)
            .bind(owner)
            .execute(&mut *connection)
            .await?;
        Self::find(connection, id)
            .await?
            .ok_or_else(|| Error::validation("Configuration revision is unavailable"))
    }

    pub async fn restore(
        connection: &mut PgConnection,
        revision: &ConfigurationRevision,
        encryption_key: Option<&str>,
    ) -> Result<Value> {
        let contract = revision.contract()?;
        let value = Self::restore_defaults(connection, revision, encryption_key).await?;
        let compiled = CompiledContract::new(&contract)
            .map_err(|_| Error::validation("Invalid configuration contract"))?;
        let valid = if revision.owner_kind == "sensor" {
            compiled.validate_authoring(&value).map(|_| ())
        } else {
            compiled.validate(&value)
        };
        valid.map_err(|_| {
            Error::validation("Stored configuration does not match its original contract")
        })?;
        Ok(value)
    }

    async fn restore_defaults(
        connection: &mut PgConnection,
        revision: &ConfigurationRevision,
        encryption_key: Option<&str>,
    ) -> Result<Value> {
        revision.contract()?;
        let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *connection,
            ENTITY_COMPONENT_CONFIGURATION,
            revision.id,
        )
        .await?;
        let value = if secrets.is_empty() {
            revision.config.clone()
        } else {
            restore_secret_values(
                revision.config.clone(),
                &secrets,
                encryption_key
                    .ok_or_else(|| Error::validation("Configuration decryption is unavailable"))?,
            )?
        };
        Ok(value)
    }

    pub async fn lock_selection(connection: &mut PgConnection) -> Result<()> {
        SchemaTypeRepository::lock_selection(connection).await
    }
}
