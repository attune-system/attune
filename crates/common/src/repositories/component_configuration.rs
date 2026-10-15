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
    pub id: i64,
    pub owner_kind: String,
    pub owner_id: i64,
    pub owner_ref: String,
    pub pack_ref: String,
    pub owner_survives: bool,
}

#[derive(sqlx::FromRow)]
struct StoredConfigurationDisclosure {
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
        let configurations: Vec<i64> = sqlx::query_scalar("WITH expired AS ( \
            SELECT r.id FROM component_configuration_revision r WHERE r.created < NOW() - INTERVAL '1 hour' \
              AND NOT EXISTS (SELECT 1 FROM component_configuration_current c WHERE c.revision = r.id) \
              AND NOT EXISTS (SELECT 1 FROM runtime_secret_origin_reference p WHERE p.source_entity = 'component_configuration' AND p.source_id = r.id) \
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
        Self::bind_pack(connection, &pack, None, encryption_key).await?;
        for sensor in
            super::trigger::SensorRepository::find_by_pack(&mut *connection, pack_id).await?
        {
            let supplied = sensor
                .config
                .as_ref()
                .filter(|value| crate::secret_values::redacted_paths(value).is_empty());
            if supplied.is_none() {
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
            Self::bind_sensor(connection, &sensor, supplied, encryption_key).await?;
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
        let value = compiled.apply_defaults(&value).map_err(|error| {
            Error::validation(format!("Invalid configuration defaults: {error}"))
        })?;
        if kind == "sensor" {
            compiled.validate_authoring(&value).map_err(|error| {
                Error::validation(format!("Invalid sensor configuration: {error}"))
            })?;
        } else {
            compiled
                .validate(&value)
                .map_err(|error| Error::validation(format!("Invalid configuration: {error}")))?;
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

    pub async fn lock_selection(connection: &mut PgConnection) -> Result<()> {
        SchemaTypeRepository::lock_selection(connection).await
    }
}
