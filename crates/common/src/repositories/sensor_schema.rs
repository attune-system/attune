use super::{
    execution_secret_value::ExecutionSecretValueRepository,
    pack_release::PackReleaseRepository,
    rule::RuleRepository,
    schema_contract::{check_dependency, SchemaContractRepository},
    trigger::{SensorRepository, TriggerRepository},
    FindById,
};
use crate::{
    event_schema::EventSchemaBinding,
    models::{OwnedSensorWorkload, SensorExecutableSnapshot},
    secret_values::{
        merge_contract_secret_redactions, prepare_secret_values, restore_secret_values,
    },
    sensor_schema::{SensorGenerationBinding, SensorTriggerBinding},
    value_schema::{digest, CompiledContract, SchemaContext, SourceIdentity},
    Error, Result,
};
use serde_json::{json, Value};
use sqlx::{Executor, PgConnection, Postgres};

pub const ENTITY_SENSOR_GENERATION_CONFIG: &str = "sensor_generation_config";
pub const ENTITY_SENSOR_GENERATION_RULE_CONFIG: &str = "sensor_generation_rule_config";
pub const ENTITY_SENSOR_RULE_DELIVERY_CONFIG: &str = "sensor_rule_delivery_config";
pub struct SensorSchemaRepository;

#[derive(Clone)]
pub struct SensorDisclosureRow {
    pub id: i64,
    pub binding: SensorGenerationBinding,
    pub initial_rules: Value,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SensorDeliveryDisclosureRow {
    pub id: i64,
    pub generation_binding: i64,
    pub rule: i64,
    pub rule_ref: String,
    pub pack_ref: String,
}

fn validate_bounded_flat(value: &Value, source_ref: &str) -> Result<()> {
    // An absent schema adds no field constraints, but cannot bypass the shared
    // value-size, traversal, or flat-parameter limits. This preview is not stored.
    let contract =
        crate::value_schema::compile_inline(source_ref, SchemaContext::FlatDocument, &json!({}))
            .map_err(|_| Error::validation("Invalid sensor parameter boundary"))?;
    CompiledContract::new(&contract)
        .map_err(|_| Error::validation("Invalid sensor parameter boundary"))?
        .validate(value)
        .map_err(|error| Error::validation(format!("Invalid sensor rule configuration: {error}")))
}

impl SensorSchemaRepository {
    pub async fn find_disclosure_rows<'e, E>(
        executor: E,
        ids: &[i64],
    ) -> Result<Vec<SensorDisclosureRow>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let rows: Vec<(i64, Value, String, Value)> = sqlx::query_as("SELECT id, binding, binding_digest, \
            (SELECT COALESCE(jsonb_object_agg(entry.key, jsonb_build_object('ref',entry.value->'ref','pack_ref',entry.value->'pack_ref')),'{}'::jsonb) \
             FROM jsonb_each(initial_rules) entry) AS initial_rules \
             FROM sensor_generation_schema_binding WHERE id = ANY($1)")
            .bind(ids).fetch_all(executor).await?;
        rows.into_iter()
            .map(|(id, body, hash, initial_rules)| {
                Ok(SensorDisclosureRow {
                    id,
                    binding: Self::decode_disclosure_binding(body, &hash)?,
                    initial_rules,
                })
            })
            .collect()
    }

    pub async fn find_delivery_disclosure_rows<'e, E>(
        executor: E,
        ids: &[i64],
    ) -> Result<Vec<SensorDeliveryDisclosureRow>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        sqlx::query_as("SELECT id, generation_binding, rule, rule_ref, pack_ref FROM sensor_rule_config_delivery WHERE id = ANY($1)")
            .bind(ids).fetch_all(executor).await.map_err(Into::into)
    }

    fn decode_disclosure_binding(body: Value, hash: &str) -> Result<SensorGenerationBinding> {
        if digest(&body).map_err(|_| Error::validation("Invalid sensor generation digest"))? != hash
        {
            return Err(Error::validation("Sensor generation digest mismatch"));
        }
        let binding: SensorGenerationBinding = serde_json::from_value(body)
            .map_err(|_| Error::validation("Invalid sensor generation binding"))?;
        binding.validate()?;
        Ok(binding)
    }
    pub async fn record_key_read_origin(
        connection: &mut PgConnection,
        generation: i64,
        origin: &crate::secret_provenance::KeyOrigin,
    ) -> Result<()> {
        let body = serde_json::to_value(origin)
            .map_err(|_| Error::validation("Invalid sensor Key read origin"))?;
        let bytes: i64 = sqlx::query_scalar("SELECT octet_length($1::JSONB::TEXT)::BIGINT")
            .bind(&body)
            .fetch_one(&mut *connection)
            .await?;
        if bytes > 16384 {
            return Err(Error::validation(
                "Sensor Key read origin exceeds its byte limit",
            ));
        }
        let hash = crate::value_schema::digest(&body)
            .map_err(|_| Error::validation("Invalid sensor Key read origin"))?;
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM sensor_generation_schema_binding WHERE id = $1 FOR UPDATE",
        )
        .bind(generation)
        .fetch_optional(&mut *connection)
        .await?;
        if exists.is_none() {
            return Err(Error::validation(
                "Sensor Key read generation is unavailable",
            ));
        }
        let repeated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sensor_generation_key_read_origin WHERE generation_binding = $1 AND origin_digest = $2)")
            .bind(generation).bind(&hash).fetch_one(&mut *connection).await?;
        if repeated {
            return Ok(());
        }
        let (count, used): (i64,i64) = sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(octet_length(origin::text)),0)::BIGINT FROM sensor_generation_key_read_origin WHERE generation_binding = $1")
            .bind(generation).fetch_one(&mut *connection).await?;
        if count >= 1024 {
            return Err(Error::validation(
                "Sensor runtime Key read origin limit reached",
            ));
        }
        if used + bytes > 1024 * 1024 {
            return Err(Error::validation(
                "Sensor runtime Key read origins exceed their byte limit",
            ));
        }
        sqlx::query("INSERT INTO sensor_generation_key_read_origin (generation_binding, origin_digest, origin) VALUES ($1,$2,$3)")
            .bind(generation).bind(hash).bind(body).execute(connection).await?;
        Ok(())
    }

    pub async fn key_read_origins<'e, E>(
        executor: E,
        generation: i64,
    ) -> Result<Vec<crate::secret_provenance::KeyOrigin>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let rows: Vec<(String, Value, i64)> = sqlx::query_as("SELECT origin_digest, origin, octet_length(origin::text)::BIGINT FROM sensor_generation_key_read_origin WHERE generation_binding = $1 ORDER BY origin_digest LIMIT 1025")
            .bind(generation).fetch_all(executor).await?;
        if rows.len() > 1024 {
            return Err(Error::validation(
                "Sensor runtime Key read origin limit exceeded",
            ));
        }
        if rows.iter().map(|(_, _, bytes)| bytes).sum::<i64>() > 1024 * 1024 {
            return Err(Error::validation(
                "Sensor runtime Key read origins exceed their byte limit",
            ));
        }
        rows.into_iter()
            .map(|(hash, body, _)| {
                if crate::value_schema::digest(&body)
                    .map_err(|_| Error::validation("Invalid sensor Key read origin"))?
                    != hash
                {
                    return Err(Error::validation("Sensor Key read origin digest mismatch"));
                }
                serde_json::from_value(body)
                    .map_err(|_| Error::validation("Invalid sensor Key read origin"))
            })
            .collect()
    }
    async fn render_inputs(
        connection: &mut PgConnection,
        binding: &SensorGenerationBinding,
        value: &Value,
        rule: Option<&crate::models::Rule>,
        encryption_key: Option<&str>,
    ) -> Result<crate::secret_values::RenderedJson> {
        let sensor = &binding.executable.sensor;
        let mut authority = None;
        let (pack_config, pack_paths, pack_revision) =
            if crate::template_resolver::references_pack_configuration(value) {
                let id = sensor.pack.ok_or_else(|| {
                    Error::validation("Sensor pack configuration owner is unavailable")
                })?;
                let pack = super::pack::PackRepository::find_by_id(&mut *connection, id)
                    .await?
                    .filter(|pack| Some(&pack.r#ref) == sensor.pack_ref.as_ref())
                    .ok_or_else(|| {
                        Error::validation("Sensor pack configuration owner is unavailable")
                    })?;
                authority = Some(Self::input_authority(connection, binding).await?);
                super::component_configuration::ComponentConfigurationRepository::pack_template_values(connection, &pack, authority.as_ref(), encryption_key).await?
            } else {
                (json!({}), Vec::new(), None)
            };
        let context = crate::template_resolver::TemplateContext::new(
            Value::Null,
            pack_config,
            json!({
                "sensor":{"id":sensor.id,"ref":sensor.r#ref},
                "rule":rule.map(|rule| json!({"id":rule.id,"ref":rule.r#ref})),
            }),
        )
        .with_origin("sensor", &sensor.r#ref);
        let context = if let Some(revision) = pack_revision {
            context.with_bound_pack_config_secret_paths(revision, pack_paths)
        } else {
            context
        };
        let expressions = crate::template_resolver::referenced_keystore_expressions(
            value, &context,
        )
        .map_err(|_| Error::validation("Sensor input Key selector could not be resolved"))?;
        let context = if expressions.is_empty() {
            context
        } else {
            let authority = match authority {
                Some(authority) => authority,
                None => Self::input_authority(connection, binding).await?,
            };
            let refs = expressions.values().cloned().collect::<Vec<_>>();
            let keys = crate::key_access::resolve_explicit_keys(
                &mut *connection,
                &authority,
                &refs,
                encryption_key,
            )
            .await?;
            context.with_resolved_keys(&expressions, keys)
        };
        crate::template_resolver::resolve_templates_with_sensitivity(value, &context)
            .map_err(|_| Error::validation("Sensor input template rendering failed"))
    }

    async fn input_authority(
        connection: &mut PgConnection,
        binding: &SensorGenerationBinding,
    ) -> Result<crate::delegation::DelegationAuthority> {
        use crate::repositories::{
            identity::{CreateIdentityInput, IdentityRepository},
            Create,
        };
        let sensor = &binding.executable.sensor;
        let login = format!("sensor:{}", sensor.r#ref);
        let identity = match IdentityRepository::find_by_login(&mut *connection, &login).await? {
            Some(identity) => identity,
            None => IdentityRepository::create(&mut *connection, CreateIdentityInput {
                login, display_name: Some(format!("Sensor: {}", sensor.r#ref)), password_hash: None,
                attributes: json!({"type":"sensor","sensor_ref":sensor.r#ref,"pack_ref":sensor.pack_ref}),
            }).await?,
        };
        if identity.frozen {
            return Err(Error::PermissionDenied(
                "Sensor input owner is frozen".into(),
            ));
        }
        let attributes = std::collections::HashMap::from([
            ("type".to_string(), json!("sensor")),
            ("sensor_ref".to_string(), json!(sensor.r#ref)),
            ("pack_ref".to_string(), json!(sensor.pack_ref)),
        ]);
        Ok(crate::delegation::DelegationAuthority::new(
            identity.id,
            attributes,
            binding.executable.input_grants.clone(),
        ))
    }

    async fn prepare_rule_inputs(
        connection: &mut PgConnection,
        binding: &SensorGenerationBinding,
        rule: &crate::models::Rule,
        encryption_key: Option<&str>,
    ) -> Result<crate::secret_values::RenderedJson> {
        let parameters = binding
            .triggers
            .get(&rule.trigger_ref)
            .filter(|trigger| trigger.event.trigger_id == rule.trigger)
            .ok_or_else(|| Error::validation("Sensor rule trigger was not admitted"))?
            .parameters
            .as_ref();
        let contract = parameters
            .map(CompiledContract::new)
            .transpose()
            .map_err(|_| Error::validation("Invalid bound sensor rule contract"))?;
        let values = if let Some(contract) = &contract {
            contract
                .validate_rule_secret_templates(&rule.trigger_params)
                .map_err(|error| {
                    Error::validation(format!(
                        "Sensor rule secret parameters require whole Key templates: {error}"
                    ))
                })?;
            contract
                .apply_defaults(&rule.trigger_params)
                .map_err(|error| {
                    Error::validation(format!("Invalid sensor rule defaults: {error}"))
                })?
        } else {
            validate_bounded_flat(&rule.trigger_params, &rule.trigger_ref)?;
            rule.trigger_params.clone()
        };
        let mut rendered =
            Self::render_inputs(connection, binding, &values, Some(rule), encryption_key).await?;
        if let Some(contract) = &contract {
            rendered.value = contract.apply_defaults(&rendered.value).map_err(|error| {
                Error::validation(format!("Invalid sensor rule defaults: {error}"))
            })?;
            contract.validate(&rendered.value).map_err(|error| {
                Error::validation(format!("Invalid sensor rule configuration: {error}"))
            })?;
            crate::secret_values::validate_contract_secret_destination_paths(
                &rendered.value,
                contract,
                &rendered.secret_paths,
            )?;
        } else {
            validate_bounded_flat(&rendered.value, &rule.trigger_ref)?;
            if !rendered.secret_paths.is_empty() {
                return Err(Error::validation(
                    "Secret sensor rule inputs require a declared secret destination",
                ));
            }
        }
        Ok(rendered)
    }
    pub async fn attach_current_log_deliveries(
        connection: &mut PgConnection,
        version: i64,
        generation: i64,
    ) -> Result<()> {
        sqlx::query("INSERT INTO sensor_log_rule_delivery_source (artifact_version, delivery) \
            SELECT $1, d.id FROM sensor_rule_config_delivery d \
            JOIN sensor_log_generation_binding b ON b.artifact_version = $1 AND b.generation_binding = d.generation_binding \
            WHERE d.generation_binding = $2 AND EXISTS (SELECT 1 FROM execution_secret_value s WHERE s.entity_type = 'sensor_rule_delivery_config' AND s.entity_id = d.id) \
            ON CONFLICT DO NOTHING")
            .bind(version).bind(generation).execute(connection).await?;
        Ok(())
    }

    pub async fn delivery_origin_paths<'e, E>(
        executor: E,
        generation: i64,
    ) -> Result<Vec<(i64, String)>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        sqlx::query_as("SELECT d.id, s.json_path FROM sensor_rule_config_delivery d JOIN execution_secret_value s \
            ON s.entity_type = 'sensor_rule_delivery_config' AND s.entity_id = d.id WHERE d.generation_binding = $1 ORDER BY d.id, s.json_path LIMIT 1025")
            .bind(generation).fetch_all(executor).await.map_err(Into::into)
    }

    pub async fn log_delivery_origins<'e, E>(executor: E, version: i64) -> Result<Vec<i64>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        sqlx::query_scalar("SELECT delivery FROM sensor_log_rule_delivery_source WHERE artifact_version = $1 ORDER BY delivery")
            .bind(version).fetch_all(executor).await.map_err(Into::into)
    }

    pub async fn find_delivery_owner<'e, E>(
        executor: E,
        id: i64,
    ) -> Result<Option<(i64, i64, String, String)>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        sqlx::query_as("SELECT generation_binding, rule, rule_ref, pack_ref FROM sensor_rule_config_delivery WHERE id = $1")
            .bind(id).fetch_optional(executor).await.map_err(Into::into)
    }
    pub async fn capture_rule_delivery(
        connection: &mut PgConnection,
        generation_id: i64,
        binding: &SensorGenerationBinding,
        rule: &crate::models::Rule,
        encryption_key: Option<&str>,
    ) -> Result<Value> {
        super::schema_type::SchemaTypeRepository::lock_selection(&mut *connection).await?;
        binding.validate()?;
        let stored_digest: Option<String> = sqlx::query_scalar(
            "SELECT binding_digest FROM sensor_generation_schema_binding WHERE id = $1 FOR UPDATE",
        )
        .bind(generation_id)
        .fetch_optional(&mut *connection)
        .await?;
        if stored_digest.as_deref() != Some(binding.binding_digest()?.as_str()) {
            return Err(Error::validation(
                "Sensor rule delivery generation mismatch",
            ));
        }
        let trigger = binding
            .triggers
            .get(&rule.trigger_ref)
            .filter(|trigger| trigger.event.trigger_id == rule.trigger)
            .ok_or_else(|| {
                Error::validation("Rule trigger was not admitted for this sensor generation")
            })?;
        let contract = trigger
            .parameters
            .as_ref()
            .map(CompiledContract::new)
            .transpose()
            .map_err(|_| Error::validation("Invalid admitted sensor rule contract"))?;
        let rendered = Self::prepare_rule_inputs(connection, binding, rule, encryption_key).await?;
        let value = rendered.value;
        if !value.is_object() {
            return Err(Error::validation(
                "Sensor rule configuration must be a flat object",
            ));
        }
        let (public, secrets) = if let Some(contract) = &contract {
            merge_contract_secret_redactions(
                value.clone(),
                &rendered.secret_path_sources,
                contract,
            )?
        } else {
            (value.clone(), Vec::new())
        };
        let material = json!({"rule":rule.id,"ref":rule.r#ref,"pack_ref":rule.pack_ref,
            "trigger_ref":rule.trigger_ref,"enabled":rule.enabled,"config":value,
            "templates":rule.trigger_params,"origins":secrets.iter().map(|secret| json!({
                "path":secret.json_path,"source_kind":secret.source_kind,"source_ref":secret.source_ref,
            })).collect::<Vec<_>>()});
        let fingerprint = if let Some(key) = encryption_key {
            use hmac::{Hmac, KeyInit, Mac};
            use sha2::Sha256;
            let bytes = serde_jcs::to_vec(&material)
                .map_err(|_| Error::validation("Invalid sensor rule delivery"))?;
            let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())
                .map_err(|_| Error::validation("Invalid sensor delivery fingerprint key"))?;
            mac.update(b"attune.sensor-rule-delivery/1\0");
            mac.update(&bytes);
            mac.finalize()
                .into_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        } else {
            if !secrets.is_empty() {
                return Err(Error::validation(
                    "Secret sensor rule delivery requires encryption",
                ));
            }
            digest(&material)
                .map_err(|_| Error::validation("Invalid sensor delivery fingerprint"))?
        };
        let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM sensor_rule_config_delivery WHERE generation_binding = $1 AND rule = $2 AND fingerprint = $3")
            .bind(generation_id).bind(rule.id).bind(&fingerprint).fetch_optional(&mut *connection).await?;
        if existing.is_some() {
            return Ok(value);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sensor_rule_config_delivery WHERE generation_binding = $1",
        )
        .bind(generation_id)
        .fetch_one(&mut *connection)
        .await?;
        if count >= 1024 {
            return Err(Error::validation(
                "Sensor rule delivery limit reached; restart the sensor to admit a new generation",
            ));
        }
        let prepared = if secrets.is_empty() {
            Vec::new()
        } else {
            prepare_secret_values(
                secrets,
                encryption_key.ok_or_else(|| {
                    Error::validation("Secret sensor rule delivery requires encryption")
                })?,
            )?
        };
        let id: Option<i64> = sqlx::query_scalar("INSERT INTO sensor_rule_config_delivery \
            (generation_binding, rule, rule_ref, pack_ref, trigger_ref, fingerprint, config) \
            VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (generation_binding, rule, fingerprint) DO NOTHING RETURNING id")
            .bind(generation_id).bind(rule.id).bind(&rule.r#ref).bind(&rule.pack_ref).bind(&rule.trigger_ref)
            .bind(fingerprint).bind(public).fetch_optional(&mut *connection).await?;
        if let Some(id) = id {
            ExecutionSecretValueRepository::upsert_many_with_conn(
                connection,
                ENTITY_SENSOR_RULE_DELIVERY_CONFIG,
                id,
                &prepared,
            )
            .await?;
        }
        Ok(value)
    }
    pub async fn attach_log_version(
        connection: &mut PgConnection,
        artifact_version: i64,
        generation_binding: i64,
    ) -> Result<()> {
        let (hash, sensor_ref): (String, String) = sqlx::query_as("SELECT binding_digest, binding->'executable'->'sensor'->>'ref' FROM sensor_generation_schema_binding WHERE id = $1 FOR SHARE")
            .bind(generation_binding).fetch_optional(&mut *connection).await?
            .ok_or_else(|| Error::validation("Sensor log generation is unavailable"))?;
        let owner: Option<i64> = sqlx::query_scalar("SELECT a.id FROM artifact_version v JOIN artifact a ON a.id = v.artifact \
            WHERE v.id = $1 AND a.scope = 'sensor' AND a.owner = $2 AND a.classification = 'runtime_log' \
              AND a.ref IN (format('sensor.%s.stdout', $2), format('sensor.%s.stderr', $2)) FOR SHARE OF v,a")
            .bind(artifact_version).bind(&sensor_ref).fetch_optional(&mut *connection).await?;
        if owner.is_none() {
            return Err(Error::validation(
                "Sensor log version does not match its generation owner",
            ));
        }
        sqlx::query("INSERT INTO sensor_log_generation_binding (artifact_version, generation_binding, generation_digest) VALUES ($1,$2,$3)")
            .bind(artifact_version).bind(generation_binding).bind(hash).execute(connection).await?;
        Ok(())
    }

    pub async fn find_by_binding_id<'e, E>(
        executor: E,
        id: i64,
    ) -> Result<Option<(SensorGenerationBinding, Value)>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let row: Option<(Value, String, Value)> = sqlx::query_as("SELECT binding, binding_digest, config FROM sensor_generation_schema_binding WHERE id = $1")
            .bind(id).fetch_optional(executor).await?;
        let Some((body, hash, config)) = row else {
            return Ok(None);
        };
        let binding = Self::decode_disclosure_binding(body, &hash)?;
        Ok(Some((binding, config)))
    }

    pub async fn find_log_generation<'e, E>(
        executor: E,
        artifact_version: i64,
    ) -> Result<Option<(i64, String)>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        sqlx::query_as("SELECT generation_binding, generation_digest FROM sensor_log_generation_binding WHERE artifact_version = $1")
            .bind(artifact_version).fetch_optional(executor).await.map_err(Into::into)
    }

    pub async fn find_initial_rules<'e, E>(executor: E, id: i64) -> Result<Option<Value>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        sqlx::query_scalar(
            "SELECT initial_rules FROM sensor_generation_schema_binding WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(executor)
        .await
        .map_err(Into::into)
    }
    pub async fn admit(
        connection: &mut PgConnection,
        workload: &OwnedSensorWorkload,
        encryption_key: Option<&str>,
    ) -> Result<()> {
        let mut executable: SensorExecutableSnapshot =
            serde_json::from_value(workload.executable_snapshot.clone().ok_or_else(|| {
                Error::validation("Sensor admission requires an executable snapshot")
            })?)
            .map_err(|_| Error::validation("Invalid sensor executable snapshot"))?;
        if executable.triggers.len() > 256 {
            return Err(Error::validation(
                "Sensor generation exceeds its trigger limit",
            ));
        }
        if executable.sensor.id != workload.sensor_id
            || Some(executable.release.id) != workload.pack_release
            || Some(&executable.release.digest) != workload.pack_release_digest.as_ref()
        {
            return Err(Error::validation(
                "Sensor admission snapshot does not match its workload",
            ));
        }
        let source = |reference: String| SourceIdentity::Released {
            component_ref: reference,
            release_id: executable.release.id,
            release_digest: executable.release.digest.clone(),
        };
        let config = SchemaContractRepository::materialize(
            connection,
            source(executable.sensor.r#ref.clone()),
            SchemaContext::FlatDocument,
            executable
                .sensor
                .param_schema
                .as_ref()
                .unwrap_or(&json!({})),
        )
        .await?;
        let mut triggers = std::collections::BTreeMap::new();
        let mut selected_triggers = Vec::new();
        let mut contract_bytes = serde_json::to_vec(&config)
            .map_err(|_| Error::validation("Invalid sensor config binding"))?
            .len();
        for trigger in &executable.triggers {
            let mut selected_trigger = trigger.clone();
            let (parameters, event) = if trigger.pack_ref == executable.sensor.pack_ref {
                let mut contracts = Vec::new();
                for schema in [
                    &trigger.param_schema,
                    &trigger.out_schema,
                    &trigger.event_config_schema,
                ] {
                    contracts.push(if let Some(schema) = schema {
                        Some(
                            SchemaContractRepository::materialize(
                                connection,
                                source(trigger.r#ref.clone()),
                                SchemaContext::FlatDocument,
                                schema,
                            )
                            .await?,
                        )
                    } else {
                        None
                    });
                }
                let mut contracts = contracts.into_iter();
                let parameters = contracts.next().flatten();
                let event = EventSchemaBinding {
                    trigger_id: Some(trigger.id),
                    trigger_ref: trigger.r#ref.clone(),
                    payload: contracts.next().flatten(),
                    config: contracts.next().flatten(),
                    webhook: false,
                };
                (parameters, event)
            } else {
                selected_trigger = TriggerRepository::find_by_id_for_share(connection, trigger.id)
                    .await?
                    .filter(|current| {
                        current.r#ref == trigger.r#ref && current.pack_ref == trigger.pack_ref
                    })
                    .ok_or_else(|| {
                        Error::validation("External sensor trigger source is unavailable")
                    })?;
                let declaring =
                    PackReleaseRepository::find_by_id(&mut *connection, executable.release.id)
                        .await?
                        .ok_or_else(|| {
                            Error::validation("Sensor declaring release is unavailable")
                        })?;
                let selected = PackReleaseRepository::find_active_by_pack_ref(
                    &mut *connection,
                    trigger.pack_ref.as_deref().ok_or_else(|| {
                        Error::validation("External sensor trigger requires a pack")
                    })?,
                )
                .await?
                .ok_or_else(|| Error::validation("External trigger release is unavailable"))?;
                check_dependency(&declaring, &selected)?;
                // External trigger projections can be edited outside release
                // activation. Capture that body as an immutable assignment;
                // the selected active pack release still governs its type view.
                let trigger_source = SourceIdentity::Assigned {
                    component_ref: trigger.r#ref.clone(),
                    assignment_id: uuid::Uuid::new_v4(),
                };
                let mut contracts = Vec::new();
                for schema in [
                    &selected_trigger.param_schema,
                    &selected_trigger.out_schema,
                    &selected_trigger.event_config_schema,
                ] {
                    contracts.push(if let Some(schema) = schema {
                        Some(
                            SchemaContractRepository::compile_selected(
                                connection,
                                trigger_source.clone(),
                                SchemaContext::FlatDocument,
                                schema,
                                Some(selected.clone()),
                            )
                            .await?,
                        )
                    } else {
                        None
                    });
                }
                let mut contracts = contracts.into_iter();
                let parameters = contracts.next().flatten();
                let event = EventSchemaBinding {
                    trigger_id: Some(trigger.id),
                    trigger_ref: trigger.r#ref.clone(),
                    payload: contracts.next().flatten(),
                    config: contracts.next().flatten(),
                    webhook: false,
                };
                (parameters, event)
            };
            let binding = SensorTriggerBinding { parameters, event };
            contract_bytes = contract_bytes
                .checked_add(
                    serde_json::to_vec(&binding)
                        .map_err(|_| Error::validation("Invalid sensor trigger binding"))?
                        .len(),
                )
                .ok_or_else(|| {
                    Error::validation("Sensor generation exceeds its binding size limit")
                })?;
            if contract_bytes > 32 * 1024 * 1024 {
                return Err(Error::validation(
                    "Sensor generation exceeds its binding size limit",
                ));
            }
            triggers.insert(trigger.r#ref.clone(), binding);
            selected_trigger.webhook_key = None;
            selected_trigger.webhook_config = None;
            selected_triggers.push(selected_trigger);
        }
        executable.triggers = selected_triggers;
        let current = SensorRepository::find_by_id_for_share(connection, workload.sensor_id)
            .await?
            .ok_or_else(|| Error::validation("Sensor configuration owner is unavailable"))?;
        let projection = super::component_configuration::ComponentConfigurationRepository::current(
            &mut *connection,
            "sensor",
            current.id,
        )
        .await?;
        let values = if let Some(revision) = &projection {
            super::component_configuration::ComponentConfigurationRepository::restore(
                connection,
                revision,
                encryption_key,
            )
            .await?
        } else {
            current
                .config
                .filter(|value| !value.is_null())
                .unwrap_or_else(|| json!({}))
        };
        // Values are stored separately from executable/schema metadata. Do not
        // copy plaintext credentials into the immutable contract bundle.
        executable.sensor.config = None;
        // Operational retention overrides apply when a new process generation
        // is admitted. Existing generations keep their captured settings.
        executable.sensor.log_retention_policy = current.log_retention_policy;
        executable.sensor.log_retention_limit = current.log_retention_limit;
        executable.sensor.artifact_retention_policy = current.artifact_retention_policy;
        executable.sensor.artifact_retention_limit = current.artifact_retention_limit;
        let binding = SensorGenerationBinding {
            executable,
            config,
            triggers,
        };
        let input_contract = CompiledContract::new(&binding.config)
            .map_err(|_| Error::validation("Invalid sensor input contract"))?;
        let values = input_contract.apply_defaults(&values).map_err(|error| {
            Error::validation(format!("Invalid sensor config defaults: {error}"))
        })?;
        let rendered =
            Self::render_inputs(connection, &binding, &values, None, encryption_key).await?;
        let values = binding.prepare_config(&rendered.value)?;
        crate::secret_values::validate_contract_secret_destination_paths(
            &values,
            &input_contract,
            &rendered.secret_paths,
        )?;
        let config_bytes = serde_json::to_vec(&values)
            .map_err(|_| Error::validation("Invalid sensor configuration"))?
            .len();
        let (initial_rules, rule_secrets) =
            Self::prepare_initial_rules(connection, &binding, config_bytes + 64, encryption_key)
                .await?;
        let prepared_rules = if rule_secrets.is_empty() {
            Vec::new()
        } else {
            prepare_secret_values(
                rule_secrets,
                encryption_key.ok_or_else(|| {
                    Error::validation("Secret sensor rule configuration requires encryption")
                })?,
            )?
        };
        let contract = CompiledContract::new(&binding.config)
            .map_err(|_| Error::validation("Invalid sensor config binding"))?;
        let (public, secrets) =
            merge_contract_secret_redactions(values, &rendered.secret_path_sources, &contract)?;
        let prepared = if secrets.is_empty() {
            Vec::new()
        } else {
            prepare_secret_values(
                secrets,
                encryption_key.ok_or_else(|| {
                    Error::validation("Secret sensor configuration requires encryption")
                })?,
            )?
        };
        let body = serde_json::to_value(&binding)
            .map_err(|_| Error::validation("Invalid sensor generation binding"))?;
        if serde_json::to_vec(&body)
            .map_err(|_| Error::validation("Invalid sensor generation binding"))?
            .len()
            > 32 * 1024 * 1024
        {
            return Err(Error::validation(
                "Sensor generation exceeds its binding size limit",
            ));
        }
        let hash =
            digest(&body).map_err(|_| Error::validation("Invalid sensor generation digest"))?;
        let id: i64 = sqlx::query_scalar("INSERT INTO sensor_generation_schema_binding (workload, generation, binding, binding_digest, config, initial_rules) VALUES ($1,$2,$3,$4,$5,$6) RETURNING id")
            .bind(workload.workload_id).bind(workload.generation).bind(body).bind(hash).bind(public).bind(initial_rules).fetch_one(&mut *connection).await?;
        ExecutionSecretValueRepository::upsert_many_with_conn(
            connection,
            ENTITY_SENSOR_GENERATION_CONFIG,
            id,
            &prepared,
        )
        .await?;
        ExecutionSecretValueRepository::upsert_many_with_conn(
            connection,
            ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            id,
            &prepared_rules,
        )
        .await?;
        Ok(())
    }

    async fn prepare_initial_rules(
        connection: &mut PgConnection,
        binding: &SensorGenerationBinding,
        initial_bytes: usize,
        encryption_key: Option<&str>,
    ) -> Result<(Value, Vec<crate::secret_values::SecretValueInput>)> {
        let mut rules = serde_json::Map::new();
        let mut secrets = Vec::new();
        let mut total_bytes = initial_bytes;
        for trigger in &binding.executable.triggers {
            let current = TriggerRepository::find_by_id(&mut *connection, trigger.id)
                .await?
                .ok_or_else(|| Error::validation("Sensor bootstrap trigger is unavailable"))?;
            if current.r#ref != trigger.r#ref
                || current.sensor != Some(binding.executable.sensor.id)
            {
                return Err(Error::validation(
                    "Sensor bootstrap trigger ownership mismatch",
                ));
            }
            if !current.enabled {
                continue;
            }
            let contract = binding.triggers[&trigger.r#ref]
                .parameters
                .as_ref()
                .map(CompiledContract::new)
                .transpose()
                .map_err(|_| Error::validation("Invalid admitted trigger parameter contract"))?;
            for rule in RuleRepository::find_by_trigger(&mut *connection, trigger.id)
                .await?
                .into_iter()
                .filter(|rule| rule.enabled)
            {
                if rules.len() >= 10_000 {
                    return Err(Error::validation("Sensor bootstrap exceeds its rule limit"));
                }
                let rendered =
                    Self::prepare_rule_inputs(connection, binding, &rule, encryption_key).await?;
                let config = rendered.value;
                if !config.is_object() {
                    return Err(Error::validation(
                        "Sensor rule configuration must be a flat object",
                    ));
                }
                let config_bytes = serde_json::to_vec(&config)
                    .map_err(|_| Error::validation("Invalid sensor rule configuration"))?
                    .len();
                let (public, mut protected) = if let Some(contract) = &contract {
                    merge_contract_secret_redactions(
                        config,
                        &rendered.secret_path_sources,
                        contract,
                    )?
                } else {
                    (config, Vec::new())
                };
                for secret in &mut protected {
                    secret.json_path = format!("/{}/config{}", rule.id, secret.json_path);
                }
                secrets.extend(protected);
                let entry = json!({"id": rule.id, "ref": rule.r#ref, "pack_ref": rule.pack_ref, "trigger_ref": trigger.r#ref, "config": public});
                total_bytes = total_bytes
                    .checked_add(
                        config_bytes
                            + serde_json::to_vec(&entry)
                                .map_err(|_| Error::validation("Invalid sensor rule bootstrap"))?
                                .len(),
                    )
                    .ok_or_else(|| {
                        Error::validation("Sensor bootstrap exceeds its delivery limit")
                    })?;
                if total_bytes > 32 * 1024 * 1024 {
                    return Err(Error::validation(
                        "Sensor bootstrap exceeds its delivery limit",
                    ));
                }
                rules.insert(rule.id.to_string(), entry);
            }
        }
        Ok((Value::Object(rules), secrets))
    }

    pub async fn restore_initial_rules(
        connection: &mut PgConnection,
        id: i64,
        binding: &SensorGenerationBinding,
        encryption_key: Option<&str>,
    ) -> Result<Vec<Value>> {
        let public: Value = sqlx::query_scalar(
            "SELECT initial_rules FROM sensor_generation_schema_binding WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&mut *connection)
        .await?;
        let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *connection,
            ENTITY_SENSOR_GENERATION_RULE_CONFIG,
            id,
        )
        .await?;
        let restored = if secrets.is_empty() {
            public
        } else {
            restore_secret_values(
                public,
                &secrets,
                encryption_key.ok_or_else(|| {
                    Error::validation("Sensor rule secret delivery requires encryption")
                })?,
            )?
        };
        let rules = restored
            .as_object()
            .ok_or_else(|| Error::validation("Invalid bound sensor rule bootstrap"))?;
        for (identity, rule) in rules {
            let rule_id = rule
                .get("id")
                .and_then(Value::as_i64)
                .filter(|id| *id > 0)
                .ok_or_else(|| Error::validation("Invalid admitted sensor rule identity"))?;
            if identity != &rule_id.to_string() {
                return Err(Error::validation("Sensor rule bootstrap identity mismatch"));
            }
            let reference = rule
                .get("trigger_ref")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::validation("Invalid sensor rule trigger"))?;
            let parameters = binding
                .triggers
                .get(reference)
                .ok_or_else(|| Error::validation("Sensor rule trigger was not admitted"))?
                .parameters
                .as_ref();
            let config = rule
                .get("config")
                .filter(|value| value.is_object())
                .ok_or_else(|| Error::validation("Invalid sensor rule configuration"))?;
            if let Some(parameters) = parameters {
                CompiledContract::new(parameters)
                    .map_err(|_| Error::validation("Invalid admitted sensor rule schema"))?
                    .validate(config)
                    .map_err(|error| {
                        Error::validation(format!(
                            "Invalid bound sensor rule configuration: {error}"
                        ))
                    })?;
            }
        }
        Ok(rules.values().cloned().collect())
    }

    pub async fn find<'e, E>(
        executor: E,
        workload: i64,
        generation: i64,
    ) -> Result<Option<(i64, SensorGenerationBinding, Value)>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let row: Option<(i64, Value, String, Value)> = sqlx::query_as("SELECT id, binding, binding_digest, config FROM sensor_generation_schema_binding WHERE workload = $1 AND generation = $2")
            .bind(workload).bind(generation).fetch_optional(executor).await?;
        let Some((id, body, hash, config)) = row else {
            return Ok(None);
        };
        if digest(&body).map_err(|_| Error::validation("Invalid sensor generation digest"))? != hash
        {
            return Err(Error::validation("Sensor generation digest mismatch"));
        }
        let binding: SensorGenerationBinding = serde_json::from_value(body)
            .map_err(|_| Error::validation("Invalid sensor generation binding"))?;
        binding.validate()?;
        Ok(Some((id, binding, config)))
    }

    pub async fn restore_config(
        connection: &mut PgConnection,
        id: i64,
        binding: &SensorGenerationBinding,
        public: Value,
        encryption_key: Option<&str>,
    ) -> Result<Value> {
        let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *connection,
            ENTITY_SENSOR_GENERATION_CONFIG,
            id,
        )
        .await?;
        let value = if secrets.is_empty() {
            public
        } else {
            restore_secret_values(
                public,
                &secrets,
                encryption_key.ok_or_else(|| {
                    Error::validation("Sensor secret delivery requires encryption")
                })?,
            )?
        };
        CompiledContract::new(&binding.config)
            .map_err(|_| Error::validation("Invalid sensor generation config"))?
            .validate(&value)
            .map_err(|error| Error::validation(format!("Invalid bound sensor config: {error}")))?;
        Ok(value)
    }
}
