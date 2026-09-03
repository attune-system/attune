//! Repository layer for database operations
//!
//! This module provides the repository pattern for all database entities in Attune.
//! Repositories abstract database operations and provide a clean interface for CRUD
//! operations and queries.
//!
//! # Architecture
//!
//! - Each entity has its own repository module (e.g., `pack`, `action`, `trigger`)
//! - Repositories use SQLx for database operations
//! - Transaction support is provided through SQLx's transaction types
//! - All operations return `Result<T, Error>` for consistent error handling
//!
//! # Example
//!
//! ```rust,no_run
//! use attune_common::repositories::{PackRepository, FindByRef};
//! use attune_common::db::Database;
//!
//! async fn example(db: &Database) -> attune_common::Result<()> {
//!     if let Some(pack) = PackRepository::find_by_ref(db.pool(), "core").await? {
//!         println!("Found pack: {}", pack.label);
//!     }
//!     Ok(())
//! }
//! ```

use sqlx::{Executor, Postgres, Transaction};

pub mod action;
pub mod analytics;
pub mod artifact;
pub mod cache;
pub mod dashboard;
pub mod entity_history;
pub mod event;
pub mod execution;
pub mod execution_admission;
pub mod execution_secret_value;
pub mod identity;
pub mod inquiry;
pub mod integration_token;
pub mod key;
pub mod maintenance;
pub mod notification;
pub mod pack;
pub mod pack_install;
pub mod pack_registry_index;
pub mod pack_test;
pub mod queue_stats;
pub mod retention;
pub mod rule;
pub mod runtime;
pub mod runtime_version;
pub mod sensor_admission;
pub mod sensor_process;
pub mod sensor_workload;
pub mod sensor_workload_admission;
pub mod trigger;
pub mod work_queue;
pub mod workflow;
pub mod workflow_cache_iteration;

pub(crate) fn ref_filter_like_pattern(filter: &str) -> Option<String> {
    if !filter.contains('*') {
        return None;
    }

    let mut pattern = String::with_capacity(filter.len());
    for ch in filter.chars() {
        match ch {
            '*' => pattern.push('%'),
            '\\' => pattern.push_str(r"\\"),
            '%' => pattern.push_str(r"\%"),
            '_' => pattern.push_str(r"\_"),
            ch => pattern.push(ch),
        }
    }

    Some(pattern)
}

/// Turns a user-entered catalog query into bounded, escaped `LIKE` patterns.
///
/// Each whitespace-separated token becomes an independently AND-matched
/// pattern. Escape `%`, `_`, and `\` so text search treats them literally.
pub(crate) fn text_search_patterns(query: Option<&str>) -> Vec<String> {
    const MAX_TOKENS: usize = 16;

    query
        .into_iter()
        .flat_map(str::split_whitespace)
        .take(MAX_TOKENS)
        .map(|token| {
            let mut escaped = String::with_capacity(token.len());
            for ch in token.to_lowercase().chars() {
                match ch {
                    '\\' | '%' | '_' => {
                        escaped.push('\\');
                        escaped.push(ch);
                    }
                    _ => escaped.push(ch),
                }
            }
            format!("%{escaped}%")
        })
        .collect()
}

#[cfg(test)]
mod text_search_tests {
    use super::text_search_patterns;

    #[test]
    fn text_search_patterns_tokenizes_and_escapes_like_wildcards() {
        assert_eq!(
            text_search_patterns(Some(" Core%  foo_bar\\baz ")),
            vec!["%core\\%%", "%foo\\_bar\\\\baz%"]
        );
    }
}

