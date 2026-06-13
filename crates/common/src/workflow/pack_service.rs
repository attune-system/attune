//! Pack Workflow Service
//!
//! This module provides high-level operations for managing workflows within packs,
//! orchestrating the loading, validation, and registration of workflows.

use crate::error::{Error, Result};
use crate::metadata_cache::MetadataCache;
use crate::repositories::{Delete, FindByRef, List, PackRepository, WorkflowDefinitionRepository};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, info, warn};

use super::loader::{LoaderConfig, WorkflowLoader};
use super::registrar::{RegistrationOptions, RegistrationResult, WorkflowRegistrar};

/// Pack workflow service configuration
#[derive(Debug, Clone)]
pub struct PackWorkflowServiceConfig {
    /// Base directory containing pack directories
    pub packs_base_dir: PathBuf,
    /// Whether to skip validation errors during loading
    pub skip_validation_errors: bool,
    /// Whether to update existing workflows during sync
    pub update_existing: bool,
    /// Maximum workflow file size in bytes
    pub max_file_size: usize,
}

impl Default for PackWorkflowServiceConfig {
    fn default() -> Self {
        Self {
            packs_base_dir: PathBuf::from("/opt/attune/packs"),
            skip_validation_errors: false,
            update_existing: true,
            max_file_size: 1024 * 1024, // 1MB
        }
    }
}

/// Result of syncing workflows for a pack
#[derive(Debug, Clone)]
pub struct PackSyncResult {
    /// Pack reference
    pub pack_ref: String,
    /// Number of workflows loaded from filesystem
    pub loaded_count: usize,
    /// Number of workflows registered/updated in database
    pub registered_count: usize,
    /// Registration results for individual workflows
    pub workflows: Vec<RegistrationResult>,
    /// Errors encountered during sync
    pub errors: Vec<String>,
}

/// Result of validating workflows for a pack
#[derive(Debug, Clone)]
pub struct PackValidationResult {
    /// Pack reference
    pub pack_ref: String,
    /// Number of workflows validated
    pub validated_count: usize,
    /// Number of workflows with errors
    pub error_count: usize,
    /// Validation errors by workflow reference
    pub errors: HashMap<String, Vec<String>>,
}

/// Service for managing workflows within packs
pub struct PackWorkflowService {
    pool: PgPool,
    config: PackWorkflowServiceConfig,
    metadata_cache: MetadataCache,
}

impl PackWorkflowService {
    /// Create a new pack workflow service
    pub fn new(pool: PgPool, config: PackWorkflowServiceConfig) -> Self {
        Self::new_with_metadata_cache(pool, config, MetadataCache::disabled())
    }

    pub fn new_with_metadata_cache(
        pool: PgPool,
        config: PackWorkflowServiceConfig,
        metadata_cache: MetadataCache,
    ) -> Self {
        Self {
            pool,
            config,
            metadata_cache,
        }
    }

    /// Sync workflows from filesystem to database for a specific pack
    ///
    /// This loads all workflow YAML files from the pack's workflows directory
    /// and registers them in the database.
    pub async fn sync_pack_workflows(&self, pack_ref: &str) -> Result<PackSyncResult> {
        info!("Syncing workflows for pack: {}", pack_ref);

        // Verify pack exists in database
        let _pack = PackRepository::find_by_ref(&self.pool, pack_ref)
            .await?
            .ok_or_else(|| Error::not_found("pack", "ref", pack_ref))?;

        // Load workflows from filesystem
        let loader_config = LoaderConfig {
            packs_base_dir: self.config.packs_base_dir.clone(),
            skip_validation: self.config.skip_validation_errors,
            max_file_size: self.config.max_file_size,
        };

        let loader = WorkflowLoader::new(loader_config);
        let pack_dir = self.config.packs_base_dir.join(pack_ref);

        let workflows = match loader.load_pack_workflows(pack_ref, &pack_dir).await {
            Ok(workflows) => workflows,
            Err(e) => {
                warn!("Failed to load workflows for pack '{}': {}", pack_ref, e);
                return Ok(PackSyncResult {
                    pack_ref: pack_ref.to_string(),
                    loaded_count: 0,
                    registered_count: 0,
                    workflows: Vec::new(),
                    errors: vec![format!("Failed to load workflows: {}", e)],
                });
            }
        };

        let loaded_count = workflows.len();

        if loaded_count == 0 {
            debug!("No workflows found for pack '{}'", pack_ref);
            return Ok(PackSyncResult {
                pack_ref: pack_ref.to_string(),
                loaded_count: 0,
                registered_count: 0,
                workflows: Vec::new(),
                errors: Vec::new(),
            });
        }

        // Register workflows in database
        let registrar_options = RegistrationOptions {
            update_existing: self.config.update_existing,
            skip_invalid: self.config.skip_validation_errors,
        };

        let registrar = WorkflowRegistrar::new_with_metadata_cache(
            self.pool.clone(),
            registrar_options,
            self.metadata_cache.clone(),
        );
        let results = registrar.register_workflows(&workflows).await?;

        let registered_count = results.len();
        let errors: Vec<String> = results.iter().flat_map(|r| r.warnings.clone()).collect();

        info!(
            "Synced {} workflows for pack '{}' ({} registered/updated)",
            loaded_count, pack_ref, registered_count
        );

        Ok(PackSyncResult {
            pack_ref: pack_ref.to_string(),
            loaded_count,
            registered_count,
            workflows: results,
            errors,
        })
    }

