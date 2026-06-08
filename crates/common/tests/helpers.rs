//! Test helpers and utilities for integration tests
//!
//! This module provides common test fixtures, database setup/teardown,
//! and utility functions for testing repositories and database operations.

#![allow(dead_code)]

use attune_common::{
    config::Config,
    db::Database,
    models::*,
    repositories::{
        action::{self, ActionRepository},
        identity::{self, IdentityRepository},
        key::{self, KeyRepository},
        pack::{self, PackRepository},
        runtime::{self, RuntimeRepository},
        trigger::{self, SensorRepository, TriggerRepository},
        Create,
    },
    Result,
};
use serde_json::json;
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;

static INIT: Once = Once::new();
static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique test identifier for fixtures
///
/// This uses a combination of timestamp (last 6 digits) and atomic counter to ensure
/// unique identifiers across parallel test execution and multiple test runs.
/// Returns only alphanumeric characters and underscores to match pack ref validation.
pub fn unique_test_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Use last 6 digits of microsecond timestamp for compact uniqueness
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros()
        % 1_000_000;
    let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}{}", timestamp, counter)
}

/// Generate a unique pack ref for testing
///
/// Creates a valid pack ref that's unique across parallel test runs.
pub fn unique_pack_ref(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Generate a unique action name for testing
pub fn unique_action_name(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Generate a unique trigger name for testing
pub fn unique_trigger_name(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Generate a unique rule name for testing
pub fn unique_rule_name(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Generate a unique execution action ref for testing
pub fn unique_execution_ref(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Generate a unique event trigger ref for testing
pub fn unique_event_ref(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Generate a unique enforcement ref for testing
pub fn unique_enforcement_ref(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Generate a unique runtime name for testing
pub fn unique_runtime_name(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Generate a unique sensor name for testing
pub fn unique_sensor_name(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Generate a unique key name for testing
pub fn unique_key_name(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Generate a unique identity username for testing
pub fn unique_identity_username(base: &str) -> String {
    format!("{}_{}", base, unique_test_id())
}

/// Initialize test environment (run once)
pub fn init_test_env() {
    INIT.call_once(|| {
        // Set test environment for config loading - use ATTUNE_ENV instead of ATTUNE__ENVIRONMENT
        // to avoid config crate parsing conflicts
        std::env::set_var("ATTUNE_ENV", "test");

        // Initialize tracing for tests
        tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive(tracing::Level::WARN.into()),
            )
            .try_init()
            .ok();
    });
}

/// Create a test database pool with a unique schema
///
/// This creates a schema-per-test setup:
/// 1. Generates unique schema name
/// 2. Creates the schema in PostgreSQL
/// 3. Runs all migrations in that schema
/// 4. Returns a pool configured to use that schema
///
/// The schema should be cleaned up after the test using `cleanup_test_schema()`
pub async fn create_test_pool() -> Result<PgPool> {
    init_test_env();

    // Generate a unique schema name for this test
    let schema = format!("test_{}", uuid::Uuid::new_v4().to_string().replace("-", ""));

    // Create the base pool to create the schema
    let base_pool = create_base_pool().await?;

    // Create the test schema
    let create_schema_sql = format!("CREATE SCHEMA IF NOT EXISTS {}", schema);
    sqlx::query(&create_schema_sql).execute(&base_pool).await?;

    // Run migrations in the new schema
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let migrations_path = format!("{}/../../migrations", manifest_dir);
    let config_path = format!("{}/../../config.test.yaml", manifest_dir);

    // Load config and set our test schema
    let mut config = Config::load_from_file(&config_path)?;
    config.database.schema = Some(schema.clone());

    // Create a pool with after_connect hook to set search_path
    let migration_pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect({
            let schema = schema.clone();
            move |conn, _meta| {
                let schema = schema.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            }
        })
        .connect(&config.database.url)
        .await?;

    // Run migration SQL files
    let migration_files = std::fs::read_dir(&migrations_path)
        .map_err(|e| anyhow::anyhow!("Failed to read migrations directory: {}", e))?;
    let mut migrations: Vec<_> = migration_files
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();

    // Sort by filename to ensure migrations run in version order
    migrations.sort_by_key(|entry| entry.path().clone());

    for migration_file in migrations {
        let migration_path = migration_file.path();
        let sql = std::fs::read_to_string(&migration_path)
            .map_err(|e| anyhow::anyhow!("Failed to read migration file: {}", e))?;

        // Set search_path before each migration
        sqlx::query(&format!("SET search_path TO {}", schema))
            .execute(&migration_pool)
            .await?;

        // Execute the migration SQL
        if let Err(e) = sqlx::raw_sql(&sql).execute(&migration_pool).await {
            // Ignore "already exists" errors since enums may be global
            let error_msg = format!("{:?}", e);
            if !error_msg.contains("already exists") && !error_msg.contains("duplicate") {
                eprintln!(
                    "Migration error in {}: {}",
                    migration_file.path().display(),
                    e
                );
                return Err(e.into());
            }
        }
    }

    // Create the proper Database instance for use in tests
    let database = Database::new(&config.database).await?;
    let pool = database.pool().clone();

    Ok(pool)
}

/// Create a base database pool without schema-specific configuration
async fn create_base_pool() -> Result<PgPool> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let config_path = format!("{}/../../config.test.yaml", manifest_dir);
    let config = Config::load_from_file(&config_path)?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&config.database.url)
        .await?;

    Ok(pool)
}

/// Cleanup a test schema by dropping it
pub async fn cleanup_test_schema(_pool: &PgPool, schema_name: &str) -> Result<()> {
    // Get a connection to the base database
    let base_pool = create_base_pool().await?;

    // Drop the schema and all its contents
    let drop_schema_sql = format!("DROP SCHEMA IF EXISTS {} CASCADE", schema_name);
    sqlx::query(&drop_schema_sql).execute(&base_pool).await?;

    Ok(())
}

/// Clean all tables in the test database
pub async fn clean_database(pool: &PgPool) -> Result<()> {
    // Use TRUNCATE with CASCADE to clear all tables efficiently
    // This respects foreign key constraints and resets sequences
    // With schema-per-test, tables are in the current schema (set via search_path)
    sqlx::query(
        r#"
        TRUNCATE TABLE
            execution,
            inquiry,
            enforcement,
            event,
            rule,
            sensor,
            trigger,
            notification,
            key,
            identity,
            worker,
            runtime,
            action,
            pack,
            artifact,
            permission_assignment,
            permission_set,
            policy
        RESTART IDENTITY CASCADE
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Fixture builder for Pack
pub struct PackFixture {
    pub r#ref: String,
    pub label: String,
    pub version: String,
    pub description: Option<String>,
    pub conf_schema: serde_json::Value,
    pub config: serde_json::Value,
    pub meta: serde_json::Value,
    pub tags: Vec<String>,
    pub runtime_deps: Vec<String>,
    pub dependencies: Vec<String>,
    pub is_standard: bool,
}

impl PackFixture {
    /// Create a new pack fixture with the given ref name
    pub fn new(ref_name: &str) -> Self {
        Self {
            r#ref: ref_name.to_string(),
            label: format!("{} Pack", ref_name),
            version: "1.0.0".to_string(),
            description: Some(format!("Test pack for {}", ref_name)),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: vec!["test".to_string()],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
        }
    }

    /// Create a new pack fixture with a unique ref to avoid test collisions
    ///
    /// This is the recommended constructor for parallel test execution.
    pub fn new_unique(base_name: &str) -> Self {
        let unique_ref = unique_pack_ref(base_name);
        Self {
            r#ref: unique_ref.clone(),
            label: format!("{} Pack", base_name),
            version: "1.0.0".to_string(),
            description: Some(format!("Test pack for {}", base_name)),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: vec!["test".to_string()],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
        }
    }

    pub fn with_version(mut self, version: &str) -> Self {
        self.version = version.to_string();
        self
    }

    pub fn with_label(mut self, label: &str) -> Self {
        self.label = label.to_string();
        self
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_standard(mut self, is_standard: bool) -> Self {
        self.is_standard = is_standard;
        self
    }

    pub async fn create(self, pool: &PgPool) -> Result<Pack> {
        let input = pack::CreatePackInput {
            r#ref: self.r#ref,
            label: self.label,
            description: self.description,
            version: self.version,
            conf_schema: self.conf_schema,
            config: self.config,
            meta: self.meta,
            tags: self.tags,
            runtime_deps: self.runtime_deps,
            dependencies: self.dependencies,
            is_standard: self.is_standard,
            installers: serde_json::json!({}),
        };

        PackRepository::create(pool, input).await
    }
}

/// Fixture builder for Action
pub struct ActionFixture {
    pub pack_id: i64,
    pub pack_ref: String,
    pub r#ref: String,
    pub label: String,
    pub description: String,
    pub entrypoint: String,
    pub runtime: Option<i64>,
    pub param_schema: Option<serde_json::Value>,
    pub out_schema: Option<serde_json::Value>,
}

impl ActionFixture {
    /// Create a new action fixture with the given pack and action name
    pub fn new(pack_id: i64, pack_ref: &str, ref_name: &str) -> Self {
        Self {
            pack_id,
            pack_ref: pack_ref.to_string(),
            r#ref: format!("{}.{}", pack_ref, ref_name),
            label: ref_name.replace('_', " ").to_string(),
            description: format!("Test action: {}", ref_name),
            entrypoint: "main.py".to_string(),
            runtime: None,
            param_schema: None,
            out_schema: None,
        }
    }

    /// Create a new action fixture with a unique name to avoid test collisions
    ///
    /// This is the recommended constructor for parallel test execution.
    pub fn new_unique(pack_id: i64, pack_ref: &str, base_name: &str) -> Self {
        let unique_name = unique_action_name(base_name);
        Self {
            pack_id,
            pack_ref: pack_ref.to_string(),
            r#ref: format!("{}.{}", pack_ref, unique_name),
            label: base_name.replace('_', " ").to_string(),
            description: format!("Test action: {}", base_name),
            entrypoint: "main.py".to_string(),
            runtime: None,
            param_schema: None,
            out_schema: None,
        }
    }

    pub fn with_label(mut self, label: &str) -> Self {
        self.label = label.to_string();
        self
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = description.to_string();
        self
    }

    pub fn with_entrypoint(mut self, entrypoint: &str) -> Self {
        self.entrypoint = entrypoint.to_string();
        self
    }

    pub fn with_runtime(mut self, runtime_id: i64) -> Self {
        self.runtime = Some(runtime_id);
        self
    }

    pub fn with_param_schema(mut self, schema: serde_json::Value) -> Self {
        self.param_schema = Some(schema);
        self
    }

    pub fn with_out_schema(mut self, schema: serde_json::Value) -> Self {
        self.out_schema = Some(schema);
        self
    }

    pub async fn create(self, pool: &PgPool) -> Result<Action> {
        let input = action::CreateActionInput {
            pack: self.pack_id,
            pack_ref: self.pack_ref,
            r#ref: self.r#ref,
            label: self.label,
            description: Some(self.description),
            entrypoint: self.entrypoint,
            runtime: self.runtime,
            enabled: true,
            runtime_version_constraint: None,
            required_worker_runtimes: serde_json::json!({}),
            worker_selector: serde_json::json!({}),
            worker_tolerations: serde_json::json!([]),
            worker_affinity: serde_json::json!({}),
            param_schema: self.param_schema,
            out_schema: self.out_schema,
            is_adhoc: false,
            accesses_mcp: false,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
        };

        ActionRepository::create(pool, input).await
    }
}

/// Fixture builder for Trigger
pub struct TriggerFixture {
    pub pack_id: Option<i64>,
    pub pack_ref: Option<String>,
    pub r#ref: String,
    pub label: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub param_schema: Option<serde_json::Value>,
    pub out_schema: Option<serde_json::Value>,
}

impl TriggerFixture {
    /// Create a new trigger fixture with the given pack and trigger name
    pub fn new(pack_id: Option<i64>, pack_ref: Option<String>, ref_name: &str) -> Self {
        let full_ref = if let Some(p_ref) = &pack_ref {
            format!("{}.{}", p_ref, ref_name)
        } else {
            format!("core.{}", ref_name)
        };

        Self {
            pack_id,
            pack_ref,
            r#ref: full_ref,
            label: ref_name.replace('_', " ").to_string(),
            description: Some(format!("Test trigger: {}", ref_name)),
            enabled: true,
            param_schema: None,
            out_schema: None,
        }
    }

    /// Create a new trigger fixture with a unique name to avoid test collisions
    ///
    /// This is the recommended constructor for parallel test execution.
    pub fn new_unique(pack_id: Option<i64>, pack_ref: Option<String>, base_name: &str) -> Self {
        let unique_name = unique_trigger_name(base_name);
        let full_ref = if let Some(p_ref) = &pack_ref {
            format!("{}.{}", p_ref, unique_name)
        } else {
            format!("core.{}", unique_name)
        };

        Self {
            pack_id,
            pack_ref,
            r#ref: full_ref,
            label: base_name.replace('_', " ").to_string(),
            description: Some(format!("Test trigger: {}", base_name)),
            enabled: true,
            param_schema: None,
            out_schema: None,
        }
    }

    pub fn with_label(mut self, label: &str) -> Self {
        self.label = label.to_string();
        self
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_param_schema(mut self, schema: serde_json::Value) -> Self {
        self.param_schema = Some(schema);
        self
    }

    pub fn with_out_schema(mut self, schema: serde_json::Value) -> Self {
        self.out_schema = Some(schema);
        self
    }

    pub async fn create(self, pool: &PgPool) -> Result<Trigger> {
        let input = trigger::CreateTriggerInput {
            r#ref: self.r#ref,
            pack: self.pack_id,
            pack_ref: self.pack_ref,
            label: self.label,
            description: self.description,
            enabled: self.enabled,
            param_schema: self.param_schema,
            out_schema: self.out_schema,
            sensor: None,
            sensor_ref: None,
            is_adhoc: false,
        };

        TriggerRepository::create(pool, input).await
    }
}

/// Fixture builder for Event
pub struct EventFixture {
    pub trigger_id: Option<i64>,
    pub trigger_ref: String,
    pub config: Option<serde_json::Value>,
    pub payload: Option<serde_json::Value>,
    pub source: Option<i64>,
    pub source_ref: Option<String>,
    pub rule: Option<i64>,
    pub rule_ref: Option<String>,
}

impl EventFixture {
    /// Create a new event fixture with the given trigger
    pub fn new(trigger_id: Option<i64>, trigger_ref: &str) -> Self {
        Self {
            trigger_id,
            trigger_ref: trigger_ref.to_string(),
            config: None,
            payload: None,
            source: None,
            source_ref: None,
            rule: None,
            rule_ref: None,
        }
    }

    /// Create a new event fixture with a unique trigger ref
    pub fn new_unique(trigger_id: Option<i64>, base_ref: &str) -> Self {
        let unique_ref = unique_event_ref(base_ref);
        Self {
            trigger_id,
            trigger_ref: unique_ref,
            config: None,
            payload: None,
            source: None,
            source_ref: None,
            rule: None,
            rule_ref: None,
        }
    }

    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn with_source(mut self, source_id: i64, source_ref: &str) -> Self {
        self.source = Some(source_id);
        self.source_ref = Some(source_ref.to_string());
        self
    }

    pub async fn create(self, pool: &PgPool) -> Result<event::Event> {
        use attune_common::repositories::event::{CreateEventInput, EventRepository};

        let input = CreateEventInput {
            trigger: self.trigger_id,
            trigger_ref: self.trigger_ref,
            config: self.config,
            payload: self.payload,
            source: self.source,
            source_ref: self.source_ref,
            rule: self.rule,
            rule_ref: self.rule_ref,
        };

        EventRepository::create(pool, input).await
    }
}

/// Fixture builder for Enforcement
pub struct EnforcementFixture {
    pub rule_id: Option<i64>,
    pub rule_ref: String,
    pub trigger_ref: String,
    pub config: Option<serde_json::Value>,
    pub event_id: Option<i64>,
    pub status: enums::EnforcementStatus,
    pub payload: serde_json::Value,
    pub condition: enums::EnforcementCondition,
    pub conditions: serde_json::Value,
}

impl EnforcementFixture {
    /// Create a new enforcement fixture
    pub fn new(rule_id: Option<i64>, rule_ref: &str, trigger_ref: &str) -> Self {
        Self {
            rule_id,
            rule_ref: rule_ref.to_string(),
            trigger_ref: trigger_ref.to_string(),
            config: None,
            event_id: None,
            status: enums::EnforcementStatus::Created,
            payload: json!({}),
            condition: enums::EnforcementCondition::All,
            conditions: json!([]),
        }
    }

    /// Create a new enforcement fixture with unique refs
    pub fn new_unique(rule_id: Option<i64>, base_rule_ref: &str, base_trigger_ref: &str) -> Self {
        let unique_rule_ref = unique_enforcement_ref(base_rule_ref);
        let unique_trigger_ref = unique_event_ref(base_trigger_ref);
        Self {
            rule_id,
            rule_ref: unique_rule_ref,
            trigger_ref: unique_trigger_ref,
            config: None,
            event_id: None,
            status: enums::EnforcementStatus::Created,
            payload: json!({}),
            condition: enums::EnforcementCondition::All,
            conditions: json!([]),
        }
    }

    pub fn with_config(mut self, config: serde_json::Value) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_event(mut self, event_id: i64) -> Self {
        self.event_id = Some(event_id);
        self
    }

    pub fn with_status(mut self, status: enums::EnforcementStatus) -> Self {
        self.status = status;
        self
    }

    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }

    pub fn with_condition(mut self, condition: enums::EnforcementCondition) -> Self {
        self.condition = condition;
        self
    }

    pub fn with_conditions(mut self, conditions: serde_json::Value) -> Self {
        self.conditions = conditions;
        self
    }

    pub async fn create(self, pool: &PgPool) -> Result<event::Enforcement> {
        use attune_common::repositories::event::{CreateEnforcementInput, EnforcementRepository};

        let input = CreateEnforcementInput {
            rule: self.rule_id,
            rule_ref: self.rule_ref,
            trigger_ref: self.trigger_ref,
            config: self.config,
            event: self.event_id,
            status: self.status,
            payload: self.payload,
            condition: self.condition,
            conditions: self.conditions,
        };

        EnforcementRepository::create(pool, input).await
    }
}

/// Fixture builder for Inquiry
pub struct InquiryFixture {
    pub execution_id: i64,
    pub prompt: String,
    pub response_schema: Option<serde_json::Value>,
    pub assigned_to: Option<i64>,
    pub status: enums::InquiryStatus,
    pub response: Option<serde_json::Value>,
    pub timeout_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl InquiryFixture {
    /// Create a new inquiry fixture for the given execution
    pub fn new(execution_id: i64, prompt: &str) -> Self {
        Self {
            execution_id,
            prompt: prompt.to_string(),
            response_schema: None,
            assigned_to: None,
            status: enums::InquiryStatus::Pending,
            response: None,
            timeout_at: None,
        }
    }

    /// Create a new inquiry fixture with a unique prompt
    pub fn new_unique(execution_id: i64, base_prompt: &str) -> Self {
        let unique_prompt = format!("{}_{}", base_prompt, unique_test_id());
        Self {
            execution_id,
            prompt: unique_prompt,
            response_schema: None,
            assigned_to: None,
            status: enums::InquiryStatus::Pending,
            response: None,
            timeout_at: None,
        }
    }

    pub fn with_response_schema(mut self, schema: serde_json::Value) -> Self {
        self.response_schema = Some(schema);
        self
    }

    pub fn with_assigned_to(mut self, identity_id: i64) -> Self {
        self.assigned_to = Some(identity_id);
        self
    }

    pub fn with_status(mut self, status: enums::InquiryStatus) -> Self {
        self.status = status;
        self
    }

    pub fn with_response(mut self, response: serde_json::Value) -> Self {
        self.response = Some(response);
        self
    }

    pub fn with_timeout_at(mut self, timeout_at: chrono::DateTime<chrono::Utc>) -> Self {
        self.timeout_at = Some(timeout_at);
        self
    }

    pub async fn create(self, pool: &PgPool) -> Result<inquiry::Inquiry> {
        use attune_common::repositories::inquiry::{CreateInquiryInput, InquiryRepository};

        let input = CreateInquiryInput {
            execution: self.execution_id,
            prompt: self.prompt,
            response_schema: self.response_schema,
            assigned_to: self.assigned_to,
            status: self.status,
            response: self.response,
            timeout_at: self.timeout_at,
        };

        InquiryRepository::create(pool, input).await
    }
}

/// Fixture builder for Identity
pub struct IdentityFixture {
    pub login: String,
    pub display_name: Option<String>,
    pub attributes: serde_json::Value,
}

impl IdentityFixture {
    /// Create a new identity fixture with the given login
    pub fn new(login: &str) -> Self {
        Self {
            login: login.to_string(),
            display_name: Some(login.to_string()),
            attributes: json!({}),
        }
    }

    /// Create a new identity fixture with a unique login to avoid test collisions
    pub fn new_unique(base_login: &str) -> Self {
        let unique_login = unique_identity_username(base_login);
        Self {
            login: unique_login,
            display_name: Some(base_login.to_string()),
            attributes: json!({}),
        }
    }

    pub fn with_display_name(mut self, display_name: &str) -> Self {
        self.display_name = Some(display_name.to_string());
        self
    }

    pub fn with_attributes(mut self, attributes: serde_json::Value) -> Self {
        self.attributes = attributes;
        self
    }

    pub async fn create(self, pool: &PgPool) -> Result<Identity> {
        let input = identity::CreateIdentityInput {
            login: self.login,
            display_name: self.display_name,
            password_hash: None,
            attributes: self.attributes,
        };

        IdentityRepository::create(pool, input).await
    }
}

/// Fixture builder for Runtime
pub struct RuntimeFixture {
    pub pack_id: Option<i64>,
    pub pack_ref: Option<String>,
    pub r#ref: String,
    pub description: Option<String>,
    pub name: String,
    pub distributions: serde_json::Value,
    pub installation: Option<serde_json::Value>,
    pub execution_config: serde_json::Value,
}

impl RuntimeFixture {
    /// Create a new runtime fixture with the given pack and name
    pub fn new(pack_id: Option<i64>, pack_ref: Option<String>, name: &str) -> Self {
        let full_ref = if let Some(p_ref) = &pack_ref {
            format!("{}.{}", p_ref, name)
        } else {
            format!("core.{}", name)
        };

        Self {
            pack_id,
            pack_ref,
            r#ref: full_ref,
            description: Some(format!("Test runtime: {}", name)),
            name: name.to_string(),
            distributions: json!({
                "linux": { "supported": true },
                "darwin": { "supported": true }
            }),
            installation: None,
            execution_config: json!({
                "interpreter": {
                    "binary": "/bin/bash",
                    "args": [],
                    "file_extension": ".sh"
                }
            }),
        }
    }

    /// Create a new runtime fixture with a unique name to avoid test collisions
    pub fn new_unique(pack_id: Option<i64>, pack_ref: Option<String>, base_name: &str) -> Self {
        let unique_name = unique_runtime_name(base_name);

        let full_ref = if let Some(p_ref) = &pack_ref {
            format!("{}.{}", p_ref, unique_name)
        } else {
            format!("core.{}", unique_name)
        };

        Self {
            pack_id,
            pack_ref,
            r#ref: full_ref,
            description: Some(format!("Test runtime: {}", base_name)),
            name: unique_name,
            distributions: json!({
                "linux": { "supported": true },
                "darwin": { "supported": true }
            }),
            installation: None,
            execution_config: json!({
                "interpreter": {
                    "binary": "/bin/bash",
                    "args": [],
                    "file_extension": ".sh"
                }
            }),
        }
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    pub fn with_distributions(mut self, distributions: serde_json::Value) -> Self {
        self.distributions = distributions;
        self
    }

    pub fn with_installation(mut self, installation: serde_json::Value) -> Self {
        self.installation = Some(installation);
        self
    }

    pub async fn create(self, pool: &PgPool) -> Result<Runtime> {
        let input = runtime::CreateRuntimeInput {
            r#ref: self.r#ref,
            pack: self.pack_id,
            pack_ref: self.pack_ref,
            description: self.description,
            name: self.name,
            aliases: vec![],
            distributions: self.distributions,
            installation: self.installation,
            execution_config: self.execution_config,
            auto_detected: false,
            detection_config: serde_json::json!({}),
        };

        RuntimeRepository::create(pool, input).await
    }
}

/// Fixture builder for Sensor
pub struct SensorFixture {
    pub pack_id: Option<i64>,
    pub pack_ref: Option<String>,
    pub r#ref: String,
    pub label: String,
    pub description: String,
    pub entrypoint: String,
    pub runtime_id: i64,
    pub runtime_ref: String,
    pub enabled: bool,
    pub param_schema: Option<serde_json::Value>,
}

impl SensorFixture {
    /// Create a new sensor fixture with the given pack, runtime and sensor name
    pub fn new(
        pack_id: Option<i64>,
        pack_ref: Option<String>,
        runtime_id: i64,
        runtime_ref: String,
        sensor_name: &str,
    ) -> Self {
        let full_ref = if let Some(p_ref) = &pack_ref {
            format!("{}.{}", p_ref, sensor_name)
        } else {
            format!("core.{}", sensor_name)
        };

        Self {
            pack_id,
            pack_ref,
            r#ref: full_ref,
            label: sensor_name.replace('_', " ").to_string(),
            description: format!("Test sensor: {}", sensor_name),
            entrypoint: format!("sensors/{}.py", sensor_name),
            runtime_id,
            runtime_ref,
            enabled: true,
            param_schema: None,
        }
    }

    /// Create a new sensor fixture with a unique name to avoid test collisions
    pub fn new_unique(
        pack_id: Option<i64>,
        pack_ref: Option<String>,
        runtime_id: i64,
        runtime_ref: String,
        base_name: &str,
    ) -> Self {
        let unique_name = unique_sensor_name(base_name);
        let full_ref = if let Some(p_ref) = &pack_ref {
            format!("{}.{}", p_ref, unique_name)
        } else {
            format!("core.{}", unique_name)
        };

        Self {
            pack_id,
            pack_ref,
            r#ref: full_ref,
            label: base_name.replace('_', " ").to_string(),
            description: format!("Test sensor: {}", base_name),
            entrypoint: format!("sensors/{}.py", base_name),
            runtime_id,
            runtime_ref,
            enabled: true,
            param_schema: None,
        }
    }

    pub fn with_label(mut self, label: &str) -> Self {
        self.label = label.to_string();
        self
    }

    pub fn with_description(mut self, description: &str) -> Self {
        self.description = description.to_string();
        self
    }

    pub fn with_entrypoint(mut self, entrypoint: &str) -> Self {
        self.entrypoint = entrypoint.to_string();
        self
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_param_schema(mut self, schema: serde_json::Value) -> Self {
        self.param_schema = Some(schema);
        self
    }

    pub async fn create(self, pool: &PgPool) -> Result<Sensor> {
        use attune_common::repositories::trigger::CreateSensorInput;

        let input = CreateSensorInput {
            r#ref: self.r#ref,
            pack: self.pack_id,
            pack_ref: self.pack_ref,
            label: self.label,
            description: Some(self.description),
            entrypoint: self.entrypoint,
            runtime: self.runtime_id,
            runtime_ref: self.runtime_ref,
            runtime_version_constraint: None,
            enabled: self.enabled,
            param_schema: self.param_schema,
            config: None,
            worker_selector: serde_json::json!({}),
            worker_tolerations: serde_json::json!([]),
            worker_affinity: serde_json::json!({}),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
        };

        SensorRepository::create(pool, input).await
    }
}

/// Fixture builder for Key
pub struct KeyFixture {
    pub r#ref: String,
    pub owner_type: enums::OwnerType,
    pub owner: Option<String>,
    pub owner_identity: Option<i64>,
    pub owner_pack: Option<i64>,
    pub owner_pack_ref: Option<String>,
    pub owner_action: Option<i64>,
    pub owner_action_ref: Option<String>,
    pub owner_sensor: Option<i64>,
    pub owner_sensor_ref: Option<String>,
    pub name: String,
    pub encrypted: bool,
    pub encryption_key_hash: Option<String>,
    pub value: serde_json::Value,
}

impl KeyFixture {
    /// Create a new key fixture for system owner
    pub fn new_system(name: &str, value: &str) -> Self {
        Self {
            r#ref: name.to_string(),
            owner_type: enums::OwnerType::System,
            owner: Some("system".to_string()),
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: name.to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: serde_json::json!(value),
        }
    }

    /// Create a new key fixture with unique name for system owner
    pub fn new_system_unique(base_name: &str, value: &str) -> Self {
        let unique_name = unique_key_name(base_name);
        Self {
            r#ref: unique_name.clone(),
            owner_type: enums::OwnerType::System,
            owner: Some("system".to_string()),
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: unique_name,
            encrypted: false,
            encryption_key_hash: None,
            value: serde_json::json!(value),
        }
    }

    /// Create a new key fixture for identity owner
    pub fn new_identity(identity_id: i64, name: &str, value: &str) -> Self {
        Self {
            r#ref: format!("{}.{}", identity_id, name),
            owner_type: enums::OwnerType::Identity,
            owner: Some(identity_id.to_string()),
            owner_identity: Some(identity_id),
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: name.to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: serde_json::json!(value),
        }
    }

    /// Create a new key fixture with unique name for identity owner
    pub fn new_identity_unique(identity_id: i64, base_name: &str, value: &str) -> Self {
        let unique_name = unique_key_name(base_name);
        Self {
            r#ref: format!("{}.{}", identity_id, unique_name),
            owner_type: enums::OwnerType::Identity,
            owner: Some(identity_id.to_string()),
            owner_identity: Some(identity_id),
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: unique_name,
            encrypted: false,
            encryption_key_hash: None,
            value: serde_json::json!(value),
        }
    }

    /// Create a new key fixture for pack owner
    pub fn new_pack(pack_id: i64, pack_ref: &str, name: &str, value: &str) -> Self {
        Self {
            r#ref: format!("{}.{}", pack_ref, name),
            owner_type: enums::OwnerType::Pack,
            owner: Some(pack_id.to_string()),
            owner_identity: None,
            owner_pack: Some(pack_id),
            owner_pack_ref: Some(pack_ref.to_string()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: name.to_string(),
            encrypted: false,
            encryption_key_hash: None,
            value: serde_json::json!(value),
        }
    }

    /// Create a new key fixture with unique name for pack owner
    pub fn new_pack_unique(pack_id: i64, pack_ref: &str, base_name: &str, value: &str) -> Self {
        let unique_name = unique_key_name(base_name);
        Self {
            r#ref: format!("{}.{}", pack_ref, unique_name),
            owner_type: enums::OwnerType::Pack,
            owner: Some(pack_id.to_string()),
            owner_identity: None,
            owner_pack: Some(pack_id),
            owner_pack_ref: Some(pack_ref.to_string()),
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: unique_name,
            encrypted: false,
            encryption_key_hash: None,
            value: serde_json::json!(value),
        }
    }

    pub fn with_encrypted(mut self, encrypted: bool) -> Self {
        self.encrypted = encrypted;
        self
    }

    pub fn with_encryption_key_hash(mut self, hash: &str) -> Self {
        self.encryption_key_hash = Some(hash.to_string());
        self
    }

    pub fn with_value(mut self, value: &str) -> Self {
        self.value = serde_json::json!(value);
        self
    }

    pub async fn create(self, pool: &PgPool) -> Result<Key> {
        let input = key::CreateKeyInput {
            r#ref: self.r#ref,
            owner_type: self.owner_type,
            owner: self.owner,
            owner_identity: self.owner_identity,
            owner_pack: self.owner_pack,
            owner_pack_ref: self.owner_pack_ref,
            owner_action: self.owner_action,
            owner_action_ref: self.owner_action_ref,
            owner_sensor: self.owner_sensor,
            owner_sensor_ref: self.owner_sensor_ref,
            name: self.name,
            encrypted: self.encrypted,
            encryption_key_hash: self.encryption_key_hash,
            value: self.value,
        };

        KeyRepository::create(pool, input).await
    }
}
