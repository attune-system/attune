//! Pack Component Loader
//!
//! Reads permission set, runtime, action, dashboard, trigger, queue, policy, rule, sensor, and cache YAML definitions from a pack directory
//! and registers them in the database. This is the Rust-native equivalent of
//! the Python `load_core_pack.py` script used during init-packs.
//!
//! Components are loaded in dependency order:
//! 1. Permission sets (no dependencies)
//! 2. Runtimes (no dependencies)
//! 3. Triggers (no dependencies)
//! 4. Actions (depend on runtime; workflow actions also create workflow_definition records)
//! 5. Dashboards (can reference pack-owned metadata)
//! 6. Work queues (can reference actions)
//! 7. Policies (can reference actions)
//! 8. Rules (depend on triggers and actions)
//! 9. Sensors (depend on triggers and runtime)
//! 10. Cache namespaces (depend on packs, actions, or sensors)
//!
//! All loaders use **upsert** semantics: if an entity with the same ref already
//! exists it is updated in place (preserving its database ID); otherwise a new
//! row is created. After loading, entities that belong to the pack but whose
//! refs are no longer present in the YAML files are deleted.
//!
//! ## Workflow Actions
//!
//! An action YAML may include a `workflow_file` field pointing to a workflow
//! definition file relative to the `actions/` directory (e.g.,
//! `workflow_file: workflows/deploy.workflow.yaml`). When present the loader:
//!
//! 1. Reads and parses the referenced workflow YAML file.
//! 2. Creates or updates a `workflow_definition` record in the database.
//! 3. Creates the action record with `workflow_def` linked to the definition.
//!
//! This allows the action YAML to control action-level metadata (ref, label,
//! parameters, policies) independently of the workflow graph. Multiple actions
//! can reference the same workflow file with different configurations.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sqlx::{PgConnection, PgPool, Transaction};
use tracing::{debug, info, warn};

use crate::action_visibility::{
    collect_workflow_action_refs, ensure_action_reference_allowed, ensure_trigger_reference_allowed,
};
use crate::config::CacheAdmissionConfig;
use crate::dashboard_spec::validate_dashboard_spec;
use crate::error::{Error, Result};
use crate::models::{
    ActionReferenceVisibility, DashboardScopeType, DashboardVisibility, Id, RetentionPolicyType,
};
use crate::pack_cache_definition::{CacheDefinitionOwnerType, CacheDefinitionYaml};
use crate::policy_control::parse_policy_controls;
use crate::queue_definition::parse_work_queue_definition_yaml;
use crate::rbac::{validate_cache_grant_constraints, Grant, Resource};
use crate::repositories::action::{
    validate_action_reference_visibility_config, ActionRepository, CreatePolicyInput,
    PolicyRepository, UpdateActionInput, UpdatePolicyInput,
};
use crate::repositories::cache::{
    CacheNamespaceRepository, CacheOwnerScope, ManagedCacheNamespaceDefinition,
};
use crate::repositories::dashboard::{
    CreateDashboardInput, DashboardRepository, DashboardScopedRef, UpdateDashboardInput,
};
use crate::repositories::identity::{
    CreatePermissionSetInput, PermissionSetRepository, UpdatePermissionSetInput,
};
use crate::repositories::rule::{CreateRuleInput, RuleRepository, UpdateRuleInput};
use crate::repositories::runtime_version::{
    CreateRuntimeVersionInput, RuntimeVersionRepository, UpdateRuntimeVersionInput,
};
use crate::repositories::trigger::{
    validate_trigger_reference_visibility_config, CreateSensorInput, CreateTriggerInput,
    SensorRepository, TriggerRepository, UpdateSensorInput, UpdateTriggerInput,
};
use crate::repositories::workflow::{
    CreateWorkflowDefinitionInput, UpdateWorkflowDefinitionInput, WorkflowDefinitionRepository,
};
use crate::repositories::{
    runtime::{CreateRuntimeInput, RuntimeRepository, UpdateRuntimeInput},
    work_queue::{CreateWorkQueueInput, UpdateWorkQueueInput, WorkQueueRepository},
    Create, Delete, FindById, FindByRef, Patch, Update,
};
use crate::schema::RefValidator;
use crate::version_matching::extract_version_components;
use crate::workflow::parser::parse_workflow_yaml;

struct CleanupRefs<'a> {
    permission_sets: &'a [String],
    runtimes: &'a [String],
    triggers: &'a [String],
    actions: &'a [String],
    dashboards: &'a [String],
    queues: &'a [String],
    policies: &'a [String],
    rules: &'a [String],
    sensors: &'a [String],
    caches: &'a [String],
}

/// Result of loading pack components into the database.
#[derive(Debug, Default)]
pub struct PackLoadResult {
    /// Number of permission sets created
    pub permission_sets_loaded: usize,
    /// Number of permission sets updated
    pub permission_sets_updated: usize,
    /// Number of permission sets skipped
    pub permission_sets_skipped: usize,
    /// Number of runtimes created
    pub runtimes_loaded: usize,
    /// Number of runtimes updated (already existed)
    pub runtimes_updated: usize,
    /// Number of runtimes skipped due to errors
    pub runtimes_skipped: usize,
    /// Number of triggers created
    pub triggers_loaded: usize,
    /// Number of triggers updated
    pub triggers_updated: usize,
    /// Number of triggers skipped
    pub triggers_skipped: usize,
    /// Number of actions created
    pub actions_loaded: usize,
    /// Number of actions updated
    pub actions_updated: usize,
    /// Number of actions skipped
    pub actions_skipped: usize,
    /// Number of queues created
    pub queues_loaded: usize,
    /// Number of queues updated
    pub queues_updated: usize,
    /// Number of queues skipped
    pub queues_skipped: usize,
    /// Number of dashboards created
    pub dashboards_loaded: usize,
    /// Number of dashboards updated
    pub dashboards_updated: usize,
    /// Number of dashboards skipped
    pub dashboards_skipped: usize,
    /// Number of policies created
    pub policies_loaded: usize,
    /// Number of policies updated
    pub policies_updated: usize,
    /// Number of policies skipped
    pub policies_skipped: usize,
    /// Number of rules created
    pub rules_loaded: usize,
    /// Number of rules updated
    pub rules_updated: usize,
    /// Number of rules skipped
    pub rules_skipped: usize,
    /// Number of sensors created
    pub sensors_loaded: usize,
    /// Number of sensors updated
    pub sensors_updated: usize,
    /// Number of sensors skipped
    pub sensors_skipped: usize,
    /// Number of cache namespace definitions created
    pub caches_loaded: usize,
    /// Number of cache namespace policies updated
    pub caches_updated: usize,
    /// Number of unchanged or unresolved cache namespace definitions skipped
    pub caches_skipped: usize,
    /// Number of stale entities removed
    pub removed: usize,
    /// Warnings encountered during loading
    pub warnings: Vec<String>,
}

impl PackLoadResult {
    pub fn total_loaded(&self) -> usize {
        self.permission_sets_loaded
            + self.runtimes_loaded
            + self.triggers_loaded
            + self.actions_loaded
            + self.dashboards_loaded
            + self.queues_loaded
            + self.policies_loaded
            + self.rules_loaded
            + self.sensors_loaded
            + self.caches_loaded
    }

    pub fn total_skipped(&self) -> usize {
        self.permission_sets_skipped
            + self.runtimes_skipped
            + self.triggers_skipped
            + self.actions_skipped
            + self.dashboards_skipped
            + self.queues_skipped
            + self.policies_skipped
            + self.rules_skipped
            + self.sensors_skipped
            + self.caches_skipped
    }

    pub fn total_updated(&self) -> usize {
        self.permission_sets_updated
            + self.runtimes_updated
            + self.triggers_updated
            + self.actions_updated
            + self.dashboards_updated
            + self.queues_updated
            + self.policies_updated
            + self.rules_updated
            + self.sensors_updated
            + self.caches_updated
    }
}

/// Loads pack components (triggers, actions, sensors) from YAML files on disk
/// into the database.
pub struct PackComponentLoader<'a> {
    pool: &'a PgPool,
    pack_id: Id,
    pack_ref: String,
    cache_admission: CacheAdmissionConfig,
}

impl<'a> PackComponentLoader<'a> {
    pub fn new(
        pool: &'a PgPool,
        pack_id: Id,
        pack_ref: &str,
        cache_admission: &CacheAdmissionConfig,
    ) -> Self {
        Self {
            pool,
            pack_id,
            pack_ref: pack_ref.to_string(),
            cache_admission: cache_admission.clone(),
        }
    }

    /// Load all components from the pack directory.
    ///
    /// Uses upsert semantics: entities that already exist (by ref) are updated
    /// in place, preserving their database IDs. New entities are created.
    /// After loading, entities that belong to the pack but are no longer
    /// present in the YAML files are removed.
    pub async fn load_all(&self, pack_dir: &Path) -> Result<PackLoadResult> {
        let mut tx = self.pool.begin().await?;
        let result = self.load_all_in_transaction(&mut tx, pack_dir).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Load all components as part of a caller-owned transaction.
    pub async fn load_all_in_transaction(
        &self,
        tx: &mut Transaction<'_, sqlx::Postgres>,
        pack_dir: &Path,
    ) -> Result<PackLoadResult> {
        let mut loader = TransactionalPackComponentLoader {
            connection: tx,
            pack_id: self.pack_id,
            pack_ref: self.pack_ref.clone(),
            cache_admission: self.cache_admission.clone(),
        };
        loader.load_all(pack_dir).await
    }
}

struct TransactionalPackComponentLoader<'a> {
    connection: &'a mut PgConnection,
    pack_id: Id,
    pack_ref: String,
    cache_admission: CacheAdmissionConfig,
}