    /// Validate workflows for a specific pack without registering them
    ///
    /// This loads workflow YAML files and validates them, returning any errors found.
    pub async fn validate_pack_workflows(&self, pack_ref: &str) -> Result<PackValidationResult> {
        info!("Validating workflows for pack: {}", pack_ref);

        // Verify pack exists
        PackRepository::find_by_ref(&self.pool, pack_ref)
            .await?
            .ok_or_else(|| Error::not_found("pack", "ref", pack_ref))?;

        // Load workflows with validation enabled
        let loader_config = LoaderConfig {
            packs_base_dir: self.config.packs_base_dir.clone(),
            skip_validation: false, // Always validate
            max_file_size: self.config.max_file_size,
        };

        let loader = WorkflowLoader::new(loader_config);
        let pack_dir = self.config.packs_base_dir.join(pack_ref);

        let workflows = loader.load_pack_workflows(pack_ref, &pack_dir).await?;
        let validated_count = workflows.len();

        let mut errors: HashMap<String, Vec<String>> = HashMap::new();
        let mut error_count = 0;

        for (ref_name, loaded) in workflows {
            let mut workflow_errors = Vec::new();

            // Check for validation error from loader
            if let Some(validation_error) = loaded.validation_error {
                workflow_errors.push(validation_error);
                error_count += 1;
            }

            // Additional validation checks
            // Check if pack reference matches
            if !loaded.workflow.r#ref.starts_with(&format!("{}.", pack_ref)) {
                workflow_errors.push(format!(
                    "Workflow ref '{}' does not match pack '{}'",
                    loaded.workflow.r#ref, pack_ref
                ));
                error_count += 1;
            }

            if !workflow_errors.is_empty() {
                errors.insert(ref_name, workflow_errors);
            }
        }

        info!(
            "Validated {} workflows for pack '{}' ({} errors)",
            validated_count, pack_ref, error_count
        );

        Ok(PackValidationResult {
            pack_ref: pack_ref.to_string(),
            validated_count,
            error_count,
            errors,
        })
    }

    /// Delete all workflows for a specific pack
    ///
    /// This removes all workflow definitions from the database for the given pack.
    /// Note: Database cascading should handle this automatically when a pack is deleted.
    pub async fn delete_pack_workflows(&self, pack_ref: &str) -> Result<usize> {
        info!("Deleting workflows for pack: {}", pack_ref);

        let workflows =
            WorkflowDefinitionRepository::find_by_pack_ref(&self.pool, pack_ref).await?;

        let mut deleted_count = 0;

        for workflow in workflows {
            if WorkflowDefinitionRepository::delete(&self.pool, workflow.id).await? {
                deleted_count += 1;
            }
        }

        info!(
            "Deleted {} workflows for pack '{}'",
            deleted_count, pack_ref
        );

        Ok(deleted_count)
    }

    /// Get count of workflows for a specific pack
    pub async fn count_pack_workflows(&self, pack_ref: &str) -> Result<i64> {
        WorkflowDefinitionRepository::count_by_pack(&self.pool, pack_ref).await
    }

    /// Sync all workflows for all packs
    ///
    /// This is useful for initial setup or bulk synchronization.
    pub async fn sync_all_packs(&self) -> Result<Vec<PackSyncResult>> {
        info!("Syncing workflows for all packs");

        let packs = PackRepository::list(&self.pool).await?;
        let mut results = Vec::new();

        for pack in packs {
            match self.sync_pack_workflows(&pack.r#ref).await {
                Ok(result) => results.push(result),
                Err(e) => {
                    warn!("Failed to sync pack '{}': {}", pack.r#ref, e);
                    results.push(PackSyncResult {
                        pack_ref: pack.r#ref.clone(),
                        loaded_count: 0,
                        registered_count: 0,
                        workflows: Vec::new(),
                        errors: vec![format!("Failed to sync: {}", e)],
                    });
                }
            }
        }

        info!("Completed syncing {} packs", results.len());
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PackWorkflowServiceConfig::default();
        assert_eq!(config.packs_base_dir, PathBuf::from("/opt/attune/packs"));
        assert!(!config.skip_validation_errors);
        assert!(config.update_existing);
        assert_eq!(config.max_file_size, 1024 * 1024);
    }

    #[test]
    fn test_pack_sync_result_creation() {
        let result = PackSyncResult {
            pack_ref: "test_pack".to_string(),
            loaded_count: 5,
            registered_count: 4,
            workflows: Vec::new(),
            errors: vec!["error1".to_string()],
        };

        assert_eq!(result.pack_ref, "test_pack");
        assert_eq!(result.loaded_count, 5);
        assert_eq!(result.registered_count, 4);
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn test_pack_validation_result_creation() {
        let mut errors = HashMap::new();
        errors.insert(
            "test.workflow".to_string(),
            vec!["validation error".to_string()],
        );

        let result = PackValidationResult {
            pack_ref: "test_pack".to_string(),
            validated_count: 10,
            error_count: 1,
            errors,
        };

        assert_eq!(result.pack_ref, "test_pack");
        assert_eq!(result.validated_count, 10);
        assert_eq!(result.error_count, 1);
        assert_eq!(result.errors.len(), 1);
    }
}
