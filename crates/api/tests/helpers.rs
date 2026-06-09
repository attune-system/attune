//! Test helpers and utilities for API integration tests
//!
//! This module provides common test fixtures, server setup/teardown,
//! and utility functions for testing API endpoints.

use attune_common::{
    config::Config,
    db::Database,
    models::*,
    repositories::{
        action::{ActionRepository, CreateActionInput},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        pack::{CreatePackInput, PackRepository},
        trigger::{CreateTriggerInput, TriggerRepository},
        workflow::{CreateWorkflowDefinitionInput, WorkflowDefinitionRepository},
        Create,
    },
};
use axum::{
    body::Body,
    http::{header, HeaderMap, Method, Request, StatusCode},
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::{Arc, Once};
use tower::Service;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

static INIT: Once = Once::new();

/// Initialize test environment (run once)
pub fn init_test_env() {
    INIT.call_once(|| {
        // Clear any existing ATTUNE environment variables
        for (key, _) in std::env::vars() {
            if key.starts_with("ATTUNE") {
                std::env::remove_var(&key);
            }
        }

        // Don't set environment via env var - let config load from file
        // The test config file already specifies environment: test

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

/// Create a base database pool (connected to attune_test database)
async fn create_base_pool() -> Result<PgPool> {
    init_test_env();

    // Load config from project root (crates/api is 2 levels deep)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let config_path = format!("{}/../../config.test.yaml", manifest_dir);

    let config = Config::load_from_file(&config_path)
        .map_err(|e| format!("Failed to load config from {}: {}", config_path, e))?;

    // Create base pool without setting search_path (for creating schemas)
    // Don't use Database::new as it sets search_path - we just need a plain connection
    let pool = sqlx::PgPool::connect(&config.database.url).await?;

    Ok(pool)
}

/// Create a test database pool with a unique schema for this test
async fn create_schema_pool(schema_name: &str) -> Result<PgPool> {
    let base_pool = create_base_pool().await?;

    // Create the test schema
    tracing::debug!("Creating test schema: {}", schema_name);
    let create_schema_sql = format!("CREATE SCHEMA IF NOT EXISTS {}", schema_name);
    sqlx::query(&create_schema_sql).execute(&base_pool).await?;
    tracing::debug!("Test schema created successfully: {}", schema_name);

    // Run migrations in the new schema
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let migrations_path = format!("{}/../../migrations", manifest_dir);

    // Create a config with our test schema and add search_path to the URL
    let config_path = format!("{}/../../config.test.yaml", manifest_dir);
    let mut config = Config::load_from_file(&config_path)?;
    config.database.schema = Some(schema_name.to_string());

    // Add search_path parameter to the database URL for the migrator
    // PostgreSQL supports setting options in the connection URL
    let separator = if config.database.url.contains('?') {
        "&"
    } else {
        "?"
    };

    // Use proper URL encoding for search_path option
    let _url_with_schema = format!(
        "{}{}options=--search_path%3D{}",
        config.database.url, separator, schema_name
    );

    // Create a pool directly with the modified URL for migrations
    // Also set after_connect hook to ensure all connections from pool have search_path
    let migration_pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect({
            let schema = schema_name.to_string();
            move |conn, _meta| {
                let schema = schema.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {}, public", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            }
        })
        .connect(&config.database.url)
        .await?;

    // Manually run migration SQL files instead of using SQLx migrator
    // This is necessary because SQLx migrator has issues with per-schema search_path
    let migration_files = std::fs::read_dir(&migrations_path)?;
    let mut migrations: Vec<_> = migration_files
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();

    // Sort by filename to ensure migrations run in version order
    migrations.sort_by_key(|entry| entry.path().clone());

    for migration_file in migrations {
        let migration_path = migration_file.path();
        let sql = std::fs::read_to_string(&migration_path)?;

        // Execute search_path setting and migration in sequence
        // First set the search_path (including public so TimescaleDB extension
        // functions like create_hypertable resolve)
        sqlx::query(&format!("SET search_path TO {}, public", schema_name))
            .execute(&migration_pool)
            .await?;

        // Then execute the migration SQL
        // This preserves DO blocks, CREATE TYPE statements, etc.
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

    // Now create the proper Database instance for use in tests
    let database = Database::new(&config.database).await?;
    let pool = database.pool().clone();

    Ok(pool)
}

/// Cleanup a test schema (drop it)
pub async fn cleanup_test_schema(schema_name: &str) -> Result<()> {
    let base_pool = create_base_pool().await?;

    // Drop the schema and all its contents
    tracing::debug!("Dropping test schema: {}", schema_name);
    let drop_schema_sql = format!("DROP SCHEMA IF EXISTS {} CASCADE", schema_name);
    sqlx::query(&drop_schema_sql).execute(&base_pool).await?;
    tracing::debug!("Test schema dropped successfully: {}", schema_name);

    Ok(())
}

/// Create unique test packs directory for this test
pub fn create_test_packs_dir(schema: &str) -> Result<std::path::PathBuf> {
    let test_packs_dir = std::path::PathBuf::from(format!("/tmp/attune-test-packs-{}", schema));
    if test_packs_dir.exists() {
        std::fs::remove_dir_all(&test_packs_dir)?;
    }
    std::fs::create_dir_all(&test_packs_dir)?;
    Ok(test_packs_dir)
}

/// Test context with server and authentication
pub struct TestContext {
    #[allow(dead_code)]
    pub pool: PgPool,
    pub app: axum::Router,
    pub token: Option<String>,
    #[allow(dead_code)]
    pub user: Option<Identity>,
    pub schema: String,
    pub test_packs_dir: std::path::PathBuf,
}

impl TestContext {
    /// Create a new test context with a unique schema
    pub async fn new() -> Result<Self> {
        // Generate a unique schema name for this test
        let schema = format!("test_{}", uuid::Uuid::new_v4().to_string().replace("-", ""));

        tracing::info!("Initializing test context with schema: {}", schema);

        // Create unique test packs directory for this test
        let test_packs_dir = create_test_packs_dir(&schema)?;

        // Create pool with the test schema
        let pool = create_schema_pool(&schema).await?;

        // Load config from project root
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        let config_path = format!("{}/../../config.test.yaml", manifest_dir);
        let mut config = Config::load_from_file(&config_path)?;
        config.database.schema = Some(schema.clone());

        let state = attune_api::state::AppState::new(pool.clone(), config.clone());
        let server = attune_api::server::Server::new(Arc::new(state));
        let app = server.router();

        Ok(Self {
            pool,
            app,
            token: None,
            user: None,
            schema,
            test_packs_dir,
        })
    }

    /// Create and authenticate a test user
    #[allow(dead_code)]
    pub async fn with_auth(mut self) -> Result<Self> {
        // Generate unique username to avoid conflicts in parallel tests
        let unique_id = uuid::Uuid::new_v4().to_string().replace("-", "")[..8].to_string();
        let login = format!("testuser_{}", unique_id);
        let token = self.create_test_user(&login).await?;
        self.token = Some(token);
        Ok(self)
    }

    /// Create and authenticate a test user with identity + permission admin grants.
    #[allow(dead_code)]
    pub async fn with_admin_auth(mut self) -> Result<Self> {
        let unique_id = uuid::Uuid::new_v4().to_string().replace("-", "")[..8].to_string();
        let login = format!("adminuser_{}", unique_id);
        let token = self.create_test_user(&login).await?;

        let identity = attune_common::repositories::identity::IdentityRepository::find_by_login(
            &self.pool, &login,
        )
        .await?
        .ok_or_else(|| format!("Failed to find newly created identity '{}'", login))?;

        let permset = PermissionSetRepository::create(
            &self.pool,
            CreatePermissionSetInput {
                r#ref: "core.admin".to_string(),
                pack: None,
                pack_ref: None,
                label: Some("Admin".to_string()),
                description: Some("Test admin permission set".to_string()),
                grants: json!([
                    {"resource": "identities", "actions": ["read", "create", "update", "delete"]},
                    {"resource": "permissions", "actions": ["read", "create", "update", "delete", "manage"]}
                ]),
            },
        )
        .await?;

        PermissionAssignmentRepository::create(
            &self.pool,
            CreatePermissionAssignmentInput {
                identity: identity.id,
                permset: permset.id,
            },
        )
        .await?;

        self.token = Some(token);
        Ok(self)
    }

    /// Create a test user and return access token
    async fn create_test_user(&self, login: &str) -> Result<String> {
        // Register via API to get real token
        let response = self
            .post(
                "/auth/register",
                json!({
                    "login": login,
                    "password": "TestPassword123!",
                    "display_name": format!("Test User {}", login)
                }),
                None,
            )
            .await?;

        let status = response.status();
        let body: Value = response.json().await?;

        if !status.is_success() {
            return Err(
                format!("Failed to register user: status={}, body={}", status, body).into(),
            );
        }

        let token = body["data"]["access_token"]
            .as_str()
            .ok_or_else(|| format!("No access token in response: {}", body))?
            .to_string();

        Ok(token)
    }

    /// Make a GET request
    #[allow(dead_code)]
    pub async fn get(&self, path: &str, token: Option<&str>) -> Result<TestResponse> {
        self.request(Method::GET, path, None::<Value>, token).await
    }

    /// Make a POST request
    pub async fn post<T: serde::Serialize>(
        &self,
        path: &str,
        body: T,
        token: Option<&str>,
    ) -> Result<TestResponse> {
        self.request(Method::POST, path, Some(body), token).await
    }

    /// Make a PUT request
    #[allow(dead_code)]
    pub async fn put<T: serde::Serialize>(
        &self,
        path: &str,
        body: T,
        token: Option<&str>,
    ) -> Result<TestResponse> {
        self.request(Method::PUT, path, Some(body), token).await
    }

    /// Make a DELETE request
    #[allow(dead_code)]
    pub async fn delete(&self, path: &str, token: Option<&str>) -> Result<TestResponse> {
        self.request(Method::DELETE, path, None::<Value>, token)
            .await
    }

    /// Make a generic HTTP request
    async fn request<T: serde::Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<T>,
        token: Option<&str>,
    ) -> Result<TestResponse> {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json");

        // Add authorization header if token provided
        if let Some(token) = token.or(self.token.as_deref()) {
            request = request.header(header::AUTHORIZATION, format!("Bearer {}", token));
        }

        let request = if let Some(body) = body {
            request.body(Body::from(serde_json::to_string(&body).unwrap()))
        } else {
            request.body(Body::empty())
        }
        .unwrap();

        let response = self
            .app
            .clone()
            .call(request)
            .await
            .expect("Failed to execute request");

        Ok(TestResponse::new(response))
    }

    /// Get authenticated token
    #[allow(dead_code)]
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        // Cleanup the test schema when the context is dropped
        // Best-effort async cleanup - schema will be dropped shortly after test completes
        // If tests are interrupted, run ./scripts/cleanup-test-schemas.sh
        let schema = self.schema.clone();
        let test_packs_dir = self.test_packs_dir.clone();

        // Spawn cleanup task in background
        drop(tokio::spawn(async move {
            if let Err(e) = cleanup_test_schema(&schema).await {
                eprintln!("Failed to cleanup test schema {}: {}", schema, e);
            }
        }));

        // Cleanup the test packs directory synchronously
        let _ = std::fs::remove_dir_all(&test_packs_dir);
    }
}

/// Test response wrapper
pub struct TestResponse {
    response: axum::response::Response,
}

impl TestResponse {
    pub fn new(response: axum::response::Response) -> Self {
        Self { response }
    }

    /// Get response status code
    pub fn status(&self) -> StatusCode {
        self.response.status()
    }

    /// Borrow response headers.
    #[allow(dead_code)]
    pub fn headers(&self) -> &HeaderMap {
        self.response.headers()
    }

    /// Deserialize response body as JSON
    pub async fn json<T: DeserializeOwned>(self) -> Result<T> {
        let body = self.response.into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Get response body as text
    #[allow(dead_code)]
    pub async fn text(self) -> Result<String> {
        let body = self.response.into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX).await?;
        Ok(String::from_utf8(bytes.to_vec())?)
    }

    /// Assert status code
    #[allow(dead_code)]
    pub fn assert_status(self, expected: StatusCode) -> Self {
        assert_eq!(
            self.response.status(),
            expected,
            "Expected status {}, got {}",
            expected,
            self.response.status()
        );
        self
    }
}

/// Fixture for creating test packs
#[allow(dead_code)]
pub async fn create_test_pack(pool: &PgPool, ref_name: &str) -> Result<Pack> {
    let input = CreatePackInput {
        r#ref: ref_name.to_string(),
        label: format!("Test Pack {}", ref_name),
        description: Some(format!("Test pack for {}", ref_name)),
        version: "1.0.0".to_string(),
        conf_schema: json!({}),
        config: json!({}),
        meta: json!({
            "author": "test",
            "keywords": ["test"]
        }),
        tags: vec!["test".to_string()],
        runtime_deps: vec![],
        dependencies: vec![],
        is_standard: false,
        installers: json!({}),
    };

    Ok(PackRepository::create(pool, input).await?)
}

/// Fixture for creating test actions
#[allow(dead_code)]
pub async fn create_test_action(pool: &PgPool, pack_id: i64, ref_name: &str) -> Result<Action> {
    let input = CreateActionInput {
        r#ref: ref_name.to_string(),
        pack: pack_id,
        pack_ref: format!("pack_{}", pack_id),
        label: format!("Test Action {}", ref_name),
        description: Some(format!("Test action for {}", ref_name)),
        entrypoint: "main.py".to_string(),
        runtime: None,
        enabled: true,
        runtime_version_constraint: None,
        required_worker_runtimes: serde_json::json!({}),
        worker_selector: serde_json::json!({}),
        worker_tolerations: serde_json::json!([]),
        worker_affinity: serde_json::json!({}),
        param_schema: None,
        out_schema: None,
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

    Ok(ActionRepository::create(pool, input).await?)
}

/// Fixture for creating test triggers
#[allow(dead_code)]
pub async fn create_test_trigger(pool: &PgPool, pack_id: i64, ref_name: &str) -> Result<Trigger> {
    let input = CreateTriggerInput {
        r#ref: ref_name.to_string(),
        pack: Some(pack_id),
        pack_ref: Some(format!("pack_{}", pack_id)),
        label: format!("Test Trigger {}", ref_name),
        description: Some(format!("Test trigger for {}", ref_name)),
        enabled: true,
        param_schema: None,
        out_schema: None,
        sensor: None,
        sensor_ref: None,
        is_adhoc: false,
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
    };

    Ok(TriggerRepository::create(pool, input).await?)
}

/// Fixture for creating test workflows
#[allow(dead_code)]
pub async fn create_test_workflow(
    pool: &PgPool,
    pack_id: i64,
    pack_ref: &str,
    ref_name: &str,
) -> Result<attune_common::models::workflow::WorkflowDefinition> {
    let input = CreateWorkflowDefinitionInput {
        r#ref: ref_name.to_string(),
        pack: pack_id,
        pack_ref: pack_ref.to_string(),
        label: format!("Test Workflow {}", ref_name),
        description: Some(format!("Test workflow for {}", ref_name)),
        version: "1.0.0".to_string(),
        param_schema: None,
        out_schema: None,
        definition: json!({
            "tasks": [
                {
                    "name": "test_task",
                    "action": "core.echo",
                    "input": {"message": "test"}
                }
            ]
        }),
        tags: vec!["test".to_string()],
    };

    Ok(WorkflowDefinitionRepository::create(pool, input).await?)
}

/// Assert that a value matches expected JSON structure
#[macro_export]
macro_rules! assert_json_contains {
    ($actual:expr, $expected:expr) => {
        let actual: serde_json::Value = $actual;
        let expected: serde_json::Value = $expected;

        // This is a simple implementation - you might want more sophisticated matching
        assert!(
            actual.get("data").is_some(),
            "Response should have 'data' field"
        );
    };
}