// Re-export repository types
pub use action::{ActionRepository, PolicyRepository};
pub use analytics::AnalyticsRepository;
pub use artifact::{ArtifactRepository, ArtifactVersionRepository};
pub use cache::{
    CacheEntryRepository, CacheGenerationRepository, CacheIngestRepository,
    CacheNamespaceRepository,
};
pub use dashboard::{DashboardRepository, DashboardVersionRepository};
pub use entity_history::EntityHistoryRepository;
pub use event::{EnforcementRepository, EventRepository};
pub use execution::ExecutionRepository;
pub use execution_admission::ExecutionAdmissionRepository;
pub use execution_secret_value::ExecutionSecretValueRepository;
pub use identity::{
    DeleteIdentityOutcome, IdentityRepository, PermissionAssignmentRepository,
    PermissionSetRepository,
};
pub use inquiry::InquiryRepository;
pub use integration_token::IntegrationTokenRepository;
pub use key::KeyRepository;
pub use maintenance::MaintenanceRepository;
pub use notification::NotificationRepository;
pub use pack::PackRepository;
pub use pack_install::{pack_install_is_terminal, PackInstallRepository};
pub use pack_registry_index::PackRegistryIndexRepository;
pub use pack_test::PackTestRepository;
pub use queue_stats::QueueStatsRepository;
pub use retention::RetentionRepository;
pub use rule::RuleRepository;
pub use runtime::{RuntimeRepository, WorkerRepository};
pub use runtime_version::RuntimeVersionRepository;
pub use sensor_admission::SensorAdmissionRepository;
pub use sensor_process::SensorProcessRepository;
pub use sensor_workload::SensorWorkloadRepository;
pub use sensor_workload_admission::SensorWorkloadAdmissionRepository;
pub use trigger::{SensorRepository, TriggerRepository};
pub use work_queue::{WorkQueueDispatchRepository, WorkQueueItemRepository, WorkQueueRepository};
pub use workflow::{WorkflowDefinitionRepository, WorkflowExecutionRepository};
pub use workflow_cache_iteration::WorkflowCacheIterationRepository;

/// Explicit patch operation for update inputs where callers must distinguish
/// between "leave unchanged", "set value", and "clear to NULL".
#[derive(Debug, Clone, PartialEq)]
pub enum Patch<T> {
    Set(T),
    Clear,
}

/// Type alias for database connection/transaction
pub type DbConnection<'c> = &'c mut Transaction<'c, Postgres>;

/// Base repository trait providing common functionality
///
/// This trait is not meant to be used directly, but serves as a foundation
/// for specific repository implementations.
pub trait Repository {
    /// The entity type this repository manages
    type Entity;

    /// Get the name of the table for this repository
    fn table_name() -> &'static str;
}

/// Trait for repositories that support finding by ID
#[async_trait::async_trait]
pub trait FindById: Repository {
    /// Find an entity by its ID
    ///
    /// # Arguments
    ///
    /// * `executor` - Database executor (pool or transaction)
    /// * `id` - The ID to search for
    ///
    /// # Returns
    ///
    /// * `Ok(Some(entity))` if found
    /// * `Ok(None)` if not found
    /// * `Err(error)` on database error
    async fn find_by_id<'e, E>(executor: E, id: i64) -> crate::Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e;

    /// Get an entity by its ID, returning an error if not found
    ///
    /// # Arguments
    ///
    /// * `executor` - Database executor (pool or transaction)
    /// * `id` - The ID to search for
    ///
    /// # Returns
    ///
    /// * `Ok(entity)` if found
    /// * `Err(NotFound)` if not found
    /// * `Err(error)` on database error
    async fn get_by_id<'e, E>(executor: E, id: i64) -> crate::Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        Self::find_by_id(executor, id)
            .await?
            .ok_or_else(|| crate::Error::not_found(Self::table_name(), "id", id.to_string()))
    }
}

/// Trait for repositories that support finding by reference
#[async_trait::async_trait]
pub trait FindByRef: Repository {
    /// Find an entity by its reference string
    ///
    /// # Arguments
    ///
    /// * `executor` - Database executor (pool or transaction)
    /// * `ref_str` - The reference string to search for
    ///
    /// # Returns
    ///
    /// * `Ok(Some(entity))` if found
    /// * `Ok(None)` if not found
    /// * `Err(error)` on database error
    async fn find_by_ref<'e, E>(executor: E, ref_str: &str) -> crate::Result<Option<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e;

    /// Get an entity by its reference, returning an error if not found
    ///
    /// # Arguments
    ///
    /// * `executor` - Database executor (pool or transaction)
    /// * `ref_str` - The reference string to search for
    ///
    /// # Returns
    ///
    /// * `Ok(entity)` if found
    /// * `Err(NotFound)` if not found
    /// * `Err(error)` on database error
    async fn get_by_ref<'e, E>(executor: E, ref_str: &str) -> crate::Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e,
    {
        Self::find_by_ref(executor, ref_str)
            .await?
            .ok_or_else(|| crate::Error::not_found(Self::table_name(), "ref", ref_str))
    }
}

