//! Workflow Registrar
//!
//! This module handles registering workflows as workflow definitions in the database.
//! Workflows are stored in the `workflow_definition` table with their full YAML definition
//! as JSON. A companion action record is also created so that workflows appear in
//! action lists and the workflow builder's action palette.
//!
//! Standalone workflow files (in `workflows/`) carry their own `ref` and `label`.
//! Action-linked workflow files (in `actions/workflows/`, referenced via
//! `workflow_file`) may omit those fields — the registrar falls back to
//! `WorkflowFile.ref_name` / `WorkflowFile.name` derived from the filename.

use crate::error::{Error, Result};
use crate::metadata_cache::{repositories::CachedMetadataRepository, MetadataCache};
use crate::models::ActionReferenceVisibility;
use crate::repositories::action::{ActionRepository, CreateActionInput, UpdateActionInput};
use crate::repositories::workflow::{CreateWorkflowDefinitionInput, UpdateWorkflowDefinitionInput};
use crate::repositories::Patch;
use crate::repositories::{
    Create, Delete, FindById, FindByRef, PackRepository, Update, WorkflowDefinitionRepository,
};
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::{debug, info, warn};

use super::loader::LoadedWorkflow;
use super::parser::WorkflowDefinition as WorkflowYaml;

/// Options for workflow registration
#[derive(Debug, Clone)]
pub struct RegistrationOptions {
    /// Whether to update existing workflows
    pub update_existing: bool,
    /// Whether to skip workflows with validation errors
    pub skip_invalid: bool,
}

impl Default for RegistrationOptions {
    fn default() -> Self {
        Self {
            update_existing: true,
            skip_invalid: true,
        }
    }
}

/// Result of workflow registration
#[derive(Debug, Clone)]
pub struct RegistrationResult {
    /// Workflow reference name
    pub ref_name: String,
    /// Whether the workflow was created (false = updated)
    pub created: bool,
    /// Workflow definition ID
    pub workflow_def_id: i64,
    /// Any warnings during registration
    pub warnings: Vec<String>,
}

/// Workflow registrar for registering workflows in the database
pub struct WorkflowRegistrar {
    pool: PgPool,
    options: RegistrationOptions,
    metadata_cache: MetadataCache,
}

impl WorkflowRegistrar {
    /// Create a new workflow registrar
    pub fn new(pool: PgPool, options: RegistrationOptions) -> Self {
        Self::new_with_metadata_cache(pool, options, MetadataCache::disabled())
    }

    pub fn new_with_metadata_cache(
        pool: PgPool,
        options: RegistrationOptions,
        metadata_cache: MetadataCache,
    ) -> Self {
        Self {
            pool,
            options,
            metadata_cache,
        }
    }

