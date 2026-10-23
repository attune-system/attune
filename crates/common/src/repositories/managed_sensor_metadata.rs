//! Immutable sensor metadata and purpose-specific authority over pinned code bytes.
use super::{
    component_configuration::ComponentConfigurationRepository, identity::PermissionSetRepository,
    runtime_authority::RuntimeAuthorityRepository, runtime_target_binding,
    schema_type::SchemaTypeRepository,
};
use crate::{
    models::{Id, OwnedSensorWorkload, Sensor, SensorExecutableSnapshot},
    rbac::{Action, Grant, Resource},
    runtime_authority::{
        AdmittedGrant, AdmittedPermissionSet, GrantTargetBindings, PackAuthorityOwner,
        PermissionScope, PermissionSetDefinition,
    },
    value_schema::{digest, Contract},
    Error, Result,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Executor, PgConnection, Postgres};
use std::collections::BTreeMap;

fn validate_entrypoint(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 1024
        || path.chars().any(char::is_control)
        || path.contains('\\')
        || std::path::Path::new(path)
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(Error::validation(
            "Sensor entrypoint must be a relative path inside its pinned release",
        ));
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SensorPurposeAuthority {
    /// New admission retains the purpose and each declared set independently.
    Declared {
        sets: BTreeMap<String, Vec<AdmittedGrant>>,
    },
    /// Historical snapshots recorded aggregate grants, not their per-set origin.
    /// Preserve that evidence explicitly rather than inventing a per-ref mapping.
    RetainedAggregate {
        selected_refs: Vec<String>,
        grants: Vec<AdmittedGrant>,
    },
}

impl SensorPurposeAuthority {
    fn grants(&self) -> Vec<&AdmittedGrant> {
        match self {
            Self::Declared { sets } => sets.values().flatten().collect(),
            Self::RetainedAggregate { grants, .. } => grants.iter().collect(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorMetadata {
    pub owner: PackAuthorityOwner,
    pub executable: SensorExecutableSnapshot,
    pub configuration_revision: Option<Id>,
    pub config_contract: Contract,
    pub trigger_contracts: BTreeMap<String, crate::sensor_schema::SensorTriggerBinding>,
    pub purposes: BTreeMap<String, SensorPurposeAuthority>,
    pub concrete_targets: BTreeMap<String, GrantTargetBindings>,
}

pub struct ManagedSensorMetadataRepository;

/// Historical evidence only. This does not validate live sponsor/approval
/// activity or grant disclosure independently of the consuming authorization.
pub struct PreparedSensorGenerationBinding {
    pub binding: crate::sensor_schema::SensorGenerationBinding,
    pub metadata: Option<SensorMetadata>,
}

#[derive(sqlx::FromRow)]
struct RetainedBindingRow {
    id: Id,
    binding: Value,
    binding_digest: String,
    revision: Option<Id>,
    metadata: Option<Value>,
    metadata_digest: Option<String>,
    sensor: Option<Id>,
    pack: Option<Id>,
    release: Option<Id>,
    configuration: Option<Id>,
    configuration_valid: bool,
    release_valid: bool,
}

fn permits(purpose: &str, resource: Resource, action: Action) -> bool {
    match purpose {
        "input" => {
            matches!(resource, Resource::Keys | Resource::Packs)
                && matches!(action, Action::Read | Action::Decrypt)
        }
        "signing" => resource == Resource::Keys && action == Action::Use,
        "cache" => resource == Resource::Caches,
        _ => false,
    }
}

fn refs(config: &Value, purpose: &str) -> Result<Vec<String>> {
    let field = format!("{purpose}_permission_set_refs");
    let Some(value) = config.get(&field) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| Error::validation("Sensor capability refs must be arrays"))?;
    if values.len() > 64 {
        return Err(Error::validation(
            "Sensor capability selection exceeds its limit",
        ));
    }
    let mut refs = Vec::new();
    for value in values {
        let reference = value
            .as_str()
            .ok_or_else(|| Error::validation("Sensor capability refs must be literal strings"))?;
        if reference == "standard" {
            if purpose != "cache" {
                return Err(Error::validation(
                    "Standard is not input or signing authority",
                ));
            }
        } else {
            crate::schema::RefValidator::validate_component_ref(reference)
                .map_err(|_| Error::validation("Invalid sensor capability ref"))?;
        }
        refs.push(reference.to_owned());
    }
    refs.sort();
    refs.dedup();
    Ok(refs)
}

fn require_approval(
    approval: &super::runtime_authority::ExternalApprovalRecord,
    owner: &PackAuthorityOwner,
    requested: &Grant,
    now: chrono::DateTime<Utc>,
) -> Result<()> {
    if !approval.is_active(now) || !approval.approval()?.covers(owner, requested) {
        return Err(Error::PermissionDenied(
            "Sensor approval is expired, revoked or outside the recorded subject scope".into(),
        ));
    }
    Ok(())
}

impl ManagedSensorMetadataRepository {
    /// One statement for at most MAX_SENSOR_ORIGIN_BATCH raw IDs. Duplicate IDs
    /// coalesce; missing generations remain absent. No current-name or policy
    /// lookup and no configuration values or ciphertext are selected.
    pub async fn prepare_exact_bindings<'e, E>(
        executor: E,
        ids: &[Id],
    ) -> Result<BTreeMap<Id, PreparedSensorGenerationBinding>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        use super::sensor_rule_origin::MAX_SENSOR_ORIGIN_BATCH;
        if ids.len() > MAX_SENSOR_ORIGIN_BATCH || ids.iter().any(|id| *id <= 0) {
            return Err(Error::validation("Invalid retained sensor binding batch"));
        }
        let ids = ids
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let rows:Vec<RetainedBindingRow>=sqlx::query_as("SELECT b.id,b.binding,b.binding_digest,g.revision,m.body AS metadata,m.body_digest AS metadata_digest,m.sensor,m.pack,m.release,m.configuration_revision AS configuration, \
            (m.configuration_revision IS NULL OR EXISTS(SELECT 1 FROM component_configuration_revision c WHERE c.id=m.configuration_revision AND c.owner_kind='sensor' AND c.owner_id=m.sensor AND c.owner_ref=m.body->'executable'->'sensor'->>'ref' AND c.pack_ref=m.body->'owner'->>'pack_ref')) AS configuration_valid, \
            EXISTS(SELECT 1 FROM pack_release r JOIN pack_release_executable e ON e.release=r.id WHERE r.id=m.release AND r.pack=m.pack AND r.digest=m.body->'executable'->'release'->>'digest' AND e.component_kind='sensor' AND e.component_id=m.sensor AND e.component_ref=m.body->'executable'->'sensor'->>'ref') AS release_valid \
            FROM sensor_generation_schema_binding b LEFT JOIN managed_sensor_generation g ON g.workload=b.workload AND g.generation=b.generation LEFT JOIN managed_sensor_metadata_revision m ON m.id=g.revision WHERE b.id=ANY($1)")
            .bind(ids).fetch_all(executor).await?;
        rows.into_iter()
            .map(|row| {
                let binding =
                    super::sensor_schema::SensorSchemaRepository::decode_disclosure_binding(
                        row.binding,
                        &row.binding_digest,
                    )?;
                let metadata = match row.revision {
                    Some(revision) => {
                        let invalid = || Error::validation("Invalid retained sensor metadata pins");
                        if !row.configuration_valid || !row.release_valid {
                            return Err(invalid());
                        }
                        let metadata = Self::decode_retained(
                            revision,
                            row.sensor.ok_or_else(invalid)?,
                            row.pack.ok_or_else(invalid)?,
                            row.release.ok_or_else(invalid)?,
                            row.configuration,
                            row.metadata.ok_or_else(invalid)?,
                            &row.metadata_digest.ok_or_else(invalid)?,
                        )?;
                        if serde_json::to_value(&binding.executable)?
                            != serde_json::to_value(&metadata.executable)?
                            || binding.config.binding_digest
                                != metadata.config_contract.binding_digest
                            || serde_json::to_value(&binding.triggers)?
                                != serde_json::to_value(&metadata.trigger_contracts)?
                            || binding.metadata_revision.is_some_and(|id| id != revision)
                            || (matches!(
                                &metadata.config_contract.source,
                                crate::value_schema::SourceIdentity::Assigned { .. }
                            ) && binding.metadata_revision != Some(revision))
                        {
                            return Err(invalid());
                        }
                        Some(metadata)
                    }
                    None => {
                        if binding.metadata_revision.is_some() {
                            return Err(Error::validation(
                                "Retained sensor metadata pin is missing",
                            ));
                        }
                        None
                    }
                };
                Ok((
                    row.id,
                    PreparedSensorGenerationBinding { binding, metadata },
                ))
            })
            .collect()
    }

    fn decode_retained(
        id: Id,
        sensor: Id,
        pack: Id,
        release: Id,
        configuration: Option<Id>,
        body: Value,
        hash: &str,
    ) -> Result<SensorMetadata> {
        if digest(&body).ok().as_deref() != Some(hash) {
            return Err(Error::validation("Invalid sensor metadata"));
        }
        let metadata: SensorMetadata = serde_json::from_value(body)
            .map_err(|_| Error::validation("Invalid sensor metadata"))?;
        if metadata.owner.pack_id != pack
            || metadata.executable.sensor.id != sensor
            || metadata.executable.release.id != release
            || metadata.configuration_revision != configuration
            || metadata.executable.sensor.pack != Some(pack)
            || metadata.executable.sensor.config.is_some()
        {
            return Err(Error::validation("Sensor metadata ownership mismatch"));
        }
        Self::validate_purposes(&metadata)?;
        crate::sensor_schema::SensorGenerationBinding {
            metadata_revision: Some(id),
            executable: metadata.executable.clone(),
            config: metadata.config_contract.clone(),
            triggers: metadata.trigger_contracts.clone(),
        }
        .validate()?;
        Ok(metadata)
    }
    fn validate_purposes(metadata: &SensorMetadata) -> Result<()> {
        if metadata.purposes.len() != 3 {
            return Err(Error::validation("Sensor purpose admission is incomplete"));
        }
        for (purpose, refs, projected) in [
            (
                "input",
                &metadata.executable.input_permission_set_refs,
                &metadata.executable.input_grants,
            ),
            (
                "signing",
                &metadata.executable.signing_permission_set_refs,
                &metadata.executable.signing_grants,
            ),
            (
                "cache",
                &metadata.executable.cache_permission_set_refs,
                &metadata.executable.cache_grants,
            ),
        ] {
            let authority = metadata
                .purposes
                .get(purpose)
                .ok_or_else(|| Error::validation("Sensor purpose admission is incomplete"))?;
            let selected = match authority {
                SensorPurposeAuthority::Declared { sets } => {
                    sets.keys().cloned().collect::<Vec<_>>()
                }
                SensorPurposeAuthority::RetainedAggregate { selected_refs, .. } => {
                    selected_refs.clone()
                }
            };
            if &selected != refs
                || authority.grants().iter().any(|grant| {
                    grant
                        .grant()
                        .actions
                        .iter()
                        .any(|action| !permits(purpose, grant.grant().resource, *action))
                })
            {
                return Err(Error::validation(
                    "Sensor purpose selection does not match its immutable executable",
                ));
            }
            let expected = match authority {
                SensorPurposeAuthority::Declared { .. } => Self::projected(metadata, purpose)?,
                SensorPurposeAuthority::RetainedAggregate { grants, .. } => {
                    grants.iter().map(|grant| grant.grant().clone()).collect()
                }
            };
            if &expected != projected {
                return Err(Error::validation(
                    "Sensor purpose projection does not match its admission",
                ));
            }
        }
        Ok(())
    }
    pub async fn current_revision<'e, E>(executor: E, sensor: Id) -> Result<Option<Id>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        Ok(sqlx::query_scalar(
            "SELECT revision FROM managed_sensor_metadata_current WHERE sensor=$1",
        )
        .bind(sensor)
        .fetch_optional(executor)
        .await?)
    }
    pub async fn is_managed(connection: &mut PgConnection, sensor: Id) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT management_origin='pack' AND managed_release IS NOT NULL FROM sensor WHERE id=$1 AND retired_at IS NULL")
            .bind(sensor).fetch_optional(connection).await?.unwrap_or(false))
    }

    pub async fn readmit_permission_set(
        connection: &mut PgConnection,
        reference: &str,
        editor: Id,
        key: Option<&str>,
    ) -> Result<()> {
        Self::lock_mutation(connection).await?;
        let sensors: Vec<Id> = sqlx::query_scalar("SELECT c.sensor FROM managed_sensor_metadata_current c JOIN managed_sensor_metadata_revision r ON r.id=c.revision WHERE r.body->'executable'->'input_permission_set_refs' ? $1 OR r.body->'executable'->'cache_permission_set_refs' ? $1 OR r.body->'executable'->'signing_permission_set_refs' ? $1 ORDER BY c.sensor")
            .bind(reference).fetch_all(&mut *connection).await?;
        for id in sensors {
            use super::FindById;
            let sensor = super::trigger::SensorRepository::find_by_id(&mut *connection, id)
                .await?
                .ok_or_else(|| Error::validation("Affected sensor is unavailable"))?;
            Self::readmit(connection, &sensor, editor, key).await?;
        }
        Ok(())
    }
    /// Exclusive graph admission must precede sensor, pack, sponsor and policy locks.
    pub async fn lock_mutation(connection: &mut PgConnection) -> Result<()> {
        SchemaTypeRepository::lock_reclamation(connection).await
    }

    async fn purpose(
        connection: &mut PgConnection,
        owner: &PackAuthorityOwner,
        references: &[String],
        purpose: &str,
        editor: Id,
    ) -> Result<SensorPurposeAuthority> {
        let named = references
            .iter()
            .filter(|reference| reference.as_str() != "standard")
            .cloned()
            .collect::<Vec<_>>();
        let sets = PermissionSetRepository::find_by_refs_for_share(connection, &named).await?;
        if sets.len() != named.len() {
            return Err(Error::validation(
                "Sensor selects an unavailable capability set",
            ));
        }
        let mut own = Vec::new();
        let mut foreign = Vec::new();
        let mut external = Vec::new();
        for set in sets {
            if set.pack == Some(owner.pack_id) {
                let mut definition: PermissionSetDefinition =
                    PermissionSetRepository::authored_definition(connection, &set.r#ref)
                        .await?
                        .ok_or_else(|| {
                            Error::validation("Sensor capability authoring is unavailable")
                        })?;
                for grant in &mut definition.grants {
                    grant
                        .actions
                        .retain(|action| permits(purpose, grant.resource, *action));
                }
                definition.grants.retain(|grant| !grant.actions.is_empty());
                external.extend(
                    definition
                        .grants
                        .iter()
                        .filter(|grant| {
                            grant.scope.unwrap_or(definition.scope) == PermissionScope::External
                        })
                        .map(|grant| Grant {
                            resource: grant.resource,
                            actions: grant.actions.clone(),
                            constraints: grant.constraints.clone(),
                        }),
                );
                own.push(definition);
            } else {
                let mut grants: Vec<Grant> = serde_json::from_value(set.grants)
                    .map_err(|_| Error::validation("Invalid sensor capability definition"))?;
                for grant in &mut grants {
                    grant
                        .actions
                        .retain(|action| permits(purpose, grant.resource, *action));
                }
                grants.retain(|grant| !grant.actions.is_empty());
                external.extend(grants.iter().cloned());
                foreign.push((set.r#ref, grants));
            }
        }
        let approvals = RuntimeAuthorityRepository::admit_external_grants(
            connection,
            owner.pack_id,
            editor,
            &external,
        )
        .await?;
        let mut admitted = BTreeMap::new();
        if references.iter().any(|reference| reference == "standard") {
            admitted.insert("standard".into(), Vec::new());
        }
        for definition in own {
            let set = definition.compile(owner, &approvals)?;
            admitted.insert(set.reference, set.grants);
        }
        for (reference, body) in foreign {
            let set = AdmittedPermissionSet::external(&reference, owner, &body, &approvals)?;
            admitted.insert(set.reference, set.grants);
        }
        Ok(SensorPurposeAuthority::Declared { sets: admitted })
    }

    async fn pin(
        connection: &mut PgConnection,
        metadata: &mut SensorMetadata,
        previous: &BTreeMap<String, GrantTargetBindings>,
    ) -> Result<()> {
        let grants = metadata
            .purposes
            .values()
            .flat_map(SensorPurposeAuthority::grants)
            .cloned()
            .collect::<Vec<_>>();
        let approval_ids = grants
            .iter()
            .filter_map(AdmittedGrant::approval_id)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if !approval_ids.is_empty() {
            let locked: Vec<Id> = sqlx::query_scalar(
                "SELECT id FROM runtime_external_approval WHERE id=ANY($1) ORDER BY id FOR SHARE",
            )
            .bind(&approval_ids)
            .fetch_all(&mut *connection)
            .await?;
            if locked.len() != approval_ids.len() {
                return Err(Error::PermissionDenied(
                    "Sensor approval is unavailable".into(),
                ));
            }
        }
        metadata.concrete_targets =
            runtime_target_binding::capture_grants(connection, &metadata.owner, &grants, previous)
                .await?;
        // Target resolution can wait on concurrent mutations. Check expiry only
        // after acquiring those locks, not against a pre-wait timestamp.
        let now = sqlx::query_scalar::<_, chrono::DateTime<Utc>>("SELECT clock_timestamp()")
            .fetch_one(&mut *connection)
            .await?;
        for grant in &grants {
            if let AdmittedGrant::External {
                approval_id,
                grant: requested,
            } = grant
            {
                let approval = RuntimeAuthorityRepository::find_approval(connection, *approval_id)
                    .await?
                    .ok_or_else(|| {
                        Error::PermissionDenied("Sensor approval is unavailable".into())
                    })?;
                require_approval(&approval, &metadata.owner, requested, now)?;
                if !runtime_target_binding::approval_covers_binding(
                    connection,
                    &approval,
                    &metadata.owner,
                    requested,
                    &metadata.concrete_targets[&runtime_target_binding::grant_key(grant)?],
                )
                .await?
                {
                    return Err(Error::PermissionDenied(
                        "Sensor approval does not cover pinned target identities".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn projected(metadata: &SensorMetadata, purpose: &str) -> Result<Vec<Grant>> {
        metadata
            .purposes
            .get(purpose)
            .into_iter()
            .flat_map(SensorPurposeAuthority::grants)
            .map(|admitted| {
                let mut grant = admitted.grant().clone();
                if let AdmittedGrant::PackOwned { pack_id, .. } = admitted {
                    grant
                        .constraints
                        .get_or_insert_with(Default::default)
                        .managing_pack_id = Some(*pack_id);
                }
                let binding = metadata
                    .concrete_targets
                    .get(&runtime_target_binding::grant_key(admitted)?)
                    .ok_or_else(|| {
                        Error::validation("Sensor concrete target binding is unavailable")
                    })?;
                if let Some(targets) = binding.targets.as_ref() {
                    grant.constraints.get_or_insert_with(Default::default).ids =
                        Some(targets.iter().map(|target| target.id).collect());
                }
                Ok(grant)
            })
            .collect()
    }

    async fn store(connection: &mut PgConnection, metadata: &SensorMetadata) -> Result<Id> {
        let executable = &metadata.executable;
        let body = serde_json::to_value(metadata)?;
        if serde_json::to_vec(&body)?.len() > 32 * 1024 * 1024 {
            return Err(Error::validation(
                "Sensor metadata exceeds its binding size limit",
            ));
        }
        let hash =
            digest(&body).map_err(|_| Error::validation("Invalid sensor metadata digest"))?;
        let id: Option<Id> = sqlx::query_scalar("INSERT INTO managed_sensor_metadata_revision (sensor,pack,release,configuration_revision,body,body_digest) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (sensor,release,body_digest) DO NOTHING RETURNING id")
            .bind(executable.sensor.id).bind(metadata.owner.pack_id).bind(executable.release.id)
            .bind(metadata.configuration_revision).bind(&body).bind(&hash).fetch_optional(&mut *connection).await?;
        match id { Some(id) => Ok(id), None => Ok(sqlx::query_scalar("SELECT id FROM managed_sensor_metadata_revision WHERE sensor = $1 AND release = $2 AND body_digest = $3")
            .bind(executable.sensor.id).bind(executable.release.id).bind(hash).fetch_one(connection).await?) }
    }

    pub async fn find(connection: &mut PgConnection, id: Id) -> Result<SensorMetadata> {
        Self::load(connection, id, true).await
    }

    async fn load(
        connection: &mut PgConnection,
        id: Id,
        require_live: bool,
    ) -> Result<SensorMetadata> {
        let (sensor, pack, release, configuration, body, hash, revoked): (Id,Id,Id,Option<Id>,Value,String,Option<chrono::DateTime<Utc>>) = sqlx::query_as(
            "SELECT sensor,pack,release,configuration_revision,body,body_digest,revoked_at FROM managed_sensor_metadata_revision WHERE id = $1 FOR SHARE")
            .bind(id).fetch_optional(&mut *connection).await?.ok_or_else(|| Error::validation("Sensor metadata revision is unavailable"))?;
        if (require_live && revoked.is_some())
            || digest(&body).map_err(|_| Error::validation("Invalid sensor metadata"))? != hash
        {
            return Err(Error::PermissionDenied(
                "Sensor metadata was revoked or is invalid".into(),
            ));
        }
        let metadata =
            Self::decode_retained(id, sensor, pack, release, configuration, body, &hash)?;
        if let Some(configuration) = configuration {
            let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM component_configuration_revision WHERE id=$1 AND owner_kind='sensor' AND owner_id=$2 AND owner_ref=$3 AND pack_ref=$4)")
                .bind(configuration).bind(sensor).bind(&metadata.executable.sensor.r#ref).bind(&metadata.owner.pack_ref)
                .fetch_one(&mut *connection).await?;
            if !owned {
                return Err(Error::validation(
                    "Sensor configuration revision ownership mismatch",
                ));
            }
        }
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pack_release p JOIN pack_release_executable e ON e.release=p.id WHERE p.id=$1 AND p.pack=$2 AND p.digest=$3 AND e.component_kind='sensor' AND e.component_id=$4 AND e.component_ref=$5)")
            .bind(release).bind(pack).bind(&metadata.executable.release.digest).bind(sensor).bind(&metadata.executable.sensor.r#ref)
            .fetch_one(connection).await?;
        if !valid {
            return Err(Error::validation(
                "Sensor metadata does not match immutable code ownership",
            ));
        }
        Ok(metadata)
    }

    /// Runs after authorized projection and configuration writes, in their transaction.
    pub async fn readmit(
        connection: &mut PgConnection,
        sensor: &Sensor,
        editor: Id,
        encryption_key: Option<&str>,
    ) -> Result<Id> {
        Self::lock_mutation(connection).await?;
        let revoked: Option<bool> = sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM managed_sensor_runtime_state WHERE sensor=$1 FOR SHARE")
            .bind(sensor.id).fetch_optional(&mut *connection).await?;
        if revoked == Some(true) {
            return Err(Error::PermissionDenied(
                "Sensor runtime was explicitly revoked".into(),
            ));
        }
        let release: Option<Id> = sqlx::query_scalar("SELECT managed_release FROM sensor WHERE id = $1 AND management_origin = 'pack' AND retired_at IS NULL FOR SHARE")
            .bind(sensor.id).fetch_optional(&mut *connection).await?.flatten();
        let release = release.ok_or_else(|| {
            Error::validation("Sensor readmission requires managed immutable code")
        })?;
        let pack_id = sensor
            .pack
            .ok_or_else(|| Error::validation("Sensor readmission requires its pack"))?;
        let owner = RuntimeAuthorityRepository::ensure_owner(connection, pack_id).await?;
        let row: Option<Value> = sqlx::query_scalar("SELECT jsonb_build_object('release',jsonb_build_object('id',p.id,'digest',p.digest,'content_path',p.content_path)) || e.snapshot FROM pack_release p JOIN pack_release_executable e ON e.release = p.id WHERE p.id = $1 AND p.pack = $2 AND e.component_kind = 'sensor' AND e.component_id = $3 AND e.component_ref = $4 FOR SHARE OF p,e")
            .bind(release).bind(pack_id).bind(sensor.id).bind(&sensor.r#ref).fetch_optional(&mut *connection).await?;
        let mut executable: SensorExecutableSnapshot =
            serde_json::from_value(row.ok_or_else(|| {
                Error::validation("Sensor is absent from its pinned code release")
            })?)
            .map_err(|_| Error::validation("Invalid released sensor executable"))?;
        // Entrypoint edits select another path within the same immutable archive;
        // they never overwrite release bytes or follow a mutable filesystem path.
        validate_entrypoint(&sensor.entrypoint)?;
        if sensor.runtime != executable.runtime.id || sensor.runtime_ref != executable.runtime.r#ref
        {
            return Err(Error::validation(
                "Sensor runtime changes require a released runtime binding",
            ));
        }
        let configuration =
            ComponentConfigurationRepository::current(&mut *connection, "sensor", sensor.id)
                .await?
                .ok_or_else(|| {
                    Error::validation("Sensor readmission requires bound configuration")
                })?;
        let config = &configuration.config;
        executable.sensor = sensor.clone();
        executable.sensor.config = None;
        let mut purposes = BTreeMap::new();
        for purpose in ["cache", "signing", "input"] {
            let references = refs(config, purpose)?;
            match purpose {
                "cache" => executable.cache_permission_set_refs = references.clone(),
                "signing" => executable.signing_permission_set_refs = references.clone(),
                _ => executable.input_permission_set_refs = references.clone(),
            }
            purposes.insert(
                purpose.into(),
                Self::purpose(connection, &owner, &references, purpose, editor).await?,
            );
        }
        let config_contract = configuration.contract()?;
        let trigger_contracts =
            super::sensor_schema::SensorSchemaRepository::capture_trigger_contracts(
                connection,
                &mut executable,
                &config_contract,
            )
            .await?;
        let previous: Option<Id> = sqlx::query_scalar(
            "SELECT revision FROM managed_sensor_metadata_current WHERE sensor = $1 FOR SHARE",
        )
        .bind(sensor.id)
        .fetch_optional(&mut *connection)
        .await?;
        let previous = match previous {
            Some(id) => {
                let old = Self::load(connection, id, false).await?;
                if old.executable.release.id == release {
                    old.concrete_targets
                } else {
                    BTreeMap::new()
                }
            }
            None => BTreeMap::new(),
        };
        let mut metadata = SensorMetadata {
            owner,
            executable,
            configuration_revision: Some(configuration.id),
            config_contract,
            trigger_contracts,
            purposes,
            concrete_targets: BTreeMap::new(),
        };
        Self::pin(connection, &mut metadata, &previous).await?;
        metadata.executable.cache_grants = Self::projected(&metadata, "cache")?;
        metadata.executable.signing_grants = Self::projected(&metadata, "signing")?;
        metadata.executable.input_grants = Self::projected(&metadata, "input")?;
        Self::validate(connection, &metadata).await?;
        let values =
            ComponentConfigurationRepository::restore(connection, &configuration, encryption_key)
                .await?;
        // Existing generations are adopted from their immutable bindings, never
        // rewritten with the edited projection or its new selections.
        Self::adopt_live(connection, sensor.id, editor).await?;
        let id = Self::store(connection, &metadata).await?;
        Self::find(connection, id).await?;
        super::sensor_schema::SensorSchemaRepository::validate_readmission_inputs(
            connection,
            id,
            &metadata.executable,
            &metadata.config_contract,
            &values,
            encryption_key,
        )
        .await?;
        sqlx::query(
            "INSERT INTO managed_sensor_runtime_state(sensor) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(sensor.id)
        .execute(&mut *connection)
        .await?;
        sqlx::query("INSERT INTO managed_sensor_metadata_current(sensor,revision) VALUES ($1,$2) ON CONFLICT(sensor) DO UPDATE SET revision = EXCLUDED.revision")
            .bind(sensor.id).bind(id).execute(connection).await?;
        Ok(id)
    }

    async fn adopt_live(connection: &mut PgConnection, sensor: Id, editor: Id) -> Result<()> {
        let rows: Vec<(Id,i64)> = sqlx::query_as("SELECT b.workload,b.generation FROM sensor_generation_schema_binding b JOIN sensor_workload w ON w.id=b.workload JOIN sensor_workload_assignment a ON a.workload=b.workload AND a.generation=b.generation WHERE w.sensor=$1 AND a.worker IS NOT NULL AND a.lease_expires_at>clock_timestamp() AND NOT EXISTS(SELECT 1 FROM managed_sensor_generation g WHERE g.workload=b.workload AND g.generation=b.generation) ORDER BY b.workload FOR SHARE OF b")
            .bind(sensor).fetch_all(&mut *connection).await?;
        for (workload, generation) in rows {
            let (_, binding, _) = super::sensor_schema::SensorSchemaRepository::find(
                &mut *connection,
                workload,
                generation,
            )
            .await?
            .ok_or_else(|| Error::validation("Retained sensor generation is unavailable"))?;
            if binding.executable.sensor.id != sensor {
                return Err(Error::validation(
                    "Retained sensor generation owner mismatch",
                ));
            }
            let owner = RuntimeAuthorityRepository::ensure_owner(
                connection,
                binding
                    .executable
                    .sensor
                    .pack
                    .ok_or_else(|| Error::validation("Missing retained sensor owner"))?,
            )
            .await?;
            let mut purposes = BTreeMap::new();
            for (purpose, grants) in [
                ("input", &binding.executable.input_grants),
                ("signing", &binding.executable.signing_grants),
                ("cache", &binding.executable.cache_grants),
            ] {
                let external = grants
                    .iter()
                    .filter(|grant| {
                        grant
                            .constraints
                            .as_ref()
                            .and_then(|scope| scope.managing_pack_id)
                            != Some(owner.pack_id)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let approvals = RuntimeAuthorityRepository::admit_external_grants(
                    connection,
                    owner.pack_id,
                    editor,
                    &external,
                )
                .await?;
                let mut admitted = Vec::new();
                for grant in grants {
                    if grant
                        .constraints
                        .as_ref()
                        .and_then(|scope| scope.managing_pack_id)
                        == Some(owner.pack_id)
                    {
                        admitted.push(AdmittedGrant::PackOwned {
                            pack_id: owner.pack_id,
                            grant: grant.clone(),
                        });
                    } else {
                        admitted.extend(
                            AdmittedPermissionSet::external(
                                "retained.sensor",
                                &owner,
                                std::slice::from_ref(grant),
                                &approvals,
                            )?
                            .grants,
                        );
                    }
                }
                let selected_refs = match purpose {
                    "input" => binding.executable.input_permission_set_refs.clone(),
                    "signing" => binding.executable.signing_permission_set_refs.clone(),
                    _ => binding.executable.cache_permission_set_refs.clone(),
                };
                purposes.insert(
                    purpose.into(),
                    SensorPurposeAuthority::RetainedAggregate {
                        selected_refs,
                        grants: admitted,
                    },
                );
            }
            let mut metadata = SensorMetadata {
                owner,
                executable: binding.executable,
                configuration_revision: None,
                config_contract: binding.config,
                trigger_contracts: binding.triggers,
                purposes,
                concrete_targets: BTreeMap::new(),
            };
            Self::pin(connection, &mut metadata, &BTreeMap::new()).await?;
            Self::validate(connection, &metadata).await?;
            let revision = Self::store(connection, &metadata).await?;
            sqlx::query("INSERT INTO managed_sensor_generation(workload,generation,revision) VALUES ($1,$2,$3)")
                .bind(workload).bind(generation).bind(revision).execute(&mut *connection).await?;
        }
        Ok(())
    }

    pub async fn select_generation(
        connection: &mut PgConnection,
        workload: &mut OwnedSensorWorkload,
    ) -> Result<()> {
        let revision: Option<Id> = sqlx::query_scalar("SELECT c.revision FROM managed_sensor_metadata_current c JOIN managed_sensor_metadata_revision r ON r.id=c.revision JOIN sensor s ON s.id=c.sensor AND s.managed_release=r.release WHERE c.sensor=$1")
            .bind(workload.sensor_id).fetch_optional(&mut *connection).await?;
        if let Some(id) = revision {
            let metadata = Self::find(connection, id).await?;
            Self::validate(connection, &metadata).await?;
            sqlx::query("INSERT INTO managed_sensor_generation(workload,generation,revision) VALUES ($1,$2,$3)")
                .bind(workload.workload_id).bind(workload.generation).bind(id).execute(&mut *connection).await?;
            Self::apply(workload, &metadata)?;
        }
        Self::validate_admission(connection, workload.workload_id, workload.generation).await?;
        Ok(())
    }

    fn apply(workload: &mut OwnedSensorWorkload, metadata: &SensorMetadata) -> Result<()> {
        if workload.sensor_id != metadata.executable.sensor.id {
            return Err(Error::validation("Sensor generation owner mismatch"));
        }
        workload.pack_release = Some(metadata.executable.release.id);
        workload.pack_release_digest = Some(metadata.executable.release.digest.clone());
        workload.executable_snapshot = Some(serde_json::to_value(&metadata.executable)?);
        Ok(())
    }

    pub async fn for_generation(
        connection: &mut PgConnection,
        workload: Id,
        generation: i64,
    ) -> Result<Option<SensorMetadata>> {
        let revision: Option<Id> = sqlx::query_scalar("SELECT revision FROM managed_sensor_generation WHERE workload=$1 AND generation=$2 FOR SHARE")
            .bind(workload).bind(generation).fetch_optional(&mut *connection).await?;
        match revision {
            Some(id) => Ok(Some(Self::find(connection, id).await?)),
            None => Ok(None),
        }
    }

    pub async fn revision_for_generation(
        connection: &mut PgConnection,
        workload: Id,
        generation: i64,
    ) -> Result<Option<Id>> {
        Ok(sqlx::query_scalar("SELECT revision FROM managed_sensor_generation WHERE workload=$1 AND generation=$2 FOR SHARE")
            .bind(workload).bind(generation).fetch_optional(connection).await?)
    }

    pub async fn restore_generation(
        connection: &mut PgConnection,
        workload: &mut OwnedSensorWorkload,
    ) -> Result<()> {
        Self::validate_generation(connection, workload.workload_id, workload.generation).await?;
        if let Some(metadata) =
            Self::for_generation(connection, workload.workload_id, workload.generation).await?
        {
            Self::apply(workload, &metadata)?;
        }
        Ok(())
    }

    pub async fn validate_generation(
        connection: &mut PgConnection,
        workload: Id,
        generation: i64,
    ) -> Result<()> {
        Self::validate_admission(connection, workload, generation).await?;
        if let Some(metadata) = Self::for_generation(connection, workload, generation).await? {
            let (_, binding, _) = super::sensor_schema::SensorSchemaRepository::find(
                &mut *connection,
                workload,
                generation,
            )
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied(
                    "Sensor generation has no complete immutable input binding".into(),
                )
            })?;
            if binding.executable.sensor.id != metadata.executable.sensor.id
                || serde_json::to_value(&binding.executable)?
                    != serde_json::to_value(&metadata.executable)?
                || binding.executable.sensor.pack != Some(metadata.owner.pack_id)
                || binding.executable.release.id != metadata.executable.release.id
                || binding.executable.release.digest != metadata.executable.release.digest
                || binding.config.binding_digest != metadata.config_contract.binding_digest
                || serde_json::to_value(&binding.triggers)?
                    != serde_json::to_value(&metadata.trigger_contracts)?
                || binding.executable.input_grants != metadata.executable.input_grants
                || binding.executable.signing_grants != metadata.executable.signing_grants
                || binding.executable.cache_grants != metadata.executable.cache_grants
                || (matches!(
                    metadata.config_contract.source,
                    crate::value_schema::SourceIdentity::Assigned { .. }
                ) && binding.metadata_revision
                    != Self::revision_for_generation(connection, workload, generation).await?)
            {
                return Err(Error::PermissionDenied(
                    "Sensor generation does not match its metadata admission".into(),
                ));
            }
        }
        Ok(())
    }

    pub(super) async fn validate_admission(
        connection: &mut PgConnection,
        workload: Id,
        generation: i64,
    ) -> Result<()> {
        let revoked: Option<bool> = sqlx::query_scalar("SELECT r.revoked_at IS NOT NULL FROM managed_sensor_runtime_state r JOIN sensor_workload w ON w.sensor=r.sensor WHERE w.id=$1 FOR SHARE OF r")
            .bind(workload).fetch_optional(&mut *connection).await?;
        if revoked == Some(true) {
            return Err(Error::PermissionDenied(
                "Sensor runtime was explicitly revoked".into(),
            ));
        }
        if let Some(metadata) = Self::for_generation(connection, workload, generation).await? {
            Self::validate(connection, &metadata).await?;
        } else if revoked.is_some() {
            return Err(Error::PermissionDenied(
                "Sensor generation has no recorded metadata admission".into(),
            ));
        }
        Ok(())
    }

    async fn validate(connection: &mut PgConnection, metadata: &SensorMetadata) -> Result<()> {
        Self::validate_purposes(metadata)?;
        let owner: Option<(Id,String,Option<Id>,bool)> = sqlx::query_as("SELECT s.id,s.ref,s.pack,i.frozen FROM sensor s JOIN pack pk ON pk.id=s.pack AND pk.ref=s.pack_ref JOIN pack_runtime_identity p ON p.pack=s.pack JOIN identity i ON i.id=p.identity WHERE s.id=$1 AND s.pack=$2 AND p.identity=$3 AND pk.ref=$4 AND s.retired_at IS NULL FOR SHARE OF s,pk,p,i")
            .bind(metadata.executable.sensor.id).bind(metadata.owner.pack_id).bind(metadata.owner.sponsor_identity_id).bind(&metadata.owner.pack_ref).fetch_optional(&mut *connection).await?;
        if !owner.is_some_and(|(id, reference, pack, frozen)| {
            id == metadata.executable.sensor.id
                && reference == metadata.executable.sensor.r#ref
                && pack == Some(metadata.owner.pack_id)
                && !frozen
        }) {
            return Err(Error::PermissionDenied(
                "Sensor runtime sponsor or owner is unavailable".into(),
            ));
        }
        let mut checked = metadata.clone();
        // Re-resolve concrete names only to detect replacement. Never install the
        // new identities into the retained policy or grant an old token access.
        Self::pin(connection, &mut checked, &BTreeMap::new()).await?;
        if checked.concrete_targets != metadata.concrete_targets {
            return Err(Error::PermissionDenied(
                "Sensor concrete target identity changed".into(),
            ));
        }
        Ok(())
    }

    pub async fn revoke(
        connection: &mut PgConnection,
        sensor: Id,
        revision: Option<Id>,
    ) -> Result<bool> {
        Self::lock_mutation(connection).await?;
        let result = if let Some(revision) = revision {
            sqlx::query("UPDATE managed_sensor_metadata_revision SET revoked_at=COALESCE(revoked_at,clock_timestamp()) WHERE id=$1 AND sensor=$2")
                .bind(revision).bind(sensor).execute(connection).await?
        } else {
            sqlx::query("INSERT INTO managed_sensor_runtime_state(sensor,revoked_at) VALUES ($1,clock_timestamp()) ON CONFLICT(sensor) DO UPDATE SET revoked_at=COALESCE(managed_sensor_runtime_state.revoked_at,EXCLUDED.revoked_at)")
                .bind(sensor).execute(connection).await?
        };
        Ok(result.rows_affected() > 0)
    }

    pub async fn cleanup_with_conn(connection: &mut PgConnection, limit: u32) -> Result<()> {
        let limit = i64::from(limit.min(1000));
        sqlx::query("DELETE FROM managed_sensor_generation g USING (SELECT g.workload,g.generation FROM managed_sensor_generation g WHERE NOT EXISTS(SELECT 1 FROM sensor_generation_schema_binding b WHERE b.workload=g.workload AND b.generation=g.generation) AND NOT EXISTS(SELECT 1 FROM sensor_workload_assignment a WHERE a.workload=g.workload AND a.generation=g.generation AND a.worker IS NOT NULL AND a.lease_expires_at>clock_timestamp()) ORDER BY g.workload,g.generation LIMIT $1) old WHERE g.workload=old.workload AND g.generation=old.generation")
            .bind(limit).execute(&mut *connection).await?;
        sqlx::query("DELETE FROM managed_sensor_metadata_revision r USING (SELECT r.id FROM managed_sensor_metadata_revision r WHERE r.created<NOW()-INTERVAL '1 hour' AND NOT EXISTS(SELECT 1 FROM managed_sensor_metadata_current c WHERE c.revision=r.id) AND NOT EXISTS(SELECT 1 FROM managed_sensor_generation g WHERE g.revision=r.id) ORDER BY r.created,r.id LIMIT $1) old WHERE r.id=old.id")
            .bind(limit).execute(connection).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sensor_purposes_do_not_promote_input_or_signing_into_api_authority() {
        assert!(permits("input", Resource::Keys, Action::Read));
        assert!(!permits("input", Resource::Keys, Action::Use));
        assert!(permits("signing", Resource::Keys, Action::Use));
        assert!(!permits("signing", Resource::Keys, Action::Read));
        assert!(!permits("cache", Resource::Keys, Action::Read));
        assert!(refs(&json!({"cache_permission_set_refs":["standard"]}), "cache").is_ok());
        assert!(refs(&json!({"input_permission_set_refs":["standard"]}), "input").is_err());
        assert!(refs(
            &json!({"signing_permission_set_refs":"private-fixture-value"}),
            "signing"
        )
        .is_err());
    }

    #[test]
    fn code_edits_cannot_escape_the_existing_release_root() {
        for path in [
            "../escape.py",
            "/absolute.py",
            "sensors/../../escape.py",
            "sensors\\escape.py",
            "sensors/\n.py",
        ] {
            assert!(validate_entrypoint(path).is_err());
        }
        assert!(validate_entrypoint("sensors/next.py").is_ok());
    }

    #[test]
    fn fixed_approval_expiry_revocation_and_subject_are_checked_without_issuer_iam() {
        let owner = PackAuthorityOwner {
            pack_id: 7,
            pack_ref: "example".into(),
            sponsor_identity_id: 17,
        };
        let grant = Grant {
            resource: Resource::Keys,
            actions: vec![Action::Use],
            constraints: None,
        };
        let now = Utc::now();
        let mut approval = super::super::runtime_authority::ExternalApprovalRecord {
            id: 1,
            subject_pack: 7,
            issuer_identity: 999,
            issuer_login: "historical-issuer".into(),
            grants: sqlx::types::Json(vec![grant.clone()]),
            approval_digest: digest(&json!({"subject_pack":7,"grants":[grant]})).unwrap(),
            expires_at: Some(now + chrono::Duration::seconds(1)),
            revoked_at: None,
        };
        assert!(require_approval(&approval, &owner, &grant, now).is_ok());
        assert!(require_approval(
            &approval,
            &owner,
            &grant,
            now + chrono::Duration::seconds(1)
        )
        .is_err());
        approval.revoked_at = Some(now);
        assert!(require_approval(&approval, &owner, &grant, now).is_err());
        approval.revoked_at = None;
        let other = PackAuthorityOwner {
            pack_id: 8,
            ..owner
        };
        assert!(require_approval(&approval, &other, &grant, now).is_err());
    }
}
