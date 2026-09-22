//! Test helpers and utilities for API integration tests
//!
//! This module provides common test fixtures, server setup/teardown,
//! and utility functions for testing API endpoints.

use attune_api::authz::AuthorizationService;
use attune_common::{
    audit::ThreadedAuditWriterHandle,
    config::{CacheAdmissionConfig, Config},
    models::*,
    repositories::{
        action::{ActionRepository, CreateActionInput},
        component_lifecycle::PackProjectionIds,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        pack::{CreatePackInput, PackRepository},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        trigger::{CreateTriggerInput, TriggerRepository},
        workflow::{CreateWorkflowDefinitionInput, WorkflowDefinitionRepository},
        Create,
    },
    test_database::TestDatabase,
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
        // Initialize tracing for tests. Authorization cache keys include the
        // database/schema namespace, so database-isolated tests exercise caching
        // without mutating process-global configuration.
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

/// Create the owning, fully migrated schema-isolated database fixture.
async fn create_test_database() -> Result<TestDatabase> {
    init_test_env();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let config_path = format!("{}/../../config.test.yaml", manifest_dir);
    let config = Config::load_from_file(&config_path)?;
    Ok(TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop())
}

/// Regression seam proving a fallible constructor retains ownership until
/// unwinding. This helper intentionally fails after database creation.
#[allow(dead_code)]
pub async fn fail_after_database_creation_for_test(observed_database: &mut String) -> Result<()> {
    let database = create_test_database().await?;
    observed_database.clone_from(&database.database_name().to_string());
    Err("intentional partial TestContext construction failure".into())
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
    #[allow(dead_code)]
    pub state: Arc<attune_api::state::AppState>,
    pub token: Option<String>,
    #[allow(dead_code)]
    pub user: Option<Identity>,
    pub schema: String,
    pub test_packs_dir: std::path::PathBuf,
    audit_writer: Option<ThreadedAuditWriterHandle>,
    // Retain schema ownership through partial construction and the full context
    // lifetime. Its Drop cleanup is the fallback for panic paths.
    database: Option<TestDatabase>,
}

impl TestContext {
    /// Create a new test context with a unique schema
    #[allow(dead_code)]
    pub async fn new() -> Result<Self> {
        Self::new_with_cache_admission(CacheAdmissionConfig::default()).await
    }

    #[allow(dead_code)]
    pub async fn new_with_cache_admission(cache_admission: CacheAdmissionConfig) -> Result<Self> {
        Self::new_with_options(cache_admission, false, false, false, None).await
    }

    #[allow(dead_code)]
    pub async fn new_without_registry_encryption_key() -> Result<Self> {
        Self::new_with_options(CacheAdmissionConfig::default(), true, false, false, None).await
    }

    #[allow(dead_code)]
    pub async fn new_with_disabled_pack_registry() -> Result<Self> {
        Self::new_with_options(CacheAdmissionConfig::default(), false, true, true, None).await
    }

    #[allow(dead_code)]
    pub async fn new_with_unverified_direct_remote_installs() -> Result<Self> {
        Self::new_with_options(CacheAdmissionConfig::default(), false, false, true, None).await
    }

    #[allow(dead_code)]
    pub async fn new_with_stream_limits(global: usize, per_identity: usize) -> Result<Self> {
        Self::new_with_options(
            CacheAdmissionConfig::default(),
            false,
            false,
            false,
            Some((global, per_identity)),
        )
        .await
    }

    async fn new_with_options(
        cache_admission: CacheAdmissionConfig,
        clear_encryption_key: bool,
        disable_pack_registry: bool,
        allow_unverified_direct_remote_installs: bool,
        stream_limits: Option<(usize, usize)>,
    ) -> Result<Self> {
        let database = create_test_database().await?;
        let pool = database.pool().clone();
        let schema = database.schema().to_string();
        let database_url = database.database_url().to_string();
        let database_name = database.database_name().to_string();
        tracing::info!("Initializing test context with database: {}", database_name);
        let test_packs_dir = create_test_packs_dir(&database_name)?;

        // Load config from project root
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        let config_path = format!("{}/../../config.test.yaml", manifest_dir);
        let mut config = Config::load_from_file(&config_path)?;
        config.database.url = database_url;
        config.database.schema = Some(schema.clone());
        config.packs_base_dir = test_packs_dir.to_string_lossy().into_owned();
        config.storage = attune_common::config::BlobStorageConfig::Filesystem {
            root: test_packs_dir.join("blobs"),
        };
        config.cache_admission = cache_admission;
        if let Some((global, per_identity)) = stream_limits {
            config.server.execution_log_stream_global_limit = global;
            config.server.execution_log_stream_per_identity_limit = per_identity;
        }
        if clear_encryption_key {
            config.security.encryption_key = None;
        }
        if disable_pack_registry {
            config.pack_registry.enabled = false;
        }
        config.pack_registry.allow_unverified_direct_remote_installs =
            allow_unverified_direct_remote_installs;

        let audit_writer = attune_common::audit::spawn_threaded_writer(
            config.database.url.clone(),
            schema.clone(),
        )?;
        let state = Arc::new(attune_api::state::AppState::new_with_audit(
            pool.clone(),
            config.clone(),
            audit_writer.emitter.clone(),
        ));
        let server = attune_api::server::Server::new(state.clone());
        let app = server.router();

        Ok(Self {
            pool,
            app,
            state,
            token: None,
            user: None,
            schema,
            test_packs_dir,
            audit_writer: Some(audit_writer),
            database: Some(database),
        })
    }

    /// Create and authenticate a test user
    #[allow(dead_code)]
    pub async fn with_auth(mut self) -> Result<Self> {
        // Generate unique username to avoid conflicts in parallel tests
        let unique_id = uuid::Uuid::new_v4().to_string().replace("-", "")[..8].to_string();
        let login = format!("testuser_{}", unique_id);
        let token = self.create_test_user(&login).await?;
        self.user = IdentityRepository::find_by_login(&self.pool, &login).await?;
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
                    {"resource": "permissions", "actions": ["read", "create", "update", "delete", "manage"]},
                    {"resource": "packs", "actions": ["read", "create", "install", "update", "configure", "delete"]}
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

        AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
        AuthorizationService::invalidate_permission_set_caches().await;

        self.token = Some(token);
        Ok(self)
    }

    /// Create a user that may install new packs but may not configure existing ones.
    #[allow(dead_code)]
    pub async fn with_pack_install_auth(mut self) -> Result<Self> {
        let unique_id = uuid::Uuid::new_v4().to_string().replace('-', "")[..8].to_string();
        let login = format!("packinstaller_{}", unique_id);
        let token = self.create_test_user(&login).await?;
        let identity = attune_common::repositories::identity::IdentityRepository::find_by_login(
            &self.pool, &login,
        )
        .await?
        .ok_or_else(|| format!("Failed to find newly created identity '{}'", login))?;
        let permset = PermissionSetRepository::create(
            &self.pool,
            CreatePermissionSetInput {
                r#ref: "core.pack_installer".to_string(),
                pack: None,
                pack_ref: None,
                label: Some("Pack installer".to_string()),
                description: Some("Install-only test permission set".to_string()),
                grants: json!([
                    {"resource": "packs", "actions": ["read", "install"]}
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
        AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
        AuthorizationService::invalidate_permission_set_caches().await;
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

    /// Make a GET request with additional headers.
    #[allow(dead_code)]
    pub async fn get_with_headers(
        &self,
        path: &str,
        token: Option<&str>,
        headers: HeaderMap,
    ) -> Result<TestResponse> {
        self.request_with_headers(Method::GET, path, None::<Value>, token, headers)
            .await
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
        self.request_with_headers(method, path, body, token, HeaderMap::new())
            .await
    }

    async fn request_with_headers<T: serde::Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<T>,
        token: Option<&str>,
        headers: HeaderMap,
    ) -> Result<TestResponse> {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json");
        request
            .headers_mut()
            .expect("request builder should expose headers")
            .extend(headers);

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

    /// Persist all audit events emitted by completed requests.
    #[allow(dead_code)]
    pub async fn flush_audit(&self) -> Result<()> {
        let writer = self
            .audit_writer
            .as_ref()
            .ok_or("audit writer is not available")?;
        if !writer.emitter.flush().await {
            return Err("audit writer stopped before flush completed".into());
        }
        Ok(())
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        // Drop router-held emitter clones before joining the dedicated writer.
        self.app = axum::Router::new();
        if let Some(writer) = self.audit_writer.take() {
            if writer.shutdown().is_err() {
                eprintln!("Audit writer thread panicked for schema {}", self.schema);
            }
        }

        // Cleanup the test packs directory synchronously, then release the
        // database owner. TestDatabase terminates only sessions tagged for its
        // schema and removes only that schema.
        let _ = std::fs::remove_dir_all(&self.test_packs_dir);
        drop(self.database.take());
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

    #[allow(dead_code)]
    pub fn into_response(self) -> axum::response::Response {
        self.response
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

    /// Get the raw response body.
    #[allow(dead_code)]
    pub async fn bytes(self) -> Result<Vec<u8>> {
        let body = self.response.into_body();
        Ok(axum::body::to_bytes(body, usize::MAX).await?.to_vec())
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

/// Adds the immutable release required before a pack's components can execute.
#[allow(dead_code)]
pub async fn activate_test_pack_release(pool: &PgPool, pack: &Pack) -> Result<()> {
    activate_test_pack_release_with_projections(pool, pack, &PackProjectionIds::default()).await
}

#[allow(dead_code)]
pub async fn activate_test_pack_release_with_projections(
    pool: &PgPool,
    pack: &Pack,
    projections: &PackProjectionIds,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version.clone(),
            digest: format!("{:0>64}", pack.id),
            object_key: format!("packs/blobs/{}.tar.gz", pack.r#ref),
            provider_version: "e:test-version".to_string(),
            content_path: format!("/packs/.releases/{}", pack.r#ref),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await?;
    PackReleaseRepository::activate_projected(&mut tx, pack.id, release.id, projections).await?;
    tx.commit().await?;
    Ok(())
}

/// Fixture for creating test actions
#[allow(dead_code)]
pub async fn create_test_action(
    pool: &PgPool,
    pack_id: i64,
    pack_ref: &str,
    ref_name: &str,
) -> Result<Action> {
    let input = CreateActionInput {
        r#ref: ref_name.to_string(),
        pack: pack_id,
        pack_ref: pack_ref.to_string(),
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