    /// Resolve the effective ref for a workflow.
    ///
    /// Prefers the value declared in the YAML; falls back to the
    /// `WorkflowFile.ref_name` derived from the filename when the YAML
    /// omits it (action-linked workflow files).
    fn effective_ref(loaded: &LoadedWorkflow) -> String {
        if loaded.workflow.r#ref.is_empty() {
            loaded.file.ref_name.clone()
        } else {
            loaded.workflow.r#ref.clone()
        }
    }

    /// Resolve the effective label for a workflow.
    ///
    /// Prefers the value declared in the YAML; falls back to the
    /// `WorkflowFile.name` (human-readable filename stem) when the YAML
    /// omits it.
    fn effective_label(loaded: &LoadedWorkflow) -> String {
        if loaded.workflow.label.is_empty() {
            loaded.file.name.clone()
        } else {
            loaded.workflow.label.clone()
        }
    }

    /// Register a single workflow
    pub async fn register_workflow(&self, loaded: &LoadedWorkflow) -> Result<RegistrationResult> {
        debug!("Registering workflow: {}", loaded.file.ref_name);

        // Check for validation errors
        if let Some(ref err) = loaded.validation_error {
            if self.options.skip_invalid {
                return Err(Error::validation(format!(
                    "Workflow has validation errors: {}",
                    err
                )));
            }
        }

        // Verify pack exists
        let pack = PackRepository::find_by_ref(&self.pool, &loaded.file.pack)
            .await?
            .ok_or_else(|| Error::not_found("pack", "ref", &loaded.file.pack))?;

        // Check if workflow already exists
        let existing_workflow =
            WorkflowDefinitionRepository::find_by_ref(&self.pool, &loaded.file.ref_name).await?;

        let mut warnings = Vec::new();

        // Add validation warning if present
        if let Some(ref err) = loaded.validation_error {
            warnings.push(err.clone());
        }

        // Resolve effective ref/label — prefer workflow YAML values, fall
        // back to filename-derived values for action-linked workflow files
        // that omit action-level metadata.
        let effective_ref = Self::effective_ref(loaded);
        let effective_label = Self::effective_label(loaded);

        let (workflow_def_id, created) = if let Some(existing) = existing_workflow {
            if !self.options.update_existing {
                return Err(Error::already_exists(
                    "workflow",
                    "ref",
                    &loaded.file.ref_name,
                ));
            }

            info!("Updating existing workflow: {}", loaded.file.ref_name);
            let workflow_def_id = self
                .update_workflow(
                    &existing.id,
                    &loaded.workflow,
                    &pack.r#ref,
                    &effective_ref,
                    &effective_label,
                )
                .await?;

            // Update or create the companion action record
            self.ensure_companion_action(
                workflow_def_id,
                &loaded.workflow,
                pack.id,
                &pack.r#ref,
                &loaded.file.name,
                &effective_ref,
                &effective_label,
            )
            .await?;

            (workflow_def_id, false)
        } else {
            info!("Creating new workflow: {}", loaded.file.ref_name);
            let workflow_def_id = self
                .create_workflow(
                    &loaded.workflow,
                    &loaded.file.pack,
                    pack.id,
                    &pack.r#ref,
                    &effective_ref,
                    &effective_label,
                )
                .await?;

            // Create a companion action record so the workflow appears in action lists
            self.create_companion_action(
                workflow_def_id,
                &loaded.workflow,
                pack.id,
                &pack.r#ref,
                &loaded.file.name,
                &effective_ref,
                &effective_label,
            )
            .await?;

            (workflow_def_id, true)
        };

        self.refresh_cached_metadata(workflow_def_id).await?;

        Ok(RegistrationResult {
            ref_name: loaded.file.ref_name.clone(),
            created,
            workflow_def_id,
            warnings,
        })
    }

    /// Register multiple workflows
    pub async fn register_workflows(
        &self,
        workflows: &HashMap<String, LoadedWorkflow>,
    ) -> Result<Vec<RegistrationResult>> {
        let mut results = Vec::new();
        let mut errors = Vec::new();

        for (ref_name, loaded) in workflows {
            match self.register_workflow(loaded).await {
                Ok(result) => {
                    info!("Registered workflow: {}", ref_name);
                    results.push(result);
                }
                Err(e) => {
                    warn!("Failed to register workflow '{}': {}", ref_name, e);
                    errors.push(format!("{}: {}", ref_name, e));
                }
            }
        }

        if !errors.is_empty() && results.is_empty() {
            return Err(Error::validation(format!(
                "Failed to register any workflows: {}",
                errors.join("; ")
            )));
        }

        Ok(results)
    }

    /// Unregister a workflow by reference
    pub async fn unregister_workflow(&self, ref_name: &str) -> Result<()> {
        debug!("Unregistering workflow: {}", ref_name);

        let workflow = WorkflowDefinitionRepository::find_by_ref(&self.pool, ref_name)
            .await?
            .ok_or_else(|| Error::not_found("workflow", "ref", ref_name))?;
        let companion_action =
            ActionRepository::find_by_workflow_def(&self.pool, workflow.id).await?;

        // Delete workflow definition (cascades to workflow_execution, and the companion
        // action is cascade-deleted via the FK on action.workflow_def)
        WorkflowDefinitionRepository::delete(&self.pool, workflow.id).await?;

        if self.metadata_cache.is_enabled() {
            let cache = CachedMetadataRepository::new(&self.pool, &self.metadata_cache);
            cache.evict_workflow_definition_best_effort(&workflow).await;
            if let Some(action) = companion_action.as_ref() {
                cache.evict_action_best_effort(action).await;
            }
        }

        info!("Unregistered workflow: {}", ref_name);
        Ok(())
    }

    async fn refresh_cached_metadata(&self, workflow_def_id: i64) -> Result<()> {
        if !self.metadata_cache.is_enabled() {
            return Ok(());
        }

        let cache = CachedMetadataRepository::new(&self.pool, &self.metadata_cache);
        if let Some(workflow) =
            WorkflowDefinitionRepository::find_by_id(&self.pool, workflow_def_id).await?
        {
            cache.put_workflow_definition_best_effort(&workflow).await;
        }
        if let Some(action) =
            ActionRepository::find_by_workflow_def(&self.pool, workflow_def_id).await?
        {
            cache.put_action_best_effort(&action).await;
        }

        Ok(())
    }

    /// Create a companion action record for a workflow definition.
    ///
    /// This ensures the workflow appears in action lists and the action palette
    /// in the workflow builder. The action is linked to the workflow definition
    /// via the `workflow_def` FK.
    ///
    /// `effective_ref` and `effective_label` are the resolved values (which may
    /// have been derived from the filename when the workflow YAML omits them).
    #[allow(clippy::too_many_arguments)]
    async fn create_companion_action(
        &self,
        workflow_def_id: i64,
        workflow: &WorkflowYaml,
        pack_id: i64,
        pack_ref: &str,
        workflow_name: &str,
        effective_ref: &str,
        effective_label: &str,
    ) -> Result<()> {
        let entrypoint = format!("workflows/{}.workflow.yaml", workflow_name);

        let action_input = CreateActionInput {
            r#ref: effective_ref.to_string(),
            pack: pack_id,
            pack_ref: pack_ref.to_string(),
            label: effective_label.to_string(),
            description: workflow.description.clone(),
            entrypoint,
            runtime: None,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: serde_json::json!({}),
            worker_selector: serde_json::json!({}),
            worker_tolerations: serde_json::json!([]),
            worker_affinity: serde_json::json!({}),
            param_schema: workflow.parameters.clone(),
            out_schema: workflow.output.clone(),
            is_adhoc: false,
            accesses_mcp: false,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        };

        let action = ActionRepository::create(&self.pool, action_input).await?;

        // Link the action to the workflow definition (sets workflow_def FK)
        ActionRepository::link_workflow_def(&self.pool, action.id, workflow_def_id).await?;

        info!(
            "Created companion action '{}' (ID: {}) for workflow definition (ID: {})",
            effective_ref, action.id, workflow_def_id
        );

        Ok(())
    }

    /// Ensure a companion action record exists for a workflow definition.
    ///
    /// If the action already exists, update it. If it doesn't exist (e.g., for
    /// workflows registered before the companion-action fix), create it.
    ///
    /// `effective_ref` and `effective_label` are the resolved values (which may
    /// have been derived from the filename when the workflow YAML omits them).
    #[allow(clippy::too_many_arguments)]
    async fn ensure_companion_action(
        &self,
        workflow_def_id: i64,
        workflow: &WorkflowYaml,
        pack_id: i64,
        pack_ref: &str,
        workflow_name: &str,
        effective_ref: &str,
        effective_label: &str,
    ) -> Result<()> {
        let existing_action =
            ActionRepository::find_by_workflow_def(&self.pool, workflow_def_id).await?;

        if let Some(action) = existing_action {
            // Update the existing companion action to stay in sync
            let update_input = UpdateActionInput {
                label: Some(effective_label.to_string()),
                description: Some(match workflow.description.clone() {
                    Some(description) => Patch::Set(description),
                    None => Patch::Clear,
                }),
                entrypoint: Some(format!("workflows/{}.workflow.yaml", workflow_name)),
                runtime: None,
                enabled: None,
                runtime_version_constraint: None,
                required_worker_runtimes: None,
                worker_selector: None,
                worker_tolerations: None,
                worker_affinity: None,
                param_schema: workflow.parameters.clone(),
                out_schema: workflow.output.clone(),
                parameter_delivery: None,
                parameter_format: None,
                output_format: None,
                accesses_mcp: None,
                default_execution_permission_set_refs: None,
                reference_visibility: None,
                reference_allowed_pack_refs: None,
                artifact_retention_policy: None,
                artifact_retention_limit: None,
                log_retention_policy: None,
                log_retention_limit: None,
                timeout_seconds: None,
            };

            ActionRepository::update(&self.pool, action.id, update_input).await?;

            debug!(
                "Updated companion action '{}' (ID: {}) for workflow definition (ID: {})",
                action.r#ref, action.id, workflow_def_id
            );
        } else {
            // Backfill: create companion action for pre-fix workflows
            self.create_companion_action(
                workflow_def_id,
                workflow,
                pack_id,
                pack_ref,
                workflow_name,
                effective_ref,
                effective_label,
            )
            .await?;
        }

        Ok(())
    }

    /// Create a new workflow definition
    ///
    /// `effective_ref` and `effective_label` are the resolved values (which may
    /// have been derived from the filename when the workflow YAML omits them).
    async fn create_workflow(
        &self,
        workflow: &WorkflowYaml,
        _pack_name: &str,
        pack_id: i64,
        pack_ref: &str,
        effective_ref: &str,
        effective_label: &str,
    ) -> Result<i64> {
        // Convert the parsed workflow back to JSON for storage
        let definition = serde_json::to_value(workflow)
            .map_err(|e| Error::validation(format!("Failed to serialize workflow: {}", e)))?;

        let input = CreateWorkflowDefinitionInput {
            r#ref: effective_ref.to_string(),
            pack: pack_id,
            pack_ref: pack_ref.to_string(),
            label: effective_label.to_string(),
            description: workflow.description.clone(),
            version: workflow.version.clone(),
            param_schema: workflow.parameters.clone(),
            out_schema: workflow.output.clone(),
            definition,
            tags: workflow.tags.clone(),
        };

        let created = WorkflowDefinitionRepository::create(&self.pool, input).await?;

        Ok(created.id)
    }

    /// Update an existing workflow definition
    ///
    /// `effective_ref` and `effective_label` are the resolved values (which may
    /// have been derived from the filename when the workflow YAML omits them).
    async fn update_workflow(
        &self,
        workflow_id: &i64,
        workflow: &WorkflowYaml,
        _pack_ref: &str,
        _effective_ref: &str,
        effective_label: &str,
    ) -> Result<i64> {
        // Convert the parsed workflow back to JSON for storage
        let definition = serde_json::to_value(workflow)
            .map_err(|e| Error::validation(format!("Failed to serialize workflow: {}", e)))?;

        let input = UpdateWorkflowDefinitionInput {
            label: Some(effective_label.to_string()),
            description: workflow.description.clone(),
            version: Some(workflow.version.clone()),
            param_schema: workflow.parameters.clone(),
            out_schema: workflow.output.clone(),
            definition: Some(definition),
            tags: Some(workflow.tags.clone()),
        };

        let updated = WorkflowDefinitionRepository::update(&self.pool, *workflow_id, input).await?;

        Ok(updated.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registration_options_default() {
        let options = RegistrationOptions::default();
        assert!(options.update_existing);
        assert!(options.skip_invalid);
    }

    #[test]
    fn test_registration_result_creation() {
        let result = RegistrationResult {
            ref_name: "test.workflow".to_string(),
            created: true,
            workflow_def_id: 123,
            warnings: vec![],
        };

        assert_eq!(result.ref_name, "test.workflow");
        assert!(result.created);
        assert_eq!(result.workflow_def_id, 123);
        assert_eq!(result.warnings.len(), 0);
    }
}