impl TransactionalPackComponentLoader<'_> {
    async fn load_all(&mut self, pack_dir: &Path) -> Result<PackLoadResult> {
        // Cache definitions load after every other component because their
        // owners must already exist. Validate their deterministic file/schema
        // constraints before mutating any component so a malformed cache file
        // cannot leave an otherwise valid pack reload partially applied.
        self.preflight_cache_definitions(pack_dir)?;
        self.preflight_component_ownership(pack_dir).await?;

        let mut result = PackLoadResult::default();

        info!(
            "Loading components for pack '{}' from {}",
            self.pack_ref,
            pack_dir.display()
        );

        // 1. Load permission sets first (no dependencies)
        let permission_set_refs = self.load_permission_sets(pack_dir, &mut result).await?;

        // 2. Load runtimes (no dependencies)
        let runtime_refs = self.load_runtimes(pack_dir, &mut result).await?;

        // 3. Load triggers (no dependencies)
        let (trigger_ids, trigger_refs) = self.load_triggers(pack_dir, &mut result).await?;

        // 4. Load actions (depend on runtime)
        let action_refs = self.load_actions(pack_dir, &mut result).await?;

        // 5. Load dashboards (pack-managed metadata)
        let dashboard_refs = self.load_dashboards(pack_dir, &mut result).await?;

        // 6. Load work queues (can reference actions)
        let queue_refs = self.load_queues(pack_dir, &mut result).await?;

        // 7. Load policies (can reference actions)
        let policy_refs = self.load_policies(pack_dir, &mut result).await?;

        // 8. Load rules (depend on triggers and actions)
        let rule_refs = self.load_rules(pack_dir, &trigger_ids, &mut result).await?;

        // 9. Load sensors (depend on triggers and runtime)
        let sensor_refs = self
            .load_sensors(pack_dir, &trigger_ids, &mut result)
            .await?;

        // 10. Load cache namespaces after action and sensor owners exist
        let cache_refs = self
            .load_caches(pack_dir, &action_refs, &sensor_refs, &mut result)
            .await?;

        // 11. Clean up entities that are no longer in the pack's YAML files
        self.cleanup_removed_entities(
            CleanupRefs {
                permission_sets: &permission_set_refs,
                runtimes: &runtime_refs,
                triggers: &trigger_refs,
                actions: &action_refs,
                dashboards: &dashboard_refs,
                queues: &queue_refs,
                policies: &policy_refs,
                rules: &rule_refs,
                sensors: &sensor_refs,
                caches: &cache_refs,
            },
            &mut result,
        )
        .await;

        info!(
            "Pack '{}' component loading complete: {} created, {} updated, {} skipped, {} removed, {} warnings",
            self.pack_ref,
            result.total_loaded(),
            result.total_updated(),
            result.total_skipped(),
            result.removed,
            result.warnings.len()
        );

        Ok(result)
    }

    async fn preflight_component_ownership(&mut self, pack_dir: &Path) -> Result<()> {
        for (directory, kind) in [
            ("permission_sets", "permission set"),
            ("runtimes", "runtime"),
            ("triggers", "trigger"),
            ("actions", "action"),
            ("sensors", "sensor"),
        ] {
            let path = pack_dir.join(directory);
            if !path.exists() {
                continue;
            }
            for (filename, content) in read_yaml_files(&path)? {
                let data: serde_yaml_ng::Value =
                    serde_yaml_ng::from_str(&content).map_err(|error| {
                        Error::validation(format!(
                            "Failed to parse {kind} YAML {filename}: {error}"
                        ))
                    })?;
                let component_ref = data
                    .get("ref")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        Error::validation(format!("{kind} YAML {filename} is missing ref"))
                    })?;
                if directory == "permission_sets" && component_ref == "standard" {
                    return Err(Error::validation(
                        "Cannot overwrite reserved permission set 'standard'",
                    ));
                }
                validate_pack_component_ref(&self.pack_ref, kind, component_ref)?;

                let owner = match directory {
                    "permission_sets" => {
                        PermissionSetRepository::find_by_ref(&mut *self.connection, component_ref)
                            .await?
                            .map(|item| item.pack)
                    }
                    "runtimes" => {
                        RuntimeRepository::find_by_ref(&mut *self.connection, component_ref)
                            .await?
                            .map(|item| item.pack)
                    }
                    "triggers" => {
                        TriggerRepository::find_by_ref(&mut *self.connection, component_ref)
                            .await?
                            .map(|item| item.pack)
                    }
                    "actions" => {
                        ActionRepository::find_by_ref(&mut *self.connection, component_ref)
                            .await?
                            .map(|item| Some(item.pack))
                    }
                    "sensors" => {
                        SensorRepository::find_by_ref(&mut *self.connection, component_ref)
                            .await?
                            .map(|item| item.pack)
                    }
                    _ => unreachable!(),
                };
                if let Some(owner) = owner {
                    ensure_existing_owner(kind, component_ref, owner, self.pack_id)?;
                }
            }
        }

        for (directory, kind) in [("policies", "policy"), ("rules", "rule")] {
            let path = pack_dir.join(directory);
            if !path.exists() {
                continue;
            }
            for (filename, content) in read_yaml_files(&path)? {
                let data: serde_yaml_ng::Value =
                    serde_yaml_ng::from_str(&content).map_err(|error| {
                        Error::validation(format!(
                            "Failed to parse {kind} YAML {filename}: {error}"
                        ))
                    })?;
                let raw_ref = data
                    .get("ref")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        Error::validation(format!("{kind} YAML {filename} is missing ref"))
                    })?;
                let component_ref = qualify_pack_ref(&self.pack_ref, raw_ref);
                validate_pack_component_ref(&self.pack_ref, kind, &component_ref)?;
                let owner = if directory == "policies" {
                    PolicyRepository::find_by_ref(&mut *self.connection, &component_ref)
                        .await?
                        .map(|item| item.pack)
                } else {
                    RuleRepository::find_by_ref(&mut *self.connection, &component_ref)
                        .await?
                        .map(|item| Some(item.pack))
                };
                if let Some(owner) = owner {
                    ensure_existing_owner(kind, &component_ref, owner, self.pack_id)?;
                }
            }
        }

        let queues_dir = pack_dir.join("queues");
        if queues_dir.exists() {
            for (filename, content) in read_yaml_files(&queues_dir)? {
                let definition = parse_work_queue_definition_yaml(&content).map_err(|error| {
                    Error::validation(format!(
                        "Failed to parse work queue YAML {filename}: {error}"
                    ))
                })?;
                validate_pack_component_ref(&self.pack_ref, "work queue", &definition.r#ref)?;
                if let Some(existing) =
                    WorkQueueRepository::find_by_ref(&mut *self.connection, &definition.r#ref)
                        .await?
                {
                    ensure_existing_owner(
                        "work queue",
                        &definition.r#ref,
                        existing.pack,
                        self.pack_id,
                    )?;
                }
            }
        }

        let actions_dir = pack_dir.join("actions");
        if actions_dir.exists() {
            for (filename, content) in read_yaml_files(&actions_dir)? {
                let action_data: serde_yaml_ng::Value =
                    serde_yaml_ng::from_str(&content).map_err(|error| {
                        Error::validation(format!(
                            "Failed to parse action YAML {filename}: {error}"
                        ))
                    })?;
                let Some(workflow_file) = action_data
                    .get("workflow_file")
                    .and_then(|value| value.as_str())
                else {
                    continue;
                };
                let action_ref = action_data
                    .get("ref")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        Error::validation(format!("Action YAML {filename} is missing ref"))
                    })?;
                let workflow_path = resolve_workflow_path(&actions_dir, workflow_file)?;
                let workflow_content =
                    std::fs::read_to_string(&workflow_path).map_err(|error| {
                        Error::io(format!(
                            "Failed to read workflow file {}: {error}",
                            workflow_path.display()
                        ))
                    })?;
                let workflow = parse_workflow_yaml(&workflow_content).map_err(|error| {
                    Error::validation(format!(
                        "Failed to parse workflow YAML {} (referenced by action '{}'): {error}",
                        workflow_path.display(),
                        action_ref
                    ))
                })?;
                let workflow_ref = if workflow.r#ref.is_empty() {
                    action_ref
                } else {
                    workflow.r#ref.as_str()
                };
                validate_pack_component_ref(&self.pack_ref, "workflow", workflow_ref)?;
                if let Some(existing) =
                    WorkflowDefinitionRepository::find_by_ref(&mut *self.connection, workflow_ref)
                        .await?
                {
                    ensure_existing_owner(
                        "workflow",
                        workflow_ref,
                        Some(existing.pack),
                        self.pack_id,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn preflight_cache_definitions(&mut self, pack_dir: &Path) -> Result<()> {
        let caches_dir = pack_dir.join("caches");
        if !caches_dir.exists() {
            return Ok(());
        }

        let mut seen_refs = HashSet::new();
        for (filename, content) in read_yaml_files(&caches_dir)? {
            let definition: CacheDefinitionYaml =
                serde_yaml_ng::from_str(&content).map_err(|error| {
                    Error::validation(format!(
                        "Failed to parse cache YAML {}: {}",
                        filename, error
                    ))
                })?;

            validate_pack_component_ref(&self.pack_ref, "cache definition", &definition.r#ref)?;
            if !seen_refs.insert(definition.r#ref.clone()) {
                return Err(Error::validation(format!(
                    "Duplicate cache definition ref '{}'",
                    definition.r#ref
                )));
            }
            definition.validate()?;

            match definition.owner_type {
                CacheDefinitionOwnerType::Pack => {
                    if definition.owner_ref != self.pack_ref {
                        return Err(Error::validation(format!(
                            "Cache definition '{}' must use installing pack '{}' as its pack owner",
                            definition.r#ref, self.pack_ref
                        )));
                    }
                }
                CacheDefinitionOwnerType::Action => validate_pack_component_ref(
                    &self.pack_ref,
                    "cache action owner",
                    &definition.owner_ref,
                )?,
                CacheDefinitionOwnerType::Sensor => validate_pack_component_ref(
                    &self.pack_ref,
                    "cache sensor owner",
                    &definition.owner_ref,
                )?,
            }
        }

        Ok(())
    }

    /// Load permission set definitions from `pack_dir/permission_sets/*.yaml`.
    ///
    /// Permission sets are pack-scoped authorization metadata. Their `grants`
    /// payload is stored verbatim and interpreted by the API authorization
    /// layer at request time.
    async fn load_permission_sets(
        &mut self,
        pack_dir: &Path,
        result: &mut PackLoadResult,
    ) -> Result<Vec<String>> {
        let permission_sets_dir = pack_dir.join("permission_sets");
        let mut loaded_refs = Vec::new();

        if !permission_sets_dir.exists() {
            info!(
                "No permission_sets directory found for pack '{}'",
                self.pack_ref
            );
            return Ok(loaded_refs);
        }

        let yaml_files = read_yaml_files(&permission_sets_dir)?;
        info!(
            "Found {} permission set definition(s) for pack '{}'",
            yaml_files.len(),
            self.pack_ref
        );

        for (filename, content) in &yaml_files {
            let data: serde_yaml_ng::Value = serde_yaml_ng::from_str(content).map_err(|e| {
                Error::validation(format!(
                    "Failed to parse permission set YAML {}: {}",
                    filename, e
                ))
            })?;

            let permission_set_ref = match data.get("ref").and_then(|v| v.as_str()) {
                Some(r) => r.to_string(),
                None => {
                    let msg = format!(
                        "Permission set YAML {} missing 'ref' field, skipping",
                        filename
                    );
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.permission_sets_skipped += 1;
                    continue;
                }
            };

            let label = data
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let description = data
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let grants = data
                .get("grants")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or_else(|| serde_json::json!([]));

            if !grants.is_array() {
                let msg = format!(
                    "Permission set '{}' has non-array 'grants', skipping",
                    permission_set_ref
                );
                warn!("{}", msg);
                result.warnings.push(msg);
                result.permission_sets_skipped += 1;
                continue;
            }

            let parsed_grants: Vec<Grant> =
                serde_json::from_value(grants.clone()).map_err(|error| {
                    Error::validation(format!(
                        "Permission set '{}' has invalid grants: {}",
                        permission_set_ref, error
                    ))
                })?;
            for grant in &parsed_grants {
                if grant.resource == Resource::Caches {
                    if let Some(constraints) = &grant.constraints {
                        validate_cache_grant_constraints(constraints).map_err(|error| {
                            Error::validation(format!(
                                "Permission set '{}' has invalid cache grant constraints: {}",
                                permission_set_ref, error
                            ))
                        })?;
                    }
                }
            }

            if let Some(existing) =
                PermissionSetRepository::find_by_ref(&mut *self.connection, &permission_set_ref)
                    .await?
            {
                ensure_existing_owner(
                    "permission set",
                    &permission_set_ref,
                    existing.pack,
                    self.pack_id,
                )?;
                let update_input = UpdatePermissionSetInput {
                    label,
                    description,
                    grants: Some(grants),
                };

                match PermissionSetRepository::update(
                    &mut *self.connection,
                    existing.id,
                    update_input,
                )
                .await
                {
                    Ok(_) => {
                        info!(
                            "Updated permission set '{}' (ID: {})",
                            permission_set_ref, existing.id
                        );
                        result.permission_sets_updated += 1;
                    }
                    Err(e) => {
                        let msg = format!(
                            "Failed to update permission set '{}': {}",
                            permission_set_ref, e
                        );
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.permission_sets_skipped += 1;
                    }
                }
                loaded_refs.push(permission_set_ref);
                continue;
            }

            let input = CreatePermissionSetInput {
                r#ref: permission_set_ref.clone(),
                pack: Some(self.pack_id),
                pack_ref: Some(self.pack_ref.clone()),
                label,
                description,
                grants,
            };

            match PermissionSetRepository::create(&mut *self.connection, input).await {
                Ok(permission_set) => {
                    info!(
                        "Created permission set '{}' (ID: {})",
                        permission_set_ref, permission_set.id
                    );
                    result.permission_sets_loaded += 1;
                    loaded_refs.push(permission_set_ref);
                }
                Err(e) => {
                    let msg = format!(
                        "Failed to create permission set '{}': {}",
                        permission_set_ref, e
                    );
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.permission_sets_skipped += 1;
                }
            }
        }

        Ok(loaded_refs)
    }

    /// Load runtime definitions from `pack_dir/runtimes/*.yaml`.
    ///
    /// Runtimes define how actions and sensors are executed (interpreter,
    /// environment setup, dependency management). They are loaded first
    /// since actions reference them.
    ///
    /// Returns the set of runtime refs that were loaded (for cleanup).
    async fn load_runtimes(
        &mut self,
        pack_dir: &Path,
        result: &mut PackLoadResult,
    ) -> Result<Vec<String>> {
        let runtimes_dir = pack_dir.join("runtimes");
        let mut loaded_refs = Vec::new();

        if !runtimes_dir.exists() {
            info!("No runtimes directory found for pack '{}'", self.pack_ref);
            return Ok(loaded_refs);
        }

        let yaml_files = read_yaml_files(&runtimes_dir)?;
        info!(
            "Found {} runtime definition(s) for pack '{}'",
            yaml_files.len(),
            self.pack_ref
        );

        for (filename, content) in &yaml_files {
            let data: serde_yaml_ng::Value = serde_yaml_ng::from_str(content).map_err(|e| {
                Error::validation(format!("Failed to parse runtime YAML {}: {}", filename, e))
            })?;

            let runtime_ref = match data.get("ref").and_then(|v| v.as_str()) {
                Some(r) => r.to_string(),
                None => {
                    let msg = format!("Runtime YAML {} missing 'ref' field, skipping", filename);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    continue;
                }
            };

            let name = data
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| extract_name_from_ref(&runtime_ref));

            let description = data
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let distributions = data
                .get("distributions")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            let installation = data
                .get("installation")
                .and_then(|v| serde_json::to_value(v).ok());

            let execution_config = data
                .get("execution_config")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            let aliases: Vec<String> = data
                .get("aliases")
                .and_then(|v| v.as_sequence())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_ascii_lowercase()))
                        .collect()
                })
                .unwrap_or_default();

            // Check if runtime already exists — update in place if so
            if let Some(existing) =
                RuntimeRepository::find_by_ref(&mut *self.connection, &runtime_ref).await?
            {
                ensure_existing_owner("runtime", &runtime_ref, existing.pack, self.pack_id)?;
                let update_input = UpdateRuntimeInput {
                    description: Some(match description {
                        Some(description) => Patch::Set(description),
                        None => Patch::Clear,
                    }),
                    name: Some(name),
                    distributions: Some(distributions),
                    installation: Some(match installation {
                        Some(installation) => Patch::Set(installation),
                        None => Patch::Clear,
                    }),
                    execution_config: Some(execution_config),
                    aliases: Some(aliases),
                    ..Default::default()
                };

                match RuntimeRepository::update(&mut *self.connection, existing.id, update_input)
                    .await
                {
                    Ok(_) => {
                        info!("Updated runtime '{}' (ID: {})", runtime_ref, existing.id);
                        result.runtimes_updated += 1;

                        // Also upsert version entries
                        self.load_runtime_versions(&data, existing.id, &runtime_ref, result)
                            .await;
                    }
                    Err(e) => {
                        let msg = format!("Failed to update runtime '{}': {}", runtime_ref, e);
                        warn!("{}", msg);
                        result.warnings.push(msg);
                    }
                }
                loaded_refs.push(runtime_ref);
                continue;
            }

            let input = CreateRuntimeInput {
                r#ref: runtime_ref.clone(),
                pack: Some(self.pack_id),
                pack_ref: Some(self.pack_ref.clone()),
                description,
                name,
                distributions,
                installation,
                execution_config,
                aliases,
                auto_detected: false,
                detection_config: serde_json::json!({}),
            };

            match RuntimeRepository::create(&mut *self.connection, input).await {
                Ok(rt) => {
                    info!("Created runtime '{}' (ID: {})", runtime_ref, rt.id);
                    result.runtimes_loaded += 1;
                    loaded_refs.push(runtime_ref.clone());

                    // Load version entries from the optional `versions` array
                    self.load_runtime_versions(&data, rt.id, &runtime_ref, result)
                        .await;
                }
                Err(e) => {
                    // Check for unique constraint violation (race condition)
                    if let Error::Database(sqlx::Error::Database(ref inner)) = e {
                        if inner.is_unique_violation() {
                            info!(
                                "Runtime '{}' already exists (concurrent creation), treating as update",
                                runtime_ref
                            );
                            loaded_refs.push(runtime_ref);
                            result.runtimes_updated += 1;
                            continue;
                        }
                    }
                    let msg = format!("Failed to create runtime '{}': {}", runtime_ref, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                }
            }
        }

        Ok(loaded_refs)
    }

    /// Load runtime version entries from a runtime's YAML `versions` array.
    ///
    /// Uses upsert: existing versions (by runtime + version string) are updated,
    /// new versions are created.
    async fn load_runtime_versions(
        &mut self,
        data: &serde_yaml_ng::Value,
        runtime_id: Id,
        runtime_ref: &str,
        result: &mut PackLoadResult,
    ) {
        let versions = match data.get("versions").and_then(|v| v.as_sequence()) {
            Some(seq) => seq,
            None => return, // No versions defined — that's fine
        };

        info!(
            "Loading {} version(s) for runtime '{}'",
            versions.len(),
            runtime_ref
        );

        // Collect version strings we loaded so we can clean up removed versions
        let mut loaded_versions = Vec::new();

        for entry in versions {
            let version_str = match entry.get("version").and_then(|v| v.as_str()) {
                Some(v) => v.to_string(),
                None => {
                    let msg = format!(
                        "Runtime '{}' has a version entry without a 'version' field, skipping",
                        runtime_ref
                    );
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    continue;
                }
            };

            let (version_major, version_minor, version_patch) =
                extract_version_components(&version_str);

            let execution_config = entry
                .get("execution_config")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            let distributions = entry
                .get("distributions")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            let is_default = entry
                .get("is_default")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let meta = entry
                .get("meta")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            // Check if this version already exists — update in place if so
            if let Ok(Some(existing)) = RuntimeVersionRepository::find_by_runtime_and_version(
                &mut *self.connection,
                runtime_id,
                &version_str,
            )
            .await
            {
                let update_input = UpdateRuntimeVersionInput {
                    version: None, // version string doesn't change
                    version_major: Some(match version_major {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    version_minor: Some(match version_minor {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    version_patch: Some(match version_patch {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    execution_config: Some(execution_config),
                    distributions: Some(distributions),
                    is_default: Some(is_default),
                    available: None, // preserve current availability — verification sets this
                    verified_at: None,
                    meta: Some(meta),
                };

                match RuntimeVersionRepository::update(
                    &mut *self.connection,
                    existing.id,
                    update_input,
                )
                .await
                {
                    Ok(_) => {
                        info!(
                            "Updated version '{}' for runtime '{}' (ID: {})",
                            version_str, runtime_ref, existing.id
                        );
                    }
                    Err(e) => {
                        let msg = format!(
                            "Failed to update version '{}' for runtime '{}': {}",
                            version_str, runtime_ref, e
                        );
                        warn!("{}", msg);
                        result.warnings.push(msg);
                    }
                }
                loaded_versions.push(version_str);
                continue;
            }

            let input = CreateRuntimeVersionInput {
                runtime: runtime_id,
                runtime_ref: runtime_ref.to_string(),
                version: version_str.clone(),
                version_major,
                version_minor,
                version_patch,
                execution_config,
                distributions,
                is_default,
                available: false, // Workers must verify the version before it becomes selectable
                meta,
            };

            match RuntimeVersionRepository::create(&mut *self.connection, input).await {
                Ok(rv) => {
                    info!(
                        "Created version '{}' for runtime '{}' (ID: {})",
                        version_str, runtime_ref, rv.id
                    );
                    loaded_versions.push(version_str);
                }
                Err(e) => {
                    // Check for unique constraint violation (race condition)
                    if let Error::Database(sqlx::Error::Database(ref inner)) = e {
                        if inner.is_unique_violation() {
                            info!(
                                "Version '{}' for runtime '{}' already exists (concurrent), skipping",
                                version_str, runtime_ref
                            );
                            loaded_versions.push(version_str);
                            continue;
                        }
                    }
                    let msg = format!(
                        "Failed to create version '{}' for runtime '{}': {}",
                        version_str, runtime_ref, e
                    );
                    warn!("{}", msg);
                    result.warnings.push(msg);
                }
            }
        }

        // Clean up versions that are no longer in the YAML
        if let Ok(existing_versions) =
            RuntimeVersionRepository::find_by_runtime(&mut *self.connection, runtime_id).await
        {
            for existing in existing_versions {
                if !loaded_versions.contains(&existing.version) {
                    info!(
                        "Removing stale version '{}' for runtime '{}'",
                        existing.version, runtime_ref
                    );
                    if let Err(e) =
                        RuntimeVersionRepository::delete(&mut *self.connection, existing.id).await
                    {
                        warn!(
                            "Failed to delete stale version '{}' for runtime '{}': {}",
                            existing.version, runtime_ref, e
                        );
                    }
                }
            }
        }
    }

    /// Load trigger definitions from `pack_dir/triggers/*.yaml`.
    ///
    /// Returns a map of trigger ref -> trigger ID for use by sensor loading,
    /// and the list of loaded trigger refs for cleanup.
    async fn load_triggers(
        &mut self,
        pack_dir: &Path,
        result: &mut PackLoadResult,
    ) -> Result<(HashMap<String, Id>, Vec<String>)> {
        let triggers_dir = pack_dir.join("triggers");
        let mut trigger_ids = HashMap::new();
        let mut loaded_refs = Vec::new();

        if !triggers_dir.exists() {
            info!("No triggers directory found for pack '{}'", self.pack_ref);
            return Ok((trigger_ids, loaded_refs));
        }

        let yaml_files = read_yaml_files(&triggers_dir)?;
        info!(
            "Found {} trigger definition(s) for pack '{}'",
            yaml_files.len(),
            self.pack_ref
        );

        for (filename, content) in &yaml_files {
            let data: serde_yaml_ng::Value = serde_yaml_ng::from_str(content).map_err(|e| {
                Error::validation(format!("Failed to parse trigger YAML {}: {}", filename, e))
            })?;

            let trigger_ref = match data.get("ref").and_then(|v| v.as_str()) {
                Some(r) => r.to_string(),
                None => {
                    let msg = format!("Trigger YAML {} missing 'ref' field, skipping", filename);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    continue;
                }
            };

            let name = extract_name_from_ref(&trigger_ref);
            let label = data
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| generate_label(&name));

            let description = data
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let enabled = data.get("enabled").and_then(|v| v.as_bool());
            let reference_visibility =
                parse_action_reference_visibility(data.get("reference_visibility"))?;
            let reference_allowed_pack_refs =
                parse_reference_allowed_pack_refs(data.get("reference_allowed_pack_refs"))?;
            validate_trigger_reference_visibility_config(
                reference_visibility,
                &reference_allowed_pack_refs,
            )?;

            let param_schema = data
                .get("parameters")
                .and_then(|v| serde_json::to_value(v).ok());

            let out_schema = data
                .get("output")
                .and_then(|v| serde_json::to_value(v).ok());

            // Check if trigger already exists — update in place if so
            if let Some(existing) =
                TriggerRepository::find_by_ref(&mut *self.connection, &trigger_ref).await?
            {
                ensure_existing_owner("trigger", &trigger_ref, existing.pack, self.pack_id)?;
                let update_input = UpdateTriggerInput {
                    label: Some(label),
                    description: Some(match description {
                        Some(description) => Patch::Set(description),
                        None => Patch::Clear,
                    }),
                    enabled,
                    param_schema: Some(match param_schema {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    out_schema: Some(match out_schema {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    sensor: None,
                    sensor_ref: None,
                    reference_visibility: Some(reference_visibility),
                    reference_allowed_pack_refs: Some(reference_allowed_pack_refs.clone()),
                };

                match TriggerRepository::update(&mut *self.connection, existing.id, update_input)
                    .await
                {
                    Ok(_) => {
                        info!("Updated trigger '{}' (ID: {})", trigger_ref, existing.id);
                        result.triggers_updated += 1;
                    }
                    Err(e) => {
                        let msg = format!("Failed to update trigger '{}': {}", trigger_ref, e);
                        warn!("{}", msg);
                        result.warnings.push(msg);
                    }
                }
                trigger_ids.insert(trigger_ref.clone(), existing.id);
                loaded_refs.push(trigger_ref);
                continue;
            }

            let input = CreateTriggerInput {
                r#ref: trigger_ref.clone(),
                pack: Some(self.pack_id),
                pack_ref: Some(self.pack_ref.clone()),
                label,
                description,
                enabled: enabled.unwrap_or(true),
                param_schema,
                out_schema,
                sensor: None,
                sensor_ref: None,
                is_adhoc: false,
                reference_visibility,
                reference_allowed_pack_refs,
            };

            match TriggerRepository::create(&mut *self.connection, input).await {
                Ok(trigger) => {
                    info!("Created trigger '{}' (ID: {})", trigger_ref, trigger.id);
                    trigger_ids.insert(trigger_ref.clone(), trigger.id);
                    loaded_refs.push(trigger_ref);
                    result.triggers_loaded += 1;
                }
                Err(e) => {
                    let msg = format!("Failed to create trigger '{}': {}", trigger_ref, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                }
            }
        }

        Ok((trigger_ids, loaded_refs))
    }

    /// Load action definitions from `pack_dir/actions/*.yaml`.
    ///
    /// Returns the list of loaded action refs for cleanup.
    ///
    /// When an action YAML contains a `workflow_file` field, the loader reads
    /// the referenced workflow definition, creates/updates a
    /// `workflow_definition` record, and links the action to it via the
    /// `action.workflow_def` FK. This enables the action YAML to control
    /// action-level metadata independently of the workflow graph, and allows
    /// multiple actions to share the same workflow file.
    async fn load_actions(
        &mut self,
        pack_dir: &Path,
        result: &mut PackLoadResult,
    ) -> Result<Vec<String>> {
        let actions_dir = pack_dir.join("actions");
        let mut loaded_refs = Vec::new();

        if !actions_dir.exists() {
            info!("No actions directory found for pack '{}'", self.pack_ref);
            return Ok(loaded_refs);
        }

        let yaml_files = read_yaml_files(&actions_dir)?;
        info!(
            "Found {} action definition(s) for pack '{}'",
            yaml_files.len(),
            self.pack_ref
        );

        // Workflows validate their referenced actions while loading. Load all
        // regular action metadata first so file names cannot affect that validation.
        let mut regular_action_files = Vec::with_capacity(yaml_files.len());
        let mut workflow_action_files = Vec::new();
        for (filename, content) in yaml_files {
            let data: serde_yaml_ng::Value = serde_yaml_ng::from_str(&content).map_err(|e| {
                Error::validation(format!("Failed to parse action YAML {}: {}", filename, e))
            })?;
            if data.get("workflow_file").is_some() {
                workflow_action_files.push((filename, content));
            } else {
                regular_action_files.push((filename, content));
            }
        }
        regular_action_files.extend(workflow_action_files);

        for (filename, content) in &regular_action_files {
            let data: serde_yaml_ng::Value = serde_yaml_ng::from_str(content).map_err(|e| {
                Error::validation(format!("Failed to parse action YAML {}: {}", filename, e))
            })?;

            let action_ref = match data.get("ref").and_then(|v| v.as_str()) {
                Some(r) => r.to_string(),
                None => {
                    let msg = format!("Action YAML {} missing 'ref' field, skipping", filename);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    continue;
                }
            };

            let name = extract_name_from_ref(&action_ref);
            let label = data
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| generate_label(&name));

            let description = data
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // ── Workflow file handling ──────────────────────────────────
            // If the action declares `workflow_file`, load the referenced
            // workflow definition and link the action to it.
            let workflow_file_field = data
                .get("workflow_file")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let workflow_def_id: Option<Id> = if let Some(ref wf_path) = workflow_file_field {
                match self
                    .load_workflow_for_action(
                        &actions_dir,
                        wf_path,
                        &action_ref,
                        &label,
                        description.as_deref().unwrap_or(""),
                        &data,
                    )
                    .await
                {
                    Ok(id) => Some(id),
                    Err(e) => {
                        let msg = format!(
                            "Failed to load workflow file '{}' for action '{}': {}",
                            wf_path, action_ref, e
                        );
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        // Continue creating the action without workflow link
                        None
                    }
                }
            } else {
                None
            };

            // For workflow actions the entrypoint is the workflow file path;
            // for regular actions it comes from entry_point in the YAML.
            let entrypoint = if let Some(ref wf_path) = workflow_file_field {
                wf_path.clone()
            } else {
                data.get("entry_point")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };

            // Resolve runtime ID from runner_type (workflow actions have no
            // runner_type and get runtime = None).
            let runtime_id = if workflow_file_field.is_some() {
                None
            } else {
                let runner_type = data
                    .get("runner_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("shell");
                self.resolve_runtime_id(runner_type).await?
            };

            let param_schema = data
                .get("parameters")
                .and_then(|v| serde_json::to_value(v).ok());

            let out_schema = data
                .get("output")
                .and_then(|v| serde_json::to_value(v).ok());
            let enabled = data.get("enabled").and_then(|v| v.as_bool());

            let parameter_delivery = data
                .get("parameter_delivery")
                .and_then(|v| v.as_str())
                .unwrap_or("stdin")
                .to_lowercase();

            let parameter_format = data
                .get("parameter_format")
                .and_then(|v| v.as_str())
                .unwrap_or("json")
                .to_lowercase();

            let output_format = data
                .get("output_format")
                .and_then(|v| v.as_str())
                .unwrap_or("text")
                .to_lowercase();

            // Optional runtime version constraint (e.g., ">=3.12", "~18.0")
            let runtime_version_constraint = data
                .get("runtime_version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let required_worker_runtimes = data
                .get("required_worker_runtimes")
                .map(|value| serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!({})))
                .unwrap_or_else(|| serde_json::json!({}));
            let worker_selector = data
                .get("worker_selector")
                .map(|value| serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!({})))
                .unwrap_or_else(|| serde_json::json!({}));
            let worker_tolerations = data
                .get("worker_tolerations")
                .map(|value| serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!([])))
                .unwrap_or_else(|| serde_json::json!([]));
            let worker_affinity = data
                .get("worker_affinity")
                .map(|value| serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!({})))
                .unwrap_or_else(|| serde_json::json!({}));

            let accesses_mcp = data
                .get("accesses_mcp")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let default_execution_permission_set_refs: Vec<String> = data
                .get("default_execution_permission_set_refs")
                .and_then(|v| v.as_sequence())
                .map(|refs| {
                    refs.iter()
                        .filter_map(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let reference_visibility =
                parse_action_reference_visibility(data.get("reference_visibility"))?;
            let reference_allowed_pack_refs =
                parse_reference_allowed_pack_refs(data.get("reference_allowed_pack_refs"))?;
            validate_action_reference_visibility_config(
                reference_visibility,
                &reference_allowed_pack_refs,
            )?;
            let log_retention_policy =
                parse_log_retention_policy(data.get("log_retention_policy"))?;
            let log_retention_limit = parse_log_retention_limit(data.get("log_retention_limit"))?;
            let artifact_retention_policy =
                parse_log_retention_policy(data.get("artifact_retention_policy"))?;
            let artifact_retention_limit =
                parse_log_retention_limit(data.get("artifact_retention_limit"))?;
            let timeout_seconds = parse_timeout_seconds(data.get("timeout_seconds"))?;

            // Check if action already exists — update in place if so
            if let Some(existing) =
                ActionRepository::find_by_ref(&mut *self.connection, &action_ref).await?
            {
                ensure_existing_owner("action", &action_ref, Some(existing.pack), self.pack_id)?;
                let update_input = UpdateActionInput {
                    label: Some(label),
                    description: Some(match description {
                        Some(description) => Patch::Set(description),
                        None => Patch::Clear,
                    }),
                    entrypoint: Some(entrypoint),
                    runtime: runtime_id,
                    enabled,
                    runtime_version_constraint: Some(match runtime_version_constraint {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    required_worker_runtimes: Some(required_worker_runtimes.clone()),
                    worker_selector: Some(worker_selector.clone()),
                    worker_tolerations: Some(worker_tolerations.clone()),
                    worker_affinity: Some(worker_affinity.clone()),
                    param_schema,
                    out_schema,
                    parameter_delivery: Some(parameter_delivery),
                    parameter_format: Some(parameter_format),
                    output_format: Some(output_format),
                    accesses_mcp: Some(accesses_mcp),
                    default_execution_permission_set_refs: Some(
                        default_execution_permission_set_refs.clone(),
                    ),
                    reference_visibility: Some(reference_visibility),
                    reference_allowed_pack_refs: Some(reference_allowed_pack_refs.clone()),
                    artifact_retention_policy: Some(match artifact_retention_policy {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    artifact_retention_limit: Some(match artifact_retention_limit {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    log_retention_policy: Some(match log_retention_policy {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    log_retention_limit: Some(match log_retention_limit {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    timeout_seconds: Some(match timeout_seconds {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                };

                match ActionRepository::update(&mut *self.connection, existing.id, update_input)
                    .await
                {
                    Ok(_) => {
                        info!("Updated action '{}' (ID: {})", action_ref, existing.id);
                        result.actions_updated += 1;

                        // Re-link workflow definition if present
                        if let Some(wf_id) = workflow_def_id {
                            if let Err(e) = ActionRepository::link_workflow_def(
                                &mut *self.connection,
                                existing.id,
                                wf_id,
                            )
                            .await
                            {
                                warn!(
                                    "Failed to link workflow def {} to action '{}': {}",
                                    wf_id, action_ref, e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        let msg = format!("Failed to update action '{}': {}", action_ref, e);
                        warn!("{}", msg);
                        result.warnings.push(msg);
                    }
                }
                loaded_refs.push(action_ref);
                continue;
            }

            // Use raw SQL to include parameter_delivery, parameter_format,
            // output_format which are not in CreateActionInput
            let create_result = sqlx::query_scalar::<_, i64>(
                r#"
                INSERT INTO action (
                    ref, pack, pack_ref, label, description, entrypoint,
                    runtime, enabled, runtime_version_constraint, required_worker_runtimes,
                    worker_selector, worker_tolerations, worker_affinity,
                    param_schema, out_schema, is_adhoc, parameter_delivery, parameter_format,
                    output_format, accesses_mcp, default_execution_permission_set_refs,
                    reference_visibility, reference_allowed_pack_refs,
                    log_retention_policy, log_retention_limit,
                    artifact_retention_policy, artifact_retention_limit, timeout_seconds
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28)
                RETURNING id
                "#,
            )
            .bind(&action_ref)
            .bind(self.pack_id)
            .bind(&self.pack_ref)
            .bind(&label)
            .bind(&description)
            .bind(&entrypoint)
            .bind(runtime_id)
            .bind(enabled.unwrap_or(true))
            .bind(&runtime_version_constraint)
            .bind(&required_worker_runtimes)
            .bind(&worker_selector)
            .bind(&worker_tolerations)
            .bind(&worker_affinity)
            .bind(&param_schema)
            .bind(&out_schema)
            .bind(false) // is_adhoc
            .bind(&parameter_delivery)
            .bind(&parameter_format)
            .bind(&output_format)
            .bind(accesses_mcp)
            .bind(&default_execution_permission_set_refs)
            .bind(reference_visibility)
            .bind(&reference_allowed_pack_refs)
            .bind(log_retention_policy)
            .bind(log_retention_limit)
            .bind(artifact_retention_policy)
            .bind(artifact_retention_limit)
            .bind(timeout_seconds)
            .fetch_one(&mut *self.connection)
            .await;

            match create_result {
                Ok(id) => {
                    info!("Created action '{}' (ID: {})", action_ref, id);
                    loaded_refs.push(action_ref.clone());
                    result.actions_loaded += 1;

                    // Link workflow definition if present
                    if let Some(wf_id) = workflow_def_id {
                        if let Err(e) =
                            ActionRepository::link_workflow_def(&mut *self.connection, id, wf_id)
                                .await
                        {
                            warn!(
                                "Failed to link workflow def {} to new action '{}': {}",
                                wf_id, action_ref, e
                            );
                        } else {
                            info!(
                                "Linked action '{}' (ID: {}) to workflow definition (ID: {})",
                                action_ref, id, wf_id
                            );
                        }
                    }
                }
                Err(e) => {
                    // Check for unique constraint violation (already exists race condition)
                    if let sqlx::Error::Database(ref db_err) = e {
                        if db_err.is_unique_violation() {
                            info!(
                                "Action '{}' already exists (concurrent creation), treating as update",
                                action_ref
                            );
                            loaded_refs.push(action_ref);
                            result.actions_updated += 1;
                            continue;
                        }
                    }
                    let msg = format!("Failed to create action '{}': {}", action_ref, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                }
            }
        }

        Ok(loaded_refs)
    }

    /// Load work queue definitions from `pack_dir/queues/*.yaml`.
    async fn load_dashboards(
        &mut self,
        pack_dir: &Path,
        result: &mut PackLoadResult,
    ) -> Result<Vec<String>> {
        let dashboards_dir = pack_dir.join("dashboards");
        let mut loaded_refs = Vec::new();

        if !dashboards_dir.exists() {
            info!("No dashboards directory found for pack '{}'", self.pack_ref);
            return Ok(loaded_refs);
        }

        let yaml_files = read_yaml_files(&dashboards_dir)?;
        info!(
            "Found {} dashboard definition(s) for pack '{}'",
            yaml_files.len(),
            self.pack_ref
        );

        for (filename, content) in &yaml_files {
            let yaml_value: serde_yaml_ng::Value = match serde_yaml_ng::from_str(content) {
                Ok(data) => data,
                Err(e) => {
                    let msg = format!("Failed to parse dashboard YAML {}: {}", filename, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.dashboards_skipped += 1;
                    continue;
                }
            };

            let dashboard_ref = match yaml_value.get("ref").and_then(|v| v.as_str()) {
                Some(r) if !r.trim().is_empty() => qualify_pack_ref(&self.pack_ref, r.trim()),
                _ => {
                    let msg = format!("Dashboard YAML {} missing 'ref' field, skipping", filename);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.dashboards_skipped += 1;
                    continue;
                }
            };

            let label = yaml_value
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| generate_label(&extract_name_from_ref(&dashboard_ref)));

            let description = yaml_value
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let enabled = yaml_value
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let is_default_home = yaml_value
                .get("is_default_home")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let spec_version = yaml_value
                .get("spec_version")
                .and_then(|v| v.as_i64())
                .unwrap_or(1) as i32;
            let scope_type = match yaml_value.get("scope_type").and_then(|v| v.as_str()) {
                Some(raw) => match parse_dashboard_scope_type(raw) {
                    Ok(scope) => scope,
                    Err(msg) => {
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.dashboards_skipped += 1;
                        continue;
                    }
                },
                None => DashboardScopeType::Pack,
            };
            let scope_ref = yaml_value
                .get("scope_ref")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| self.pack_ref.clone());
            let visibility = match yaml_value.get("visibility").and_then(|v| v.as_str()) {
                Some(raw) => match parse_dashboard_visibility(raw) {
                    Ok(visibility) => visibility,
                    Err(msg) => {
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.dashboards_skipped += 1;
                        continue;
                    }
                },
                None => DashboardVisibility::Pack,
            };
            let tags = yaml_value
                .get("tags")
                .and_then(|v| v.as_sequence())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let spec = serde_json::to_value(&yaml_value).unwrap_or_else(|_| serde_json::json!({}));
            if let Err(validation_error) = validate_dashboard_spec(&spec) {
                let msg = format!(
                    "Dashboard '{}' in {} failed spec validation: {}",
                    dashboard_ref, filename, validation_error
                );
                warn!("{}", msg);
                result.warnings.push(msg);
                result.dashboards_skipped += 1;
                continue;
            }

            if let Some(existing) = DashboardRepository::find_by_ref_in_scope(
                &mut *self.connection,
                &DashboardScopedRef {
                    scope_type,
                    scope_ref: scope_ref.clone(),
                    r#ref: dashboard_ref.clone(),
                },
            )
            .await?
            {
                ensure_existing_owner("dashboard", &dashboard_ref, existing.pack, self.pack_id)?;
                let update_input = UpdateDashboardInput {
                    scope_type: Some(scope_type),
                    scope_ref: Some(scope_ref.clone()),
                    pack: Some(Patch::Set(self.pack_id)),
                    owner_identity: Some(Patch::Clear),
                    visibility: Some(visibility),
                    is_adhoc: Some(false),
                    label: Some(label.clone()),
                    description: Some(match description.clone() {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    enabled: Some(enabled),
                    is_default_home: Some(is_default_home),
                    spec_version: Some(spec_version),
                    spec: Some(spec),
                    tags: Some(tags),
                    expected_revision: None,
                    updated_by: None,
                };

                match DashboardRepository::update_with_version_in_transaction(
                    &mut *self.connection,
                    existing.id,
                    update_input,
                )
                .await
                {
                    Ok(_) => {
                        info!(
                            "Updated dashboard '{}' (ID: {})",
                            dashboard_ref, existing.id
                        );
                        result.dashboards_updated += 1;
                        loaded_refs.push(dashboard_ref);
                    }
                    Err(e) => {
                        let msg = format!("Failed to update dashboard '{}': {}", dashboard_ref, e);
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.dashboards_skipped += 1;
                    }
                }
                continue;
            }

            match DashboardRepository::create(
                &mut *self.connection,
                CreateDashboardInput {
                    r#ref: dashboard_ref.clone(),
                    scope_type,
                    scope_ref: scope_ref.clone(),
                    pack: Some(self.pack_id),
                    owner_identity: None,
                    visibility,
                    is_adhoc: false,
                    label,
                    description,
                    enabled,
                    is_default_home,
                    spec_version,
                    spec,
                    tags,
                    created_by: None,
                },
            )
            .await
            {
                Ok(dashboard) => {
                    info!(
                        "Created dashboard '{}' (ID: {})",
                        dashboard.r#ref, dashboard.id
                    );
                    result.dashboards_loaded += 1;
                    loaded_refs.push(dashboard.r#ref);
                }
                Err(e) => {
                    let msg = format!("Failed to create dashboard '{}': {}", dashboard_ref, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.dashboards_skipped += 1;
                }
            }
        }

        Ok(loaded_refs)
    }

    /// Load work queue definitions from `pack_dir/queues/*.yaml`.
    async fn load_queues(
        &mut self,
        pack_dir: &Path,
        result: &mut PackLoadResult,
    ) -> Result<Vec<String>> {
        let queues_dir = pack_dir.join("queues");
        let mut loaded_refs = Vec::new();

        if !queues_dir.exists() {
            info!("No queues directory found for pack '{}'", self.pack_ref);
            return Ok(loaded_refs);
        }

        let yaml_files = read_yaml_files(&queues_dir)?;
        info!(
            "Found {} work queue definition(s) for pack '{}'",
            yaml_files.len(),
            self.pack_ref
        );

        for (filename, content) in &yaml_files {
            let definition = match parse_work_queue_definition_yaml(content) {
                Ok(definition) => definition,
                Err(e) => {
                    let msg = format!("Failed to parse work queue YAML {}: {}", filename, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.queues_skipped += 1;
                    continue;
                }
            };

            let dispatch_action = match ActionRepository::find_by_ref(
                &mut *self.connection,
                &definition.dispatch_action,
            )
            .await?
            {
                Some(action) => action,
                None => {
                    let msg = format!(
                        "Work queue '{}' references unknown action '{}', skipping",
                        definition.r#ref, definition.dispatch_action
                    );
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.queues_skipped += 1;
                    continue;
                }
            };
            if let Err(e) = ensure_action_reference_allowed(
                &dispatch_action,
                Some(&self.pack_ref),
                "work queue",
                &definition.r#ref,
            ) {
                let msg = format!("{}", e);
                warn!("{}", msg);
                result.warnings.push(msg);
                result.queues_skipped += 1;
                continue;
            }

            let queue_yaml: serde_yaml_ng::Value =
                serde_yaml_ng::from_str(content).unwrap_or(serde_yaml_ng::Value::Null);
            let enabled = queue_yaml.get("enabled").and_then(|value| value.as_bool());
            let accepting_new_items = queue_yaml
                .get("accepting_new_items")
                .and_then(|value| value.as_bool());

            if let Some(existing) =
                WorkQueueRepository::find_by_ref(&mut *self.connection, &definition.r#ref).await?
            {
                ensure_existing_owner(
                    "work queue",
                    &definition.r#ref,
                    existing.pack,
                    self.pack_id,
                )?;
                let update_input = UpdateWorkQueueInput {
                    pack: Some(Patch::Set(self.pack_id)),
                    pack_ref: Some(Patch::Set(self.pack_ref.clone())),
                    is_adhoc: Some(false),
                    label: Some(definition.label.clone()),
                    description: Some(match definition.description.clone() {
                        Some(description) => Patch::Set(description),
                        None => Patch::Clear,
                    }),
                    enabled,
                    accepting_new_items,
                    dispatch_action: Some(Patch::Set(dispatch_action.id)),
                    dispatch_action_ref: Some(definition.dispatch_action.clone()),
                    default_priority: Some(definition.default_priority),
                    allow_pending_update: Some(definition.allow_pending_update),
                    update_strategy: Some(definition.update_strategy),
                    batch_mode: Some(definition.batch_mode),
                    item_schema: Some(definition.item_schema.clone()),
                    action_params: Some(definition.action_params.clone()),
                    trace_tag_template: Some(match definition.trace_tag_template.clone() {
                        Some(template) => Patch::Set(template),
                        None => Patch::Clear,
                    }),
                    permission_set_refs: Some(match definition.permission_set_refs.clone() {
                        Some(refs) => Patch::Set(refs),
                        None => Patch::Clear,
                    }),
                    config: Some(definition.config.clone()),
                    reference_visibility: Some(definition.reference_visibility),
                    reference_allowed_pack_refs: Some(
                        definition.reference_allowed_pack_refs.clone(),
                    ),
                };

                match WorkQueueRepository::update(&mut *self.connection, existing.id, update_input)
                    .await
                {
                    Ok(_) => {
                        info!(
                            "Updated work queue '{}' (ID: {})",
                            definition.r#ref, existing.id
                        );
                        result.queues_updated += 1;
                        loaded_refs.push(definition.r#ref);
                    }
                    Err(e) => {
                        let msg =
                            format!("Failed to update work queue '{}': {}", definition.r#ref, e);
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.queues_skipped += 1;
                    }
                }
                continue;
            }

            match WorkQueueRepository::create(
                &mut *self.connection,
                CreateWorkQueueInput {
                    r#ref: definition.r#ref.clone(),
                    pack: Some(self.pack_id),
                    pack_ref: Some(self.pack_ref.clone()),
                    is_adhoc: false,
                    label: definition.label.clone(),
                    description: definition.description.clone(),
                    enabled: definition.enabled,
                    accepting_new_items: definition.accepting_new_items,
                    dispatch_action: Some(dispatch_action.id),
                    dispatch_action_ref: definition.dispatch_action.clone(),
                    default_priority: definition.default_priority,
                    allow_pending_update: definition.allow_pending_update,
                    update_strategy: definition.update_strategy,
                    batch_mode: definition.batch_mode,
                    item_schema: definition.item_schema.clone(),
                    action_params: definition.action_params.clone(),
                    trace_tag_template: definition.trace_tag_template.clone(),
                    permission_set_refs: definition.permission_set_refs.clone(),
                    config: definition.config.clone(),
                    reference_visibility: definition.reference_visibility,
                    reference_allowed_pack_refs: definition.reference_allowed_pack_refs.clone(),
                },
            )
            .await
            {
                Ok(queue) => {
                    info!("Created work queue '{}' (ID: {})", queue.r#ref, queue.id);
                    result.queues_loaded += 1;
                    loaded_refs.push(queue.r#ref);
                }
                Err(e) => {
                    let msg = format!("Failed to create work queue '{}': {}", definition.r#ref, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.queues_skipped += 1;
                }
            }
        }

        Ok(loaded_refs)
    }

    /// Load policy definitions from `pack_dir/policies/*.yaml`.
    async fn load_policies(
        &mut self,
        pack_dir: &Path,
        result: &mut PackLoadResult,
    ) -> Result<Vec<String>> {
        let policies_dir = pack_dir.join("policies");
        let mut loaded_refs = Vec::new();

        if !policies_dir.exists() {
            info!("No policies directory found for pack '{}'", self.pack_ref);
            return Ok(loaded_refs);
        }

        let yaml_files = read_yaml_files(&policies_dir)?;
        info!(
            "Found {} policy definition(s) for pack '{}'",
            yaml_files.len(),
            self.pack_ref
        );

        for (filename, content) in &yaml_files {
            let data: serde_yaml_ng::Value = match serde_yaml_ng::from_str(content) {
                Ok(data) => data,
                Err(e) => {
                    let msg = format!("Failed to parse policy YAML {}: {}", filename, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.policies_skipped += 1;
                    continue;
                }
            };

            let Some(raw_ref) = data.get("ref").and_then(|value| value.as_str()) else {
                let msg = format!("Policy YAML {} missing required 'ref'", filename);
                warn!("{}", msg);
                result.warnings.push(msg);
                result.policies_skipped += 1;
                continue;
            };
            let policy_ref = qualify_pack_ref(&self.pack_ref, raw_ref);

            let name = data
                .get("name")
                .or_else(|| data.get("label"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    policy_ref
                        .rsplit('.')
                        .next()
                        .unwrap_or(&policy_ref)
                        .to_string()
                });
            let description = data
                .get("description")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let enabled = data
                .get("enabled")
                .and_then(|value| value.as_bool())
                .unwrap_or(true);
            let priority = data
                .get("priority")
                .and_then(|value| value.as_i64())
                .unwrap_or(0) as i32;
            let tags = yaml_string_array(data.get("tags"));

            let action_ref = data
                .get("action_ref")
                .and_then(|value| value.as_str())
                .map(|value| qualify_pack_ref(&self.pack_ref, value));
            let explicit_pack_ref = data
                .get("pack_ref")
                .and_then(|value| value.as_str())
                .map(str::to_string);

            if explicit_pack_ref
                .as_deref()
                .is_some_and(|pack_ref| pack_ref != self.pack_ref)
            {
                let msg = format!(
                    "Policy '{}' declares pack_ref '{}' but is loaded from pack '{}', skipping",
                    policy_ref,
                    explicit_pack_ref.as_deref().unwrap_or_default(),
                    self.pack_ref
                );
                warn!("{}", msg);
                result.warnings.push(msg);
                result.policies_skipped += 1;
                continue;
            }

            let (action_id, resolved_action_ref) = match action_ref {
                Some(action_ref) => {
                    match ActionRepository::find_by_ref(&mut *self.connection, &action_ref).await? {
                        Some(action) => (Some(action.id), Some(action.r#ref.clone())),
                        None => {
                            let msg = format!(
                                "Policy '{}' references unknown action '{}', skipping",
                                policy_ref, action_ref
                            );
                            warn!("{}", msg);
                            result.warnings.push(msg);
                            result.policies_skipped += 1;
                            continue;
                        }
                    }
                }
                None => (None, None),
            };

            let controls = match parse_policy_controls(&data) {
                Ok(controls) => controls,
                Err(error)
                    if error.contains("must include both")
                        || error.contains("must configure concurrency")
                        || error.contains("must be greater than zero") =>
                {
                    let detail = error.strip_prefix("Policy ").unwrap_or(&error);
                    let msg = format!("Policy '{}' {}, skipping", policy_ref, detail);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.policies_skipped += 1;
                    continue;
                }
                Err(error) => return Err(Error::validation(error)),
            };
            let threshold = controls.threshold;
            let method = controls.method;
            let parameters = controls.parameters;
            let rate_limit_max_executions = controls.rate_limit_max_executions;
            let rate_limit_window_seconds = controls.rate_limit_window_seconds;
            let quotas = controls.quotas;

            if let Some(existing) =
                PolicyRepository::find_by_ref(&mut *self.connection, &policy_ref).await?
            {
                ensure_existing_owner("policy", &policy_ref, existing.pack, self.pack_id)?;
                let update = UpdatePolicyInput {
                    enabled: Some(enabled),
                    priority: Some(priority),
                    parameters: Some(parameters),
                    method: Some(method),
                    threshold: Some(threshold),
                    rate_limit_max_executions: Some(rate_limit_max_executions),
                    rate_limit_window_seconds: Some(rate_limit_window_seconds),
                    quotas: Some(quotas),
                    name: Some(name),
                    description: Some(description),
                    tags: Some(tags),
                };

                match PolicyRepository::update(&mut *self.connection, existing.id, update).await {
                    Ok(_) => {
                        info!("Updated policy '{}' (ID: {})", policy_ref, existing.id);
                        result.policies_updated += 1;
                        loaded_refs.push(policy_ref);
                    }
                    Err(e) => {
                        let msg = format!("Failed to update policy '{}': {}", policy_ref, e);
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.policies_skipped += 1;
                    }
                }
                continue;
            }

            let create = CreatePolicyInput {
                r#ref: policy_ref.clone(),
                pack: Some(self.pack_id),
                pack_ref: Some(self.pack_ref.clone()),
                action: action_id,
                action_ref: resolved_action_ref,
                enabled,
                priority,
                parameters,
                method,
                threshold,
                rate_limit_max_executions,
                rate_limit_window_seconds,
                quotas,
                name,
                description,
                tags,
            };

            match PolicyRepository::create(&mut *self.connection, create).await {
                Ok(policy) => {
                    info!("Created policy '{}' (ID: {})", policy.r#ref, policy.id);
                    result.policies_loaded += 1;
                    loaded_refs.push(policy.r#ref);
                }
                Err(e) => {
                    let msg = format!("Failed to create policy '{}': {}", policy_ref, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.policies_skipped += 1;
                }
            }
        }

        Ok(loaded_refs)
    }

    /// Load rule definitions from `pack_dir/rules/*.yaml`.
    ///
    /// Pack rules are declarative metadata. They are installed as non-ad-hoc
    /// rules owned by the pack and are cleaned up on pack reload when removed
    /// from the `rules/` directory.
    async fn load_rules(
        &mut self,
        pack_dir: &Path,
        _trigger_ids: &HashMap<String, Id>,
        result: &mut PackLoadResult,
    ) -> Result<Vec<String>> {
        let rules_dir = pack_dir.join("rules");
        let mut loaded_refs = Vec::new();

        if !rules_dir.exists() {
            info!("No rules directory found for pack '{}'", self.pack_ref);
            return Ok(loaded_refs);
        }

        let yaml_files = read_yaml_files(&rules_dir)?;
        info!(
            "Found {} rule definition(s) for pack '{}'",
            yaml_files.len(),
            self.pack_ref
        );

        for (filename, content) in &yaml_files {
            let data: serde_yaml_ng::Value = match serde_yaml_ng::from_str(content) {
                Ok(data) => data,
                Err(e) => {
                    let msg = format!("Failed to parse rule YAML {}: {}", filename, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.rules_skipped += 1;
                    continue;
                }
            };

            let rule_ref = match data.get("ref").and_then(|v| v.as_str()) {
                Some(r) if !r.trim().is_empty() => qualify_pack_ref(&self.pack_ref, r.trim()),
                _ => {
                    let msg = format!("Rule YAML {} missing 'ref' field, skipping", filename);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.rules_skipped += 1;
                    continue;
                }
            };

            let trigger_ref = match data.get("trigger_ref").and_then(|v| v.as_str()) {
                Some(r) if !r.trim().is_empty() => qualify_pack_ref(&self.pack_ref, r.trim()),
                _ => {
                    let msg = format!(
                        "Rule '{}' in {} missing 'trigger_ref' field, skipping",
                        rule_ref, filename
                    );
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.rules_skipped += 1;
                    continue;
                }
            };

            let action_ref = match data.get("action_ref").and_then(|v| v.as_str()) {
                Some(r) if !r.trim().is_empty() => qualify_pack_ref(&self.pack_ref, r.trim()),
                _ => {
                    let msg = format!(
                        "Rule '{}' in {} missing 'action_ref' field, skipping",
                        rule_ref, filename
                    );
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.rules_skipped += 1;
                    continue;
                }
            };

            let trigger =
                match TriggerRepository::find_by_ref(&mut *self.connection, &trigger_ref).await? {
                    Some(trigger) => trigger,
                    None => {
                        let msg = format!(
                            "Rule '{}' references unknown trigger '{}', skipping",
                            rule_ref, trigger_ref
                        );
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.rules_skipped += 1;
                        continue;
                    }
                };
            if let Err(e) =
                ensure_trigger_reference_allowed(&trigger, Some(&self.pack_ref), "rule", &rule_ref)
            {
                let msg = format!("{}", e);
                warn!("{}", msg);
                result.warnings.push(msg);
                result.rules_skipped += 1;
                continue;
            }

            let action =
                match ActionRepository::find_by_ref(&mut *self.connection, &action_ref).await? {
                    Some(action) => action,
                    None => {
                        let msg = format!(
                            "Rule '{}' references unknown action '{}', skipping",
                            rule_ref, action_ref
                        );
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.rules_skipped += 1;
                        continue;
                    }
                };
            if let Err(e) =
                ensure_action_reference_allowed(&action, Some(&self.pack_ref), "rule", &rule_ref)
            {
                let msg = format!("{}", e);
                warn!("{}", msg);
                result.warnings.push(msg);
                result.rules_skipped += 1;
                continue;
            }

            let name = extract_name_from_ref(&rule_ref);
            let label = data
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| generate_label(&name));
            let description = data
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let enabled = data.get("enabled").and_then(|v| v.as_bool());
            let conditions = data
                .get("conditions")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            let action_params = data
                .get("action_params")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            let trigger_params = data
                .get("trigger_params")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            let trace_tag_template = data
                .get("trace_tag_template")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let permission_set_refs = parse_optional_permission_set_refs(
                data.get("permission_set_refs")
                    .or_else(|| data.get("permission_set_ref")),
            )?;

            if let Some(existing) =
                RuleRepository::find_by_ref(&mut *self.connection, &rule_ref).await?
            {
                ensure_existing_owner("rule", &rule_ref, Some(existing.pack), self.pack_id)?;
                let update_input = UpdateRuleInput {
                    pack: Some(self.pack_id),
                    pack_ref: Some(self.pack_ref.clone()),
                    label: Some(label),
                    description: Some(match description {
                        Some(description) => Patch::Set(description),
                        None => Patch::Clear,
                    }),
                    action: Some(action.id),
                    action_ref: Some(action_ref),
                    trigger: Some(trigger.id),
                    trigger_ref: Some(trigger_ref),
                    conditions: Some(conditions),
                    action_params: Some(action_params),
                    trigger_params: Some(trigger_params),
                    trace_tag_template: Some(match trace_tag_template.clone() {
                        Some(template) => Patch::Set(template),
                        None => Patch::Clear,
                    }),
                    permission_set_refs: Some(match permission_set_refs.clone() {
                        Some(refs) => Patch::Set(refs),
                        None => Patch::Clear,
                    }),
                    enabled,
                    is_adhoc: Some(false),
                    owner_identity: Some(Patch::Clear),
                };

                match RuleRepository::update(&mut *self.connection, existing.id, update_input).await
                {
                    Ok(_) => {
                        info!("Updated rule '{}' (ID: {})", rule_ref, existing.id);
                        result.rules_updated += 1;
                        loaded_refs.push(rule_ref);
                    }
                    Err(e) => {
                        let msg = format!("Failed to update rule '{}': {}", rule_ref, e);
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.rules_skipped += 1;
                    }
                }
                continue;
            }

            match RuleRepository::create(
                &mut *self.connection,
                CreateRuleInput {
                    r#ref: rule_ref.clone(),
                    pack: self.pack_id,
                    pack_ref: self.pack_ref.clone(),
                    label,
                    description,
                    action: action.id,
                    action_ref,
                    trigger: trigger.id,
                    trigger_ref,
                    conditions,
                    action_params,
                    trigger_params,
                    trace_tag_template: trace_tag_template.clone(),
                    permission_set_refs,
                    enabled: enabled.unwrap_or(true),
                    is_adhoc: false,
                    owner_identity: None,
                },
            )
            .await
            {
                Ok(rule) => {
                    info!("Created rule '{}' (ID: {})", rule.r#ref, rule.id);
                    result.rules_loaded += 1;
                    loaded_refs.push(rule.r#ref);
                }
                Err(e) => {
                    let msg = format!("Failed to create rule '{}': {}", rule_ref, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    result.rules_skipped += 1;
                }
            }
        }

        Ok(loaded_refs)
    }

    /// Load a workflow definition file referenced by an action's `workflow_file`
    /// field and create/update the corresponding `workflow_definition` record.
    ///
    /// Returns the database ID of the workflow definition.
    async fn load_workflow_for_action(
        &mut self,
        actions_dir: &Path,
        workflow_file_path: &str,
        action_ref: &str,
        action_label: &str,
        action_description: &str,
        action_data: &serde_yaml_ng::Value,
    ) -> Result<Id> {
        let full_path = resolve_workflow_path(actions_dir, workflow_file_path)?;
        if !full_path.exists() {
            return Err(Error::validation(format!(
                "Workflow file '{}' not found at '{}'",
                workflow_file_path,
                full_path.display()
            )));
        }

        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The workflow path is normalized and confined to the pack root before this local read.
        let content = std::fs::read_to_string(&full_path).map_err(|e| {
            Error::io(format!(
                "Failed to read workflow file '{}': {}",
                full_path.display(),
                e
            ))
        })?;

        let mut workflow_yaml = parse_workflow_yaml(&content).map_err(|error| {
            Error::validation(format!(
                "Failed to parse workflow YAML {} (referenced by action '{}'): {error}",
                full_path.display(),
                action_ref
            ))
        })?;

        // The action YAML is authoritative for action-level metadata.
        // Fill in ref/label/description/tags from the action when the
        // workflow file omits them (action-linked workflow files should
        // contain only the execution graph).
        if workflow_yaml.r#ref.is_empty() {
            workflow_yaml.r#ref = action_ref.to_string();
        }
        if workflow_yaml.label.is_empty() {
            workflow_yaml.label = action_label.to_string();
        }
        if workflow_yaml.description.is_none() {
            workflow_yaml.description = Some(action_description.to_string());
        }
        if workflow_yaml.tags.is_empty() {
            if let Some(tags_val) = action_data.get("tags") {
                if let Some(tags_seq) = tags_val.as_sequence() {
                    workflow_yaml.tags = tags_seq
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                }
            }
        }

        let workflow_ref = workflow_yaml.r#ref.clone();
        validate_pack_component_ref(&self.pack_ref, "workflow", &workflow_ref)?;
        for action_ref in collect_workflow_action_refs(&workflow_yaml) {
            let action = ActionRepository::find_by_ref(&mut *self.connection, &action_ref)
                .await?
                .ok_or_else(|| {
                    Error::validation(format!(
                        "Workflow '{}' references unknown action '{}'",
                        workflow_ref, action_ref
                    ))
                })?;
            ensure_action_reference_allowed(
                &action,
                Some(&self.pack_ref),
                "workflow",
                &workflow_ref,
            )?;
        }

        // The action YAML is authoritative for param_schema / out_schema.
        // Fall back to the workflow file's own schemas only if the action
        // YAML doesn't define them.
        let param_schema = action_data
            .get("parameters")
            .and_then(|v| serde_json::to_value(v).ok())
            .or_else(|| workflow_yaml.parameters.clone());

        let out_schema = action_data
            .get("output")
            .and_then(|v| serde_json::to_value(v).ok())
            .or_else(|| workflow_yaml.output.clone());

        let definition_json = serde_json::to_value(&workflow_yaml)
            .map_err(|e| Error::validation(format!("Failed to serialize workflow: {}", e)))?;

        // Derive label/description for the DB record from the action YAML,
        // since it is authoritative. The workflow file values were already
        // used as fallback above when populating workflow_yaml.
        let label = workflow_yaml.label.clone();
        let description = workflow_yaml.description.clone();
        let tags = workflow_yaml.tags.clone();

        // Check if this workflow definition already exists
        if let Some(existing) =
            WorkflowDefinitionRepository::find_by_ref(&mut *self.connection, &workflow_ref).await?
        {
            ensure_existing_owner("workflow", &workflow_ref, Some(existing.pack), self.pack_id)?;
            debug!(
                "Updating existing workflow definition '{}' (ID: {})",
                workflow_ref, existing.id
            );

            let update_input = UpdateWorkflowDefinitionInput {
                label: Some(label),
                description,
                version: Some(workflow_yaml.version.clone()),
                param_schema,
                out_schema,
                definition: Some(definition_json),
                tags: Some(tags),
            };

            WorkflowDefinitionRepository::update(&mut *self.connection, existing.id, update_input)
                .await?;

            info!(
                "Updated workflow definition '{}' (ID: {}) for action '{}'",
                workflow_ref, existing.id, action_ref
            );

            Ok(existing.id)
        } else {
            debug!(
                "Creating new workflow definition '{}' for action '{}'",
                workflow_ref, action_ref
            );

            let create_input = CreateWorkflowDefinitionInput {
                r#ref: workflow_ref.clone(),
                pack: self.pack_id,
                pack_ref: self.pack_ref.clone(),
                label,
                description,
                version: workflow_yaml.version.clone(),
                param_schema,
                out_schema,
                definition: definition_json,
                tags,
            };

            let created =
                WorkflowDefinitionRepository::create(&mut *self.connection, create_input).await?;

            info!(
                "Created workflow definition '{}' (ID: {}) for action '{}'",
                workflow_ref, created.id, action_ref
            );

            Ok(created.id)
        }
    }

    /// Load sensor definitions from `pack_dir/sensors/*.yaml`.
    ///
    /// Returns the list of loaded sensor refs for cleanup.
    async fn load_sensors(
        &mut self,
        pack_dir: &Path,
        trigger_ids: &HashMap<String, Id>,
        result: &mut PackLoadResult,
    ) -> Result<Vec<String>> {
        let sensors_dir = pack_dir.join("sensors");
        let mut loaded_refs = Vec::new();

        if !sensors_dir.exists() {
            info!("No sensors directory found for pack '{}'", self.pack_ref);
            return Ok(loaded_refs);
        }

        let yaml_files = read_yaml_files(&sensors_dir)?;
        info!(
            "Found {} sensor definition(s) for pack '{}'",
            yaml_files.len(),
            self.pack_ref
        );

        for (filename, content) in &yaml_files {
            let data: serde_yaml_ng::Value = serde_yaml_ng::from_str(content).map_err(|e| {
                Error::validation(format!("Failed to parse sensor YAML {}: {}", filename, e))
            })?;

            // Resolve sensor runtime from YAML runner_type field.
            // Defaults to "native" if not specified (compiled binary, no interpreter).
            let runner_type = data
                .get("runner_type")
                .and_then(|v| v.as_str())
                .unwrap_or("native");
            let (sensor_runtime_id, sensor_runtime_ref) = self.resolve_runtime(runner_type).await?;

            // Validate: if the runner_type suggests an interpreted runtime (not native)
            // but we couldn't resolve it, or it resolved to a runtime with no
            // execution_config, warn at registration time rather than failing
            // opaquely at sensor startup with "Permission denied".
            let is_native_runner = matches!(
                runner_type.to_lowercase().as_str(),
                "native" | "builtin" | "standalone"
            );
            if sensor_runtime_id == 0 && !is_native_runner {
                let msg = format!(
                    "Sensor '{}' declares runner_type '{}' but no matching runtime \
                     was found in the database. The sensor will not be able to start. \
                     Ensure the core pack (with runtimes) is loaded before registering \
                     packs that depend on its runtimes.",
                    filename, runner_type
                );
                warn!("{}", msg);
                result.warnings.push(msg);
            } else if sensor_runtime_id != 0 && !is_native_runner {
                // Verify the resolved runtime has a non-empty execution_config
                if let Some(runtime) =
                    RuntimeRepository::find_by_id(&mut *self.connection, sensor_runtime_id).await?
                {
                    let exec_config = runtime.parsed_execution_config();
                    if exec_config.interpreter.binary.is_empty()
                        || exec_config.interpreter.binary == "native"
                        || exec_config.interpreter.binary == "none"
                    {
                        let msg = format!(
                            "Sensor '{}' declares runner_type '{}' (resolved to runtime '{}') \
                             but that runtime has no interpreter configured in its \
                             execution_config. The sensor will fail to start. \
                             Check the runtime definition for '{}'.",
                            filename, runner_type, runtime.r#ref, runtime.r#ref
                        );
                        warn!("{}", msg);
                        result.warnings.push(msg);
                    }
                }
            }

            let sensor_ref = match data.get("ref").and_then(|v| v.as_str()) {
                Some(r) => r.to_string(),
                None => {
                    let msg = format!("Sensor YAML {} missing 'ref' field, skipping", filename);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                    continue;
                }
            };

            let name = extract_name_from_ref(&sensor_ref);
            let label = data
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| generate_label(&name));

            let description = data
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let enabled = data.get("enabled").and_then(|v| v.as_bool());

            let entrypoint = data
                .get("entry_point")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Resolve trigger reference(s) for post-load linkage
            let sensor_triggers = self.resolve_sensor_triggers(&data, trigger_ids).await;

            let param_schema = data
                .get("parameters")
                .and_then(|v| serde_json::to_value(v).ok());

            let config = data
                .get("config")
                .and_then(|v| serde_json::to_value(v).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            // Optional runtime version constraint (e.g., ">=3.12", "~18.0")
            let runtime_version_constraint = data
                .get("runtime_version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let worker_selector = data
                .get("worker_selector")
                .map(|value| serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!({})))
                .unwrap_or_else(|| serde_json::json!({}));
            let worker_tolerations = data
                .get("worker_tolerations")
                .map(|value| serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!([])))
                .unwrap_or_else(|| serde_json::json!([]));
            let worker_affinity = data
                .get("worker_affinity")
                .map(|value| serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!({})))
                .unwrap_or_else(|| serde_json::json!({}));
            let log_retention_policy =
                parse_log_retention_policy(data.get("log_retention_policy"))?;
            let log_retention_limit = parse_log_retention_limit(data.get("log_retention_limit"))?;
            let artifact_retention_policy =
                parse_log_retention_policy(data.get("artifact_retention_policy"))?;
            let artifact_retention_limit =
                parse_log_retention_limit(data.get("artifact_retention_limit"))?;

            // Upsert: update existing sensors so re-registration corrects
            // stale metadata (especially runtime assignments).
            if let Some(existing) =
                SensorRepository::find_by_ref(&mut *self.connection, &sensor_ref).await?
            {
                ensure_existing_owner("sensor", &sensor_ref, existing.pack, self.pack_id)?;
                let update_input = UpdateSensorInput {
                    label: Some(label),
                    description: Some(match description {
                        Some(description) => Patch::Set(description),
                        None => Patch::Clear,
                    }),
                    entrypoint: Some(entrypoint),
                    runtime: Some(sensor_runtime_id),
                    runtime_ref: Some(sensor_runtime_ref.clone()),
                    runtime_version_constraint: Some(match runtime_version_constraint.clone() {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    enabled,
                    param_schema: Some(match param_schema {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    config: Some(config),
                    worker_selector: Some(worker_selector.clone()),
                    worker_tolerations: Some(worker_tolerations.clone()),
                    worker_affinity: Some(worker_affinity.clone()),
                    artifact_retention_policy: Some(match artifact_retention_policy {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    artifact_retention_limit: Some(match artifact_retention_limit {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    log_retention_policy: Some(match log_retention_policy {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                    log_retention_limit: Some(match log_retention_limit {
                        Some(value) => Patch::Set(value),
                        None => Patch::Clear,
                    }),
                };

                match SensorRepository::update(&mut *self.connection, existing.id, update_input)
                    .await
                {
                    Ok(_) => {
                        info!(
                            "Updated sensor '{}' (ID: {}, runtime: {} → {})",
                            sensor_ref, existing.id, existing.runtime_ref, sensor_runtime_ref
                        );
                        self.link_triggers_to_sensor(existing.id, &sensor_ref, &sensor_triggers)
                            .await;
                        result.sensors_updated += 1;
                    }
                    Err(e) => {
                        let msg = format!("Failed to update sensor '{}': {}", sensor_ref, e);
                        warn!("{}", msg);
                        result.warnings.push(msg);
                    }
                }
                loaded_refs.push(sensor_ref);
                continue;
            }

            let input = CreateSensorInput {
                r#ref: sensor_ref.clone(),
                pack: Some(self.pack_id),
                pack_ref: Some(self.pack_ref.clone()),
                label,
                description,
                entrypoint,
                runtime: sensor_runtime_id,
                runtime_ref: sensor_runtime_ref.clone(),
                runtime_version_constraint,
                enabled: enabled.unwrap_or(true),
                param_schema,
                config: Some(config),
                worker_selector,
                worker_tolerations,
                worker_affinity,
                artifact_retention_policy,
                artifact_retention_limit,
                log_retention_policy,
                log_retention_limit,
            };

            match SensorRepository::create(&mut *self.connection, input).await {
                Ok(sensor) => {
                    info!("Created sensor '{}' (ID: {})", sensor_ref, sensor.id);
                    self.link_triggers_to_sensor(sensor.id, &sensor_ref, &sensor_triggers)
                        .await;
                    loaded_refs.push(sensor_ref);
                    result.sensors_loaded += 1;
                }
                Err(e) => {
                    let msg = format!("Failed to create sensor '{}': {}", sensor_ref, e);
                    warn!("{}", msg);
                    result.warnings.push(msg);
                }
            }
        }

        Ok(loaded_refs)
    }

    /// Load declarative cache namespace definitions from
    /// `pack_dir/caches/*.yaml`.
    ///
    /// Definitions are applied atomically. Existing live definitions preserve
    /// their namespace IDs and generations; only policy fields are mutable.
    async fn load_caches(
        &mut self,
        pack_dir: &Path,
        action_refs: &[String],
        sensor_refs: &[String],
        result: &mut PackLoadResult,
    ) -> Result<Vec<String>> {
        let caches_dir = pack_dir.join("caches");
        if !caches_dir.exists() {
            info!("No caches directory found for pack '{}'", self.pack_ref);
            return Ok(Vec::new());
        }

        let yaml_files = read_yaml_files(&caches_dir)?;
        info!(
            "Found {} cache definition(s) for pack '{}'",
            yaml_files.len(),
            self.pack_ref
        );

        let current_actions: HashSet<&str> = action_refs.iter().map(String::as_str).collect();
        let current_sensors: HashSet<&str> = sensor_refs.iter().map(String::as_str).collect();
        let mut seen_refs = HashSet::new();
        let mut definitions = Vec::new();
        let mut loaded_refs = Vec::new();

        for (filename, content) in &yaml_files {
            let definition: CacheDefinitionYaml =
                serde_yaml_ng::from_str(content).map_err(|e| {
                    Error::validation(format!("Failed to parse cache YAML {}: {}", filename, e))
                })?;
            definition.validate()?;

            validate_pack_component_ref(&self.pack_ref, "cache definition", &definition.r#ref)?;
            if !seen_refs.insert(definition.r#ref.clone()) {
                return Err(Error::validation(format!(
                    "Duplicate cache definition ref '{}'",
                    definition.r#ref
                )));
            }

            let owner = match definition.owner_type {
                CacheDefinitionOwnerType::Pack => {
                    if definition.owner_ref != self.pack_ref {
                        return Err(Error::validation(format!(
                            "Cache definition '{}' must use installing pack '{}' as its pack owner",
                            definition.r#ref, self.pack_ref
                        )));
                    }
                    CacheOwnerScope::pack(self.pack_id, Some(self.pack_ref.clone()))
                }
                CacheDefinitionOwnerType::Action => {
                    validate_pack_component_ref(
                        &self.pack_ref,
                        "cache action owner",
                        &definition.owner_ref,
                    )?;
                    if !current_actions.contains(definition.owner_ref.as_str()) {
                        let msg = format!(
                            "Cache definition '{}' references action owner '{}' that is not \
                             present in the installing pack; skipping the definition",
                            definition.r#ref, definition.owner_ref
                        );
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.caches_skipped += 1;
                        continue;
                    }
                    let action =
                        ActionRepository::find_by_ref(&mut *self.connection, &definition.owner_ref)
                            .await?
                            .ok_or_else(|| {
                                Error::validation(format!(
                                    "Cache definition '{}' action owner '{}' was not loaded",
                                    definition.r#ref, definition.owner_ref
                                ))
                            })?;
                    if action.pack != self.pack_id {
                        return Err(Error::validation(format!(
                            "Cache definition '{}' action owner '{}' belongs to another pack",
                            definition.r#ref, definition.owner_ref
                        )));
                    }
                    CacheOwnerScope::action(action.id, Some(action.r#ref))
                }
                CacheDefinitionOwnerType::Sensor => {
                    validate_pack_component_ref(
                        &self.pack_ref,
                        "cache sensor owner",
                        &definition.owner_ref,
                    )?;
                    if !current_sensors.contains(definition.owner_ref.as_str()) {
                        let msg = format!(
                            "Cache definition '{}' references sensor owner '{}' that is not \
                             present in the installing pack; skipping the definition",
                            definition.r#ref, definition.owner_ref
                        );
                        warn!("{}", msg);
                        result.warnings.push(msg);
                        result.caches_skipped += 1;
                        continue;
                    }
                    let sensor =
                        SensorRepository::find_by_ref(&mut *self.connection, &definition.owner_ref)
                            .await?
                            .ok_or_else(|| {
                                Error::validation(format!(
                                    "Cache definition '{}' sensor owner '{}' was not loaded",
                                    definition.r#ref, definition.owner_ref
                                ))
                            })?;
                    if sensor.pack != Some(self.pack_id) {
                        return Err(Error::validation(format!(
                            "Cache definition '{}' sensor owner '{}' belongs to another pack",
                            definition.r#ref, definition.owner_ref
                        )));
                    }
                    CacheOwnerScope::sensor(sensor.id, Some(sensor.r#ref))
                }
            };

            loaded_refs.push(definition.r#ref.clone());
            let policy = definition.policy();
            definitions.push(ManagedCacheNamespaceDefinition {
                definition_ref: definition.r#ref,
                owner,
                namespace: definition.namespace,
                policy,
            });
        }

        let summary = CacheNamespaceRepository::upsert_managed_definitions_in_transaction(
            &mut *self.connection,
            self.pack_id,
            &self.pack_ref,
            &definitions,
            &self.cache_admission,
        )
        .await?;
        result.caches_loaded += summary.created;
        result.caches_updated += summary.updated;
        result.caches_skipped += summary.unchanged;

        info!(
            "Loaded cache definitions for pack '{}': {} created, {} updated, {} unchanged",
            self.pack_ref, summary.created, summary.updated, summary.unchanged
        );
        Ok(loaded_refs)
    }

    /// Resolve a runtime ID from a runner type string (e.g., "shell", "python", "native").
    ///
    /// Looks up the runtime in the database by `core.{name}` ref pattern,
    /// then falls back to name-based lookup (case-insensitive).
    ///
    /// - "shell" -> "core.shell"
    /// - "python" -> "core.python"
    /// - "node"  -> "core.nodejs"
    /// - "native" -> "core.native"
    async fn resolve_runtime_id(&mut self, runner_type: &str) -> Result<Option<Id>> {
        let (id, _ref) = self.resolve_runtime(runner_type).await?;
        if id == 0 {
            Ok(None)
        } else {
            Ok(Some(id))
        }
    }

    /// Map a runner_type string to a (runtime_id, runtime_ref) pair.
    ///
    /// Returns `(0, "unknown")` when no matching runtime is found.
    async fn resolve_runtime(&mut self, runner_type: &str) -> Result<(Id, String)> {
        let runner_lower = runner_type.to_lowercase();

        // Runtime refs use the format `{pack_ref}.{name}` (e.g., "core.python").
        let refs_to_try = match runner_lower.as_str() {
            "shell" | "bash" | "sh" => vec!["core.shell"],
            "python" | "python3" => vec!["core.python"],
            "node" | "nodejs" | "node.js" => vec!["core.nodejs"],
            "native" | "builtin" | "standalone" => vec!["core.native"],
            other => vec![other],
        };

        for runtime_ref in &refs_to_try {
            if let Some(runtime) =
                RuntimeRepository::find_by_ref(&mut *self.connection, runtime_ref).await?
            {
                return Ok((runtime.id, runtime.r#ref));
            }
        }

        // Fall back to name-based lookup (case-insensitive)
        use crate::repositories::runtime::RuntimeRepository as RR;
        if let Some(runtime) = RR::find_by_name(&mut *self.connection, &runner_lower).await? {
            return Ok((runtime.id, runtime.r#ref));
        }

        warn!(
            "Could not find runtime for runner_type '{}', component will have no runtime",
            runner_type
        );
        Ok((0, "unknown".to_string()))
    }

    /// Resolve all trigger references for a sensor from its YAML `trigger_types`/`trigger_type` field.
    ///
    /// Returns a list of (trigger_id, trigger_ref) pairs for all triggers this sensor emits.
    async fn resolve_sensor_triggers(
        &mut self,
        data: &serde_yaml_ng::Value,
        trigger_ids: &HashMap<String, Id>,
    ) -> Vec<(Option<Id>, String)> {
        // Collect all trigger type strings
        let trigger_type_strs: Vec<String> =
            if let Some(seq) = data.get("trigger_types").and_then(|v| v.as_sequence()) {
                seq.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            } else if let Some(t) = data.get("trigger_type").and_then(|v| v.as_str()) {
                vec![t.to_string()]
            } else {
                return vec![];
            };

        let mut results = Vec::new();
        for t in trigger_type_strs {
            let trigger_ref = if t.contains('.') {
                t
            } else {
                format!("{}.{}", self.pack_ref, t)
            };

            // Look up trigger ID from our loaded triggers map first
            if let Some(&id) = trigger_ids.get(&trigger_ref) {
                results.push((Some(id), trigger_ref));
                continue;
            }

            // Fall back to database lookup
            match TriggerRepository::find_by_ref(&mut *self.connection, &trigger_ref).await {
                Ok(Some(trigger)) => results.push((Some(trigger.id), trigger_ref)),
                _ => {
                    warn!("Could not resolve trigger ref '{}' for sensor", trigger_ref);
                    results.push((None, trigger_ref));
                }
            }
        }

        results
    }

    /// After a sensor is created/updated, update its triggers to point back to it.
    async fn link_triggers_to_sensor(
        &mut self,
        sensor_id: Id,
        sensor_ref: &str,
        trigger_refs: &[(Option<Id>, String)],
    ) {
        for (trigger_id_opt, trigger_ref) in trigger_refs {
            let trigger_id = match trigger_id_opt {
                Some(id) => *id,
                None => {
                    warn!(
                        "Skipping trigger linkage for unresolved trigger '{}'",
                        trigger_ref
                    );
                    continue;
                }
            };

            match TriggerRepository::find_by_id(&mut *self.connection, trigger_id).await {
                Ok(Some(trigger)) if trigger.pack == Some(self.pack_id) => {}
                Ok(_) => {
                    warn!(
                        "Skipping trigger linkage for '{}' because it is not owned by pack '{}'",
                        trigger_ref, self.pack_ref
                    );
                    continue;
                }
                Err(error) => {
                    warn!(
                        "Failed to verify trigger '{}' ownership: {}",
                        trigger_ref, error
                    );
                    continue;
                }
            }

            let update_input = UpdateTriggerInput {
                sensor: Some(Patch::Set(sensor_id)),
                sensor_ref: Some(Patch::Set(sensor_ref.to_string())),
                ..Default::default()
            };

            match TriggerRepository::update(&mut *self.connection, trigger_id, update_input).await {
                Ok(_) => {
                    info!("Linked trigger '{}' → sensor '{}'", trigger_ref, sensor_ref);
                }
                Err(e) => {
                    warn!(
                        "Failed to link trigger '{}' → sensor '{}': {}",
                        trigger_ref, sensor_ref, e
                    );
                }
            }
        }
    }

    /// Remove entities that belong to this pack but whose refs are no longer
    /// present in the pack's YAML files.
    ///
    /// This handles the case where an action/trigger/sensor/runtime was removed
    /// from the pack between versions. Ad-hoc (user-created) entities are never
    /// removed.
    async fn cleanup_removed_entities(
        &mut self,
        refs: CleanupRefs<'_>,
        result: &mut PackLoadResult,
    ) {
        match PermissionSetRepository::delete_by_pack_excluding(
            &mut *self.connection,
            self.pack_id,
            refs.permission_sets,
        )
        .await
        {
            Ok(count) => {
                if count > 0 {
                    info!(
                        "Removed {} stale permission set(s) from pack '{}'",
                        count, self.pack_ref
                    );
                    result.removed += count as usize;
                }
            }
            Err(e) => {
                warn!(
                    "Failed to clean up stale permission sets for pack '{}': {}",
                    self.pack_ref, e
                );
            }
        }

        match CacheNamespaceRepository::tombstone_managed_by_pack_excluding_in_transaction(
            &mut *self.connection,
            self.pack_id,
            refs.caches,
        )
        .await
        {
            Ok(count) => {
                if count > 0 {
                    info!(
                        "Tombstoned {} stale cache definition(s) from pack '{}'",
                        count, self.pack_ref
                    );
                    result.removed += count as usize;
                }
            }
            Err(e) => {
                let msg = format!(
                    "Failed to tombstone stale cache definitions for pack '{}': {}",
                    self.pack_ref, e
                );
                warn!("{}", msg);
                result.warnings.push(msg);
            }
        }

        // Clean up queues before actions for consistency with load order; action deletion would
        // null out queue dispatch_action references via ON DELETE SET NULL, so either order works.
        match DashboardRepository::delete_non_adhoc_by_pack_excluding(
            &mut *self.connection,
            self.pack_id,
            refs.dashboards,
        )
        .await
        {
            Ok(count) => {
                if count > 0 {
                    info!(
                        "Removed {} stale dashboard(s) from pack '{}'",
                        count, self.pack_ref
                    );
                    result.removed += count as usize;
                }
            }
            Err(e) => {
                warn!(
                    "Failed to clean up stale dashboards for pack '{}': {}",
                    self.pack_ref, e
                );
            }
        }

        // Clean up queues before actions for consistency with load order; action deletion would
        // null out queue dispatch_action references via ON DELETE SET NULL, so either order works.
        match WorkQueueRepository::delete_non_adhoc_by_pack_excluding(
            &mut *self.connection,
            self.pack_id,
            refs.queues,
        )
        .await
        {
            Ok(count) => {
                if count > 0 {
                    info!(
                        "Removed {} stale work queue(s) from pack '{}'",
                        count, self.pack_ref
                    );
                    result.removed += count as usize;
                }
            }
            Err(e) => {
                warn!(
                    "Failed to clean up stale work queues for pack '{}': {}",
                    self.pack_ref, e
                );
            }
        }

        match PolicyRepository::delete_by_pack_excluding(
            &mut *self.connection,
            self.pack_id,
            refs.policies,
        )
        .await
        {
            Ok(count) => {
                if count > 0 {
                    info!(
                        "Removed {} stale policy(s) from pack '{}'",
                        count, self.pack_ref
                    );
                    result.removed += count as usize;
                }
            }
            Err(e) => {
                warn!(
                    "Failed to clean up stale policies for pack '{}': {}",
                    self.pack_ref, e
                );
            }
        }

        // Clean up rules before actions/triggers; rule FKs use ON DELETE SET NULL,
        // but deleting stale declarative rules first preserves clear pack semantics.
        match RuleRepository::delete_by_pack_excluding(
            &mut *self.connection,
            self.pack_id,
            refs.rules,
        )
        .await
        {
            Ok(count) => {
                if count > 0 {
                    info!(
                        "Removed {} stale rule(s) from pack '{}'",
                        count, self.pack_ref
                    );
                    result.removed += count as usize;
                }
            }
            Err(e) => {
                warn!(
                    "Failed to clean up stale rules for pack '{}': {}",
                    self.pack_ref, e
                );
            }
        }

        // Clean up stale cache owners after their dependents, but before
        // triggers/runtimes. Cache tombstoning and owner deletion are one
        // repository transaction so a lifecycle failure cannot partially
        // delete actions or sensors.
        match CacheNamespaceRepository::delete_removed_pack_owners_in_transaction(
            &mut *self.connection,
            self.pack_id,
            refs.actions,
            refs.sensors,
        )
        .await
        {
            Ok(summary) => {
                if summary.tombstoned_namespaces > 0 {
                    info!(
                        "Tombstoned {} cache namespace(s) whose pack owner was removed from '{}'",
                        summary.tombstoned_namespaces, self.pack_ref
                    );
                    result.removed += summary.tombstoned_namespaces as usize;
                }
                if summary.deleted_sensors > 0 {
                    info!(
                        "Removed {} stale sensor(s) from pack '{}'",
                        summary.deleted_sensors, self.pack_ref
                    );
                    result.removed += summary.deleted_sensors as usize;
                }
                if summary.deleted_actions > 0 {
                    info!(
                        "Removed {} stale action(s) from pack '{}'",
                        summary.deleted_actions, self.pack_ref
                    );
                    result.removed += summary.deleted_actions as usize;
                }
            }
            Err(e) => {
                let msg = format!(
                    "Failed to atomically clean up stale cache owners for pack '{}': {}",
                    self.pack_ref, e
                );
                warn!("{}", msg);
                result.warnings.push(msg);
            }
        }

        // Clean up triggers (ad-hoc preserved)
        match TriggerRepository::delete_non_adhoc_by_pack_excluding(
            &mut *self.connection,
            self.pack_id,
            refs.triggers,
        )
        .await
        {
            Ok(count) => {
                if count > 0 {
                    info!(
                        "Removed {} stale trigger(s) from pack '{}'",
                        count, self.pack_ref
                    );
                    result.removed += count as usize;
                }
            }
            Err(e) => {
                warn!(
                    "Failed to clean up stale triggers for pack '{}': {}",
                    self.pack_ref, e
                );
            }
        }

        // Clean up runtimes last (actions/sensors may reference them)
        match RuntimeRepository::delete_by_pack_excluding(
            &mut *self.connection,
            self.pack_id,
            refs.runtimes,
        )
        .await
        {
            Ok(count) => {
                if count > 0 {
                    info!(
                        "Removed {} stale runtime(s) from pack '{}'",
                        count, self.pack_ref
                    );
                    result.removed += count as usize;
                }
            }
            Err(e) => {
                warn!(
                    "Failed to clean up stale runtimes for pack '{}': {}",
                    self.pack_ref, e
                );
            }
        }
    }
}

fn resolve_workflow_path(actions_dir: &Path, workflow_file: &str) -> Result<PathBuf> {
    let relative = Path::new(workflow_file); // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The path is parsed only for component validation before canonical confinement beneath actions/workflows.
    if relative.is_absolute()
        || !matches!(relative.components().next(), Some(std::path::Component::Normal(part)) if part == "workflows")
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(Error::validation(format!(
            "Workflow file '{}' must be relative to actions/ and located under actions/workflows/",
            workflow_file
        )));
    }

    let canonical_actions_dir = actions_dir.canonicalize().map_err(|e| {
        Error::io(format!(
            "Failed to resolve actions directory '{}': {}",
            actions_dir.display(),
            e
        ))
    })?;
    let workflows_dir = actions_dir.join("workflows");
    let canonical_workflows_dir = workflows_dir.canonicalize().map_err(|e| {
        Error::io(format!(
            "Failed to resolve workflows directory '{}': {}",
            workflows_dir.display(),
            e
        ))
    })?;
    if !canonical_workflows_dir.starts_with(&canonical_actions_dir) {
        return Err(Error::validation(format!(
            "Workflow directory '{}' escapes actions directory '{}'",
            canonical_workflows_dir.display(),
            canonical_actions_dir.display()
        )));
    }

    let candidate = actions_dir.join(relative);
    let canonical_candidate = candidate.canonicalize().map_err(|e| {
        Error::io(format!(
            "Failed to resolve workflow file '{}': {}",
            candidate.display(),
            e
        ))
    })?;
    if !canonical_candidate.starts_with(&canonical_workflows_dir) {
        return Err(Error::validation(format!(
            "Workflow file '{}' escapes canonical actions/workflows directory",
            workflow_file
        )));
    }

    Ok(canonical_candidate)
}

/// Read all YAML files from a directory, returning `(filename, content)` pairs
/// sorted by filename for deterministic ordering.
fn read_yaml_files(dir: &Path) -> Result<Vec<(String, String)>> {
    let mut files = Vec::new();

    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Pack loader scans pack-owned directories on disk after selecting the pack root.
    let entries = std::fs::read_dir(dir)
        .map_err(|e| Error::io(format!("Failed to read directory {}: {}", dir.display(), e)))?;

    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            path.is_file()
                && matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("yaml") | Some("yml")
                )
        })
        .collect();

    // Sort by filename for deterministic ordering
    paths.sort_by_key(|e| e.file_name());

    for entry in paths {
        let path = entry.path();
        let filename = entry.file_name().to_string_lossy().to_string();

        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- YAML files are read only after being discovered under the selected pack directory.
        let content = std::fs::read_to_string(&path)
            .map_err(|e| Error::io(format!("Failed to read file {}: {}", path.display(), e)))?;

        files.push((filename, content));
    }

    Ok(files)
}

/// Extract the short name from a dotted ref (e.g., "core.echo" -> "echo").
fn extract_name_from_ref(r: &str) -> String {
    r.rsplit('.').next().unwrap_or(r).to_string()
}

/// Expand short refs to this pack's namespace while preserving already-qualified refs.
fn qualify_pack_ref(pack_ref: &str, r: &str) -> String {
    if r.contains('.') {
        r.to_string()
    } else {
        format!("{pack_ref}.{r}")
    }
}

fn parse_log_retention_policy(
    value: Option<&serde_yaml_ng::Value>,
) -> Result<Option<RetentionPolicyType>> {
    value
        .map(|value| {
            let json = serde_json::to_value(value).map_err(|e| {
                Error::validation(format!("Invalid log_retention_policy value: {}", e))
            })?;
            serde_json::from_value(json).map_err(|e| {
                Error::validation(format!("Invalid log_retention_policy value: {}", e))
            })
        })
        .transpose()
}

fn yaml_string_array(value: Option<&serde_yaml_ng::Value>) -> Vec<String> {
    value
        .and_then(|value| value.as_sequence())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_log_retention_limit(value: Option<&serde_yaml_ng::Value>) -> Result<Option<i32>> {
    value
        .map(|value| {
            let json = serde_json::to_value(value).map_err(|e| {
                Error::validation(format!("Invalid log_retention_limit value: {}", e))
            })?;
            serde_json::from_value(json)
                .map_err(|e| Error::validation(format!("Invalid log_retention_limit value: {}", e)))
        })
        .transpose()
}

/// Parse an optional `timeout_seconds` action field. Accepts a positive integer
/// number of seconds. Returns `None` when the field is absent.
fn parse_timeout_seconds(value: Option<&serde_yaml_ng::Value>) -> Result<Option<i32>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let json = serde_json::to_value(value)
        .map_err(|e| Error::validation(format!("Invalid timeout_seconds value: {}", e)))?;
    let parsed: i32 = serde_json::from_value(json)
        .map_err(|e| Error::validation(format!("Invalid timeout_seconds value: {}", e)))?;
    if parsed <= 0 {
        return Err(Error::validation(
            "timeout_seconds must be greater than zero",
        ));
    }
    Ok(Some(parsed))
}

fn parse_action_reference_visibility(
    value: Option<&serde_yaml_ng::Value>,
) -> Result<ActionReferenceVisibility> {
    let Some(value) = value else {
        return Ok(ActionReferenceVisibility::Public);
    };
    let Some(raw) = value.as_str() else {
        return Err(Error::validation(
            "reference_visibility must be one of public, private, or restricted",
        ));
    };
    match raw.trim().to_lowercase().as_str() {
        "public" => Ok(ActionReferenceVisibility::Public),
        "private" => Ok(ActionReferenceVisibility::Private),
        "restricted" => Ok(ActionReferenceVisibility::Restricted),
        other => Err(Error::validation(format!(
            "invalid reference_visibility '{}'; expected public, private, or restricted",
            other
        ))),
    }
}

fn parse_dashboard_scope_type(raw: &str) -> std::result::Result<DashboardScopeType, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "global" => Ok(DashboardScopeType::Global),
        "pack" => Ok(DashboardScopeType::Pack),
        "identity" => Ok(DashboardScopeType::Identity),
        "tenant" => Ok(DashboardScopeType::Tenant),
        other => Err(format!(
            "invalid dashboard scope_type '{}'; expected global, pack, identity, or tenant",
            other
        )),
    }
}

fn parse_dashboard_visibility(raw: &str) -> std::result::Result<DashboardVisibility, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "private" => Ok(DashboardVisibility::Private),
        "pack" => Ok(DashboardVisibility::Pack),
        "public" => Ok(DashboardVisibility::Public),
        other => Err(format!(
            "invalid dashboard visibility '{}'; expected private, pack, or public",
            other
        )),
    }
}

fn parse_reference_allowed_pack_refs(value: Option<&serde_yaml_ng::Value>) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(sequence) = value.as_sequence() else {
        return Err(Error::validation(
            "reference_allowed_pack_refs must be an array of pack refs",
        ));
    };

    sequence
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    Error::validation(
                        "reference_allowed_pack_refs entries must be non-empty strings",
                    )
                })
        })
        .collect()
}

fn parse_optional_permission_set_refs(
    value: Option<&serde_yaml_ng::Value>,
) -> Result<Option<Vec<String>>> {
    let Some(value) = value else {
        return Ok(None);
    };

    let refs = match value {
        serde_yaml_ng::Value::String(permission_set_ref) => {
            vec![permission_set_ref.trim().to_string()]
        }
        serde_yaml_ng::Value::Sequence(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| Error::validation("permission_set_refs entries must be strings"))
            })
            .collect::<Result<Vec<_>>>()?,
        serde_yaml_ng::Value::Null => return Ok(None),
        _ => {
            return Err(Error::validation(
                "permission_set_refs must be a string, an array of strings, or null",
            ))
        }
    };

    if refs
        .iter()
        .any(|permission_set_ref| permission_set_ref.is_empty())
    {
        return Err(Error::validation(
            "permission_set_refs cannot contain empty refs",
        ));
    }

    Ok(Some(refs))
}

/// Generate a human-readable label from a snake_case name.
///
/// Examples:
/// - "echo" -> "Echo"
/// - "http_request" -> "Http Request"
/// - "datetime_timer" -> "Datetime Timer"
fn generate_label(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    format!("{}{}", upper, chars.as_str())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_pack_component_ref(pack_ref: &str, field: &str, value: &str) -> Result<()> {
    RefValidator::validate_component_ref(value)
        .map_err(|error| Error::validation(format!("Invalid {field} ref '{value}': {error}")))?;
    let prefix = format!("{pack_ref}.");
    if !value.starts_with(&prefix) {
        return Err(Error::validation(format!(
            "{field} ref '{value}' must belong to installing pack '{pack_ref}'"
        )));
    }
    Ok(())
}

fn ensure_existing_owner(
    kind: &str,
    component_ref: &str,
    owner: Option<Id>,
    installing_pack: Id,
) -> Result<()> {
    if owner != Some(installing_pack) {
        let reserved = component_ref == "standard";
        return Err(Error::validation(format!(
            "Cannot overwrite {}{} '{}' owned by another pack",
            if reserved { "reserved " } else { "" },
            kind,
            component_ref
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_name_from_ref() {
        assert_eq!(extract_name_from_ref("core.echo"), "echo");
        assert_eq!(extract_name_from_ref("python_example.greet"), "greet");
        assert_eq!(extract_name_from_ref("simple"), "simple");
        assert_eq!(extract_name_from_ref("a.b.c"), "c");
    }

    #[test]
    fn test_generate_label() {
        assert_eq!(generate_label("echo"), "Echo");
        assert_eq!(generate_label("http_request"), "Http Request");
        assert_eq!(generate_label("datetime_timer"), "Datetime Timer");
        assert_eq!(generate_label("a_b_c"), "A B C");
    }

    #[test]
    fn test_parse_optional_permission_set_refs() {
        assert_eq!(parse_optional_permission_set_refs(None).unwrap(), None);

        let single = serde_yaml_ng::Value::String(" standard ".to_string());
        assert_eq!(
            parse_optional_permission_set_refs(Some(&single)).unwrap(),
            Some(vec!["standard".to_string()])
        );

        let many: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("- standard\n- elevated\n").unwrap();
        assert_eq!(
            parse_optional_permission_set_refs(Some(&many)).unwrap(),
            Some(vec!["standard".to_string(), "elevated".to_string()])
        );

        let null = serde_yaml_ng::Value::Null;
        assert_eq!(
            parse_optional_permission_set_refs(Some(&null)).unwrap(),
            None
        );
    }

    #[test]
    fn test_parse_optional_permission_set_refs_rejects_invalid_values() {
        let empty = serde_yaml_ng::Value::String(" ".to_string());
        assert!(parse_optional_permission_set_refs(Some(&empty)).is_err());

        let non_string_entry: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("- standard\n- 42\n").unwrap();
        assert!(parse_optional_permission_set_refs(Some(&non_string_entry)).is_err());

        let invalid = serde_yaml_ng::Value::Bool(true);
        assert!(parse_optional_permission_set_refs(Some(&invalid)).is_err());
    }

    #[test]
    fn cache_definition_yaml_uses_flat_policy_defaults() {
        let definition: CacheDefinitionYaml = serde_yaml_ng::from_str(
            r#"
ref: demo.inventory
namespace: inventory
owner_type: pack
owner_ref: demo
"#,
        )
        .unwrap();

        assert_eq!(definition.r#ref, "demo.inventory");
        assert_eq!(definition.namespace, "inventory");
        assert!(matches!(
            definition.owner_type,
            CacheDefinitionOwnerType::Pack
        ));
        assert_eq!(
            definition.policy(),
            crate::repositories::cache::CacheNamespacePolicy::default()
        );
    }

    #[test]
    fn cache_definition_yaml_supports_action_and_sensor_owners() {
        for (owner_type, expected) in [
            ("action", CacheDefinitionOwnerType::Action),
            ("sensor", CacheDefinitionOwnerType::Sensor),
        ] {
            let yaml = format!(
                "ref: demo.inventory\nnamespace: inventory\nowner_type: {owner_type}\n\
                 owner_ref: demo.refresh\nfreshness_target_seconds: 60\n\
                 max_retained_generations: 2\n"
            );
            let definition: CacheDefinitionYaml = serde_yaml_ng::from_str(&yaml).unwrap();
            assert_eq!(definition.owner_type, expected);
            assert_eq!(definition.freshness_target_seconds, 60);
            assert_eq!(definition.max_retained_generations, 2);
        }
    }

    #[test]
    fn cache_definition_yaml_rejects_unsupported_owners_and_nested_policy() {
        for owner_type in ["system", "identity"] {
            let yaml = format!(
                "ref: demo.inventory\nnamespace: inventory\nowner_type: {owner_type}\n\
                 owner_ref: demo\n"
            );
            assert!(serde_yaml_ng::from_str::<CacheDefinitionYaml>(&yaml).is_err());
        }

        let nested_policy = r#"
ref: demo.inventory
namespace: inventory
owner_type: pack
owner_ref: demo
policy:
  freshness_target_seconds: 60
"#;
        assert!(serde_yaml_ng::from_str::<CacheDefinitionYaml>(nested_policy).is_err());
    }

    #[test]
    fn cache_definition_refs_reject_cross_pack_owners() {
        assert!(validate_pack_component_ref("demo", "cache definition", "demo.inventory").is_ok());
        assert!(
            validate_pack_component_ref("demo", "cache action owner", "other.refresh").is_err()
        );
        assert!(validate_pack_component_ref("demo", "cache sensor owner", "demo.Refresh").is_err());
    }

    #[test]
    fn reserved_standard_permission_set_cannot_be_claimed() {
        let error = ensure_existing_owner("permission set", "standard", None, 42).unwrap_err();
        assert!(error
            .to_string()
            .contains("reserved permission set 'standard'"));
    }

    #[test]
    fn cross_pack_component_collision_is_rejected() {
        assert!(ensure_existing_owner("action", "demo.run", Some(41), 42).is_err());
        assert!(ensure_existing_owner("action", "demo.run", Some(42), 42).is_ok());
        assert!(validate_pack_component_ref("demo", "action", "other.run").is_err());
    }

    #[test]
    fn workflow_path_is_confined_to_actions_workflows() {
        let temp = tempfile::tempdir().unwrap();
        let actions_dir = temp.path().join("actions");
        let workflows_dir = actions_dir.join("workflows");
        std::fs::create_dir_all(&workflows_dir).unwrap();
        let workflow = workflows_dir.join("deploy.workflow.yaml");
        std::fs::write(&workflow, "version: 1.0.0\ntasks: []\n").unwrap();

        assert_eq!(
            resolve_workflow_path(&actions_dir, "workflows/deploy.workflow.yaml").unwrap(),
            workflow.canonicalize().unwrap()
        );
        assert!(resolve_workflow_path(&actions_dir, "../deploy.workflow.yaml").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn workflow_path_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let actions_dir = temp.path().join("actions");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&actions_dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("deploy.workflow.yaml"), "version: 1.0.0\n").unwrap();
        symlink(&outside, actions_dir.join("workflows")).unwrap();

        assert!(resolve_workflow_path(&actions_dir, "workflows/deploy.workflow.yaml").is_err());
    }
}