/// Trait for repositories that support listing all entities
#[async_trait::async_trait]
pub trait List: Repository {
    /// List all entities
    ///
    /// # Arguments
    ///
    /// * `executor` - Database executor (pool or transaction)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<entity>)` - List of all entities
    /// * `Err(error)` on database error
    async fn list<'e, E>(executor: E) -> crate::Result<Vec<Self::Entity>>
    where
        E: Executor<'e, Database = Postgres> + 'e;
}

/// Trait for repositories that support creating entities
#[async_trait::async_trait]
pub trait Create: Repository {
    /// Input type for creating a new entity
    type CreateInput;

    /// Create a new entity
    ///
    /// # Arguments
    ///
    /// * `executor` - Database executor (pool or transaction)
    /// * `input` - The data for creating the entity
    ///
    /// # Returns
    ///
    /// * `Ok(entity)` - The created entity
    /// * `Err(error)` on database error or validation failure
    async fn create<'e, E>(executor: E, input: Self::CreateInput) -> crate::Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e;
}

/// Trait for repositories that support updating entities
#[async_trait::async_trait]
pub trait Update: Repository {
    /// Input type for updating an entity
    type UpdateInput;

    /// Update an existing entity by ID
    ///
    /// # Arguments
    ///
    /// * `executor` - Database executor (pool or transaction)
    /// * `id` - The ID of the entity to update
    /// * `input` - The data for updating the entity
    ///
    /// # Returns
    ///
    /// * `Ok(entity)` - The updated entity
    /// * `Err(NotFound)` if the entity doesn't exist
    /// * `Err(error)` on database error or validation failure
    async fn update<'e, E>(
        executor: E,
        id: i64,
        input: Self::UpdateInput,
    ) -> crate::Result<Self::Entity>
    where
        E: Executor<'e, Database = Postgres> + 'e;
}

/// Trait for repositories that support deleting entities
#[async_trait::async_trait]
pub trait Delete: Repository {
    /// Delete an entity by ID
    ///
    /// # Arguments
    ///
    /// * `executor` - Database executor (pool or transaction)
    /// * `id` - The ID of the entity to delete
    ///
    /// # Returns
    ///
    /// * `Ok(true)` if the entity was deleted
    /// * `Ok(false)` if the entity didn't exist
    /// * `Err(error)` on database error
    async fn delete<'e, E>(executor: E, id: i64) -> crate::Result<bool>
    where
        E: Executor<'e, Database = Postgres> + 'e;
}

/// Helper struct for pagination parameters
#[derive(Debug, Clone, Copy)]
pub struct Pagination {
    /// Page number (0-based)
    pub page: i64,
    /// Number of items per page
    pub per_page: i64,
}

impl Pagination {
    /// Create a new Pagination instance
    pub fn new(page: i64, per_page: i64) -> Self {
        Self { page, per_page }
    }

    /// Calculate the OFFSET for SQL queries
    pub fn offset(&self) -> i64 {
        self.page * self.per_page
    }

    /// Get the LIMIT for SQL queries
    pub fn limit(&self) -> i64 {
        self.per_page
    }
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            page: 0,
            per_page: 50,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pagination() {
        let p = Pagination::new(0, 10);
        assert_eq!(p.offset(), 0);
        assert_eq!(p.limit(), 10);

        let p = Pagination::new(2, 10);
        assert_eq!(p.offset(), 20);
        assert_eq!(p.limit(), 10);
    }

    #[test]
    fn test_pagination_default() {
        let p = Pagination::default();
        assert_eq!(p.page, 0);
        assert_eq!(p.per_page, 50);
    }

    #[test]
    fn ref_filter_like_pattern_supports_glob_wildcards() {
        assert_eq!(
            ref_filter_like_pattern("core.*"),
            Some("core.%".to_string())
        );
        assert_eq!(
            ref_filter_like_pattern("core.queue_*"),
            Some(r"core.queue\_%".to_string())
        );
        assert_eq!(ref_filter_like_pattern("core.timer"), None);
    }

    #[test]
    fn ref_filter_like_pattern_escapes_like_metacharacters() {
        assert_eq!(
            ref_filter_like_pattern("pack%_name.*"),
            Some(r"pack\%\_name.%".to_string())
        );
    }
}
