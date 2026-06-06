//! Configuration management for Attune services
//!
//! This module provides configuration loading and validation for all services.
//! Configuration is loaded from YAML files with environment variable overrides.
//!
//! ## Configuration Loading Priority
//!
//! 1. Default YAML file (`config.yaml` or path from `ATTUNE_CONFIG` env var)
//! 2. Environment-specific YAML file (`config.{environment}.yaml`)
//! 3. Environment variables with `ATTUNE__` prefix (e.g., `ATTUNE__DATABASE__URL`)
//!
//! ## Example YAML Configuration
//!
//! ```yaml
//! service_name: attune
//! environment: development
//!
//! database:
//!   url: postgresql://postgres:postgres@localhost:5432/attune
//!   max_connections: 50
//!   min_connections: 5
//!
//! server:
//!   host: 0.0.0.0
//!   port: 8080
//!   cors_origins:
//!     - http://localhost:3000
//!     - http://localhost:5173
//!
//! security:
//!   jwt_secret: your-secret-key-here
//!   jwt_access_expiration: 3600
//!
//! log:
//!   level: info
//!   format: json
//! ```

use config as config_crate;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use utoipa::ToSchema;

/// Custom deserializer for fields that can be either a comma-separated string or an array
mod string_or_vec {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrVec {
            String(String),
            Vec(Vec<String>),
        }

        match StringOrVec::deserialize(deserializer)? {
            StringOrVec::String(s) => {
                // Split by comma and trim whitespace
                Ok(s.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect())
            }
            StringOrVec::Vec(v) => Ok(v),
        }
    }
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// PostgreSQL connection URL
    #[serde(default = "default_database_url")]
    pub url: String,

    /// Maximum number of connections in the pool
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,

    /// Minimum number of connections in the pool
    #[serde(default = "default_min_connections")]
    pub min_connections: u32,

    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connect_timeout: u64,

    /// Idle timeout in seconds
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: u64,

    /// Enable SQL statement logging
    #[serde(default)]
    pub log_statements: bool,

    /// PostgreSQL schema name (defaults to "attune")
    pub schema: Option<String>,
}

fn default_database_url() -> String {
    "postgresql://postgres:postgres@localhost:5432/attune".to_string()
}

fn default_max_connections() -> u32 {
    50
}

fn default_min_connections() -> u32 {
    5
}

fn default_connection_timeout() -> u64 {
    30
}

fn default_idle_timeout() -> u64 {
    600
}

/// Message queue configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageQueueConfig {
    /// AMQP connection URL (RabbitMQ)
    #[serde(default = "default_amqp_url")]
    pub url: String,

    /// Exchange name
    #[serde(default = "default_exchange")]
    pub exchange: String,

    /// Enable dead letter queue
    #[serde(default = "default_true")]
    pub enable_dlq: bool,

    /// Message TTL in seconds
    #[serde(default = "default_message_ttl")]
    pub message_ttl: u64,
}

fn default_amqp_url() -> String {
    "amqp://guest:guest@localhost:5672/%2f".to_string()
}

fn default_exchange() -> String {
    "attune".to_string()
}

fn default_message_ttl() -> u64 {
    3600
}

fn default_true() -> bool {
    true
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Host to bind to
    #[serde(default = "default_host")]
    pub host: String,

    /// Port to bind to
    #[serde(default = "default_port")]
    pub port: u16,

    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout: u64,

    /// Enable CORS
    #[serde(default = "default_true")]
    pub enable_cors: bool,

    /// Allowed origins for CORS
    /// Can be specified as a comma-separated string or array
    #[serde(default, deserialize_with = "string_or_vec::deserialize")]
    pub cors_origins: Vec<String>,

    /// Maximum request body size in bytes
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_request_timeout() -> u64 {
    30
}

fn default_max_body_size() -> usize {
    10 * 1024 * 1024 // 10MB
}

/// Notifier service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifierConfig {
    /// Host to bind to
    #[serde(default = "default_notifier_host")]
    pub host: String,

    /// Port to bind to
    #[serde(default = "default_notifier_port")]
    pub port: u16,

    /// Maximum number of concurrent WebSocket connections
    #[serde(default = "default_max_connections_notifier")]
    pub max_connections: usize,
}

fn default_notifier_host() -> String {
    "0.0.0.0".to_string()
}

fn default_notifier_port() -> u16 {
    8081
}

fn default_max_connections_notifier() -> usize {
    10000
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Log format (json, pretty)
    #[serde(default = "default_log_format")]
    pub format: String,

    /// Enable console logging
    #[serde(default = "default_true")]
    pub console: bool,

    /// Optional log file path
    pub file: Option<PathBuf>,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// JWT secret key
    pub jwt_secret: Option<String>,

    /// JWT access token expiration in seconds
    #[serde(default = "default_jwt_access_expiration")]
    pub jwt_access_expiration: u64,

    /// JWT refresh token expiration in seconds
    #[serde(default = "default_jwt_refresh_expiration")]
    pub jwt_refresh_expiration: u64,

    /// Encryption key for secrets
    pub encryption_key: Option<String>,

    /// Enable authentication
    #[serde(default = "default_true")]
    pub enable_auth: bool,

    /// Allow unauthenticated self-service user registration
    #[serde(default)]
    pub allow_self_registration: bool,

    /// Login page visibility defaults for the web UI.
    #[serde(default)]
    pub login_page: LoginPageConfig,

    /// Optional OpenID Connect configuration for browser login.
    #[serde(default)]
    pub oidc: Option<OidcConfig>,

    /// Optional LDAP configuration for username/password login against a directory.
    #[serde(default)]
    pub ldap: Option<LdapConfig>,
}

fn default_jwt_access_expiration() -> u64 {
    3600 // 1 hour
}

fn default_jwt_refresh_expiration() -> u64 {
    604800 // 7 days
}

/// Web login page configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginPageConfig {
    /// Show the local username/password form by default.
    #[serde(default = "default_true")]
    pub show_local_login: bool,

    /// Show the OIDC/SSO option by default when configured.
    #[serde(default = "default_true")]
    pub show_oidc_login: bool,

    /// Show the LDAP option by default when configured.
    #[serde(default = "default_true")]
    pub show_ldap_login: bool,
}

impl Default for LoginPageConfig {
    fn default() -> Self {
        Self {
            show_local_login: true,
            show_oidc_login: true,
            show_ldap_login: true,
        }
    }
}

/// OpenID Connect configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    /// Enable OpenID Connect login flow.
    #[serde(default)]
    pub enabled: bool,

    /// OpenID Provider discovery document URL.
    /// Required when `enabled` is true; ignored otherwise.
    #[serde(default)]
    pub discovery_url: Option<String>,

    /// Client ID registered with the provider.
    /// Required when `enabled` is true; ignored otherwise.
    #[serde(default)]
    pub client_id: Option<String>,

    /// Provider name used in login-page overrides such as `?auth=<provider_name>`.
    #[serde(default = "default_oidc_provider_name")]
    pub provider_name: String,

    /// User-facing provider label shown on the login page.
    pub provider_label: Option<String>,

    /// Optional icon URL shown beside the provider label on the login page.
    pub provider_icon_url: Option<String>,

    /// Client secret. Required for confidential clients; omit for public clients
    /// that use PKCE-only (e.g., Okta browser-type apps with `pkce_required=true`).
    pub client_secret: Option<String>,

    /// Redirect URI registered with the provider.
    /// Required when `enabled` is true; ignored otherwise.
    #[serde(default)]
    pub redirect_uri: Option<String>,

    /// Optional post-logout redirect URI.
    pub post_logout_redirect_uri: Option<String>,

    /// Optional requested scopes in addition to `openid email profile`.
    #[serde(default)]
    pub scopes: Vec<String>,
}

fn default_oidc_provider_name() -> String {
    "oidc".to_string()
}

/// LDAP authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapConfig {
    /// Enable LDAP login flow.
    #[serde(default)]
    pub enabled: bool,

    /// LDAP server URL (e.g., "ldap://ldap.example.com:389" or "ldaps://ldap.example.com:636").
    /// Required when `enabled` is true; ignored otherwise.
    #[serde(default)]
    pub url: Option<String>,

    /// Bind DN template. Use `{login}` as placeholder for the user-supplied login.
    /// Example: "uid={login},ou=users,dc=example,dc=com"
    /// If not set, an anonymous bind is attempted first to search for the user.
    pub bind_dn_template: Option<String>,

    /// Base DN for user searches when bind_dn_template is not set.
    /// Example: "ou=users,dc=example,dc=com"
    pub user_search_base: Option<String>,

    /// LDAP search filter template. Use `{login}` as placeholder.
    /// Default: "(uid={login})"
    #[serde(default = "default_ldap_user_filter")]
    pub user_filter: String,

    /// DN of a service account used to search for users (required when using search-based auth).
    pub search_bind_dn: Option<String>,

    /// Password for the search service account.
    pub search_bind_password: Option<String>,

    /// LDAP attribute to use as the login name. Default: "uid"
    #[serde(default = "default_ldap_login_attr")]
    pub login_attr: String,

    /// LDAP attribute to use as the email. Default: "mail"
    #[serde(default = "default_ldap_email_attr")]
    pub email_attr: String,

    /// LDAP attribute to use as the display name. Default: "cn"
    #[serde(default = "default_ldap_display_name_attr")]
    pub display_name_attr: String,

    /// LDAP attribute that contains group membership. Default: "memberOf"
    #[serde(default = "default_ldap_group_attr")]
    pub group_attr: String,

    /// Whether to use STARTTLS. Default: false
    #[serde(default)]
    pub starttls: bool,

    /// Whether to skip TLS certificate verification (insecure!). Default: false
    #[serde(default)]
    pub danger_skip_tls_verify: bool,

    /// Provider name used in login-page overrides such as `?auth=<provider_name>`.
    #[serde(default = "default_ldap_provider_name")]
    pub provider_name: String,

    /// User-facing provider label shown on the login page.
    pub provider_label: Option<String>,

    /// Optional icon URL shown beside the provider label on the login page.
    pub provider_icon_url: Option<String>,
}

fn default_ldap_provider_name() -> String {
    "ldap".to_string()
}

fn default_ldap_user_filter() -> String {
    "(uid={login})".to_string()
}

fn default_ldap_login_attr() -> String {
    "uid".to_string()
}

fn default_ldap_email_attr() -> String {
    "mail".to_string()
}

fn default_ldap_display_name_attr() -> String {
    "cn".to_string()
}

fn default_ldap_group_attr() -> String {
    "memberOf".to_string()
}

/// Worker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Worker name/identifier (optional, defaults to hostname)
    pub name: Option<String>,

    /// Worker type (local, remote, container)
    pub worker_type: Option<crate::models::WorkerType>,

    /// Runtime ID this worker is associated with
    pub runtime_id: Option<i64>,

    /// Worker host (optional, defaults to hostname)
    pub host: Option<String>,

    /// Worker port
    pub port: Option<i32>,

    /// Worker capabilities (runtimes, max_concurrent_executions, etc.)
    /// Can be overridden by ATTUNE_WORKER_RUNTIMES environment variable
    pub capabilities: Option<std::collections::HashMap<String, serde_json::Value>>,

    /// Worker placement labels used by executor scheduling constraints.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,

    /// Worker taints that require matching action tolerations.
    #[serde(default)]
    pub taints: Vec<crate::scheduling::WorkerTaint>,

    /// Maximum concurrent tasks
    #[serde(default = "default_max_concurrent_tasks")]
    pub max_concurrent_tasks: usize,

    /// Heartbeat interval in seconds
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: u64,

    /// Task timeout in seconds
    #[serde(default = "default_task_timeout")]
    pub task_timeout: u64,

    /// Maximum stdout size in bytes (default 10MB)
    #[serde(default = "default_max_stdout_bytes")]
    pub max_stdout_bytes: usize,

    /// Maximum stderr size in bytes (default 10MB)
    #[serde(default = "default_max_stderr_bytes")]
    pub max_stderr_bytes: usize,

    /// Retention policy for per-execution stdout/stderr log artifacts (default: keep 7 days).
    #[serde(default = "default_execution_log_retention_policy")]
    pub execution_log_retention_policy: crate::models::enums::RetentionPolicyType,

    /// Retention limit for per-execution stdout/stderr log artifacts (default: 7 days).
    /// Interpreted according to `execution_log_retention_policy`.
    #[serde(default = "default_execution_log_retention_limit")]
    pub execution_log_retention_limit: i32,

    /// Graceful shutdown timeout in seconds
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout: Option<u64>,

    /// Enable log streaming instead of buffering
    #[serde(default = "default_true")]
    pub stream_logs: bool,
}

fn default_max_concurrent_tasks() -> usize {
    10
}

fn default_heartbeat_interval() -> u64 {
    30
}

fn default_shutdown_timeout() -> Option<u64> {
    Some(30)
}

fn default_task_timeout() -> u64 {
    300 // 5 minutes
}

fn default_max_stdout_bytes() -> usize {
    10 * 1024 * 1024 // 10MB
}

fn default_max_stderr_bytes() -> usize {
    10 * 1024 * 1024 // 10MB
}

fn default_execution_log_retention_policy() -> crate::models::enums::RetentionPolicyType {
    crate::models::enums::RetentionPolicyType::Days
}

fn default_execution_log_retention_limit() -> i32 {
    7
}

/// Sensor service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorConfig {
    /// Sensor worker name/identifier (optional, defaults to hostname)
    pub worker_name: Option<String>,

    /// Sensor worker host (optional, defaults to hostname)
    pub host: Option<String>,

    /// Sensor worker capabilities (runtimes, max_concurrent_sensors, etc.)
    /// Can be overridden by ATTUNE_SENSOR_RUNTIMES environment variable
    pub capabilities: Option<std::collections::HashMap<String, serde_json::Value>>,

    /// Sensor worker placement labels used by sensor scheduling constraints.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,

    /// Sensor worker taints that require matching sensor tolerations.
    #[serde(default)]
    pub taints: Vec<crate::scheduling::WorkerTaint>,

    /// Maximum concurrent sensors
    pub max_concurrent_sensors: Option<usize>,

    /// Heartbeat interval in seconds
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: u64,

    /// Sensor poll interval in seconds
    #[serde(default = "default_sensor_poll_interval")]
    pub poll_interval: u64,

    /// Sensor execution timeout in seconds
    #[serde(default = "default_sensor_timeout")]
    pub sensor_timeout: u64,

    /// Graceful shutdown timeout in seconds
    #[serde(default = "default_sensor_shutdown_timeout")]
    pub shutdown_timeout: u64,
}

fn default_sensor_poll_interval() -> u64 {
    30
}

fn default_sensor_timeout() -> u64 {
    30
}

fn default_sensor_shutdown_timeout() -> u64 {
    30
}

/// Pack registry index configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIndexConfig {
    /// Registry index URL (https://, http://, or file://)
    pub url: String,

    /// Registry priority (lower number = higher priority)
    #[serde(default = "default_registry_priority")]
    pub priority: u32,

    /// Whether this registry is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Human-readable registry name
    pub name: Option<String>,

    /// Custom HTTP headers for authenticated registries
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
}

fn default_registry_priority() -> u32 {
    100
}

/// Pack registry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackRegistryConfig {
    /// Enable pack registry system
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// List of registry indices
    #[serde(default)]
    pub indices: Vec<RegistryIndexConfig>,

    /// Cache TTL in seconds (how long to cache index files)
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl: u64,

    /// Enable registry index caching
    #[serde(default = "default_true")]
    pub cache_enabled: bool,

    /// Download timeout in seconds
    #[serde(default = "default_registry_timeout")]
    pub timeout: u64,

    /// Verify checksums during installation
    #[serde(default = "default_true")]
    pub verify_checksums: bool,

    /// Additional remote hosts allowed for pack archive/git downloads.
    /// Hosts from enabled registry indices are implicitly allowed.
    #[serde(default)]
    pub allowed_source_hosts: Vec<String>,

    /// Allow HTTP (non-HTTPS) registries
    #[serde(default)]
    pub allow_http: bool,
}

fn default_cache_ttl() -> u64 {
    3600 // 1 hour
}

fn default_registry_timeout() -> u64 {
    120 // 2 minutes
}

impl Default for PackRegistryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            indices: Vec::new(),
            cache_ttl: default_cache_ttl(),
            cache_enabled: true,
            timeout: default_registry_timeout(),
            verify_checksums: true,
            allowed_source_hosts: Vec::new(),
            allow_http: false,
        }
    }
}

/// Agent binary distribution configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Directory containing agent binary files
    pub binary_dir: String,
    /// Optional bootstrap token for authenticating agent binary downloads
    pub bootstrap_token: Option<String>,
}

/// Configuration for artifact file transport.
///
/// Controls how file content (not metadata) is transferred between
/// workers/sensors and the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactsConfig {
    /// Transport mode: "auto" (default), "volume", or "api".
    ///
    /// - `auto`: detect shared volume via sentinel file, fall back to API
    /// - `volume`: always use shared filesystem
    /// - `api`: always use HTTP API for file transfer
    #[serde(default)]
    pub transport: crate::artifact_transport::TransportMode,

    /// Maximum file upload size in bytes for API transport (default: 100 MB).
    #[serde(default = "default_max_upload_size")]
    pub max_upload_size: u64,

    /// Buffer flush interval in milliseconds for streaming writes (default: 500).
    #[serde(default = "default_flush_interval_ms")]
    pub flush_interval_ms: u64,

    /// Sensor log rotation: max bytes per log file (default: 10 MB).
    #[serde(default = "default_sensor_log_max_bytes")]
    pub sensor_log_max_bytes: u64,

    /// Sensor log rotation: number of rotated files to keep (default: 4).
    #[serde(default = "default_sensor_log_max_files")]
    pub sensor_log_max_files: u32,
}

impl Default for ArtifactsConfig {
    fn default() -> Self {
        Self {
            transport: crate::artifact_transport::TransportMode::Auto,
            max_upload_size: default_max_upload_size(),
            flush_interval_ms: default_flush_interval_ms(),
            sensor_log_max_bytes: default_sensor_log_max_bytes(),
            sensor_log_max_files: default_sensor_log_max_files(),
        }
    }
}

fn default_max_upload_size() -> u64 {
    100 * 1024 * 1024 // 100 MB
}

fn default_flush_interval_ms() -> u64 {
    500
}

fn default_sensor_log_max_bytes() -> u64 {
    10 * 1024 * 1024 // 10 MB
}

fn default_sensor_log_max_files() -> u32 {
    4
}

/// Runtime database row retention settings.
///
/// A target with `max_age_seconds: None` keeps rows forever (purging disabled).
/// A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct RetentionTargetConfig {
    /// Maximum row age in seconds. `None` means keep forever (no purging).
    #[serde(default)]
    pub max_age_seconds: Option<u64>,
}

impl RetentionTargetConfig {
    pub fn with_days(days: u64) -> Self {
        Self {
            max_age_seconds: Some(days * 24 * 60 * 60),
        }
    }

    pub fn keep_forever() -> Self {
        Self {
            max_age_seconds: None,
        }
    }
}

/// Per-table runtime retention targets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct RetentionTargetsConfig {
    #[serde(default = "default_retention_events")]
    pub events: RetentionTargetConfig,
    #[serde(default = "default_retention_enforcements")]
    pub enforcements: RetentionTargetConfig,
    #[serde(default = "default_retention_executions")]
    pub executions: RetentionTargetConfig,
    #[serde(default = "default_retention_execution_history")]
    pub execution_history: RetentionTargetConfig,
    #[serde(default = "default_retention_worker_history")]
    pub worker_history: RetentionTargetConfig,
    #[serde(default = "default_retention_sensor_process_history")]
    pub sensor_process_history: RetentionTargetConfig,
    #[serde(default = "default_retention_audit_events")]
    pub audit_events: RetentionTargetConfig,
    #[serde(default = "default_retention_continuous_aggregates")]
    pub continuous_aggregates: RetentionTargetConfig,
    #[serde(default = "default_retention_notifications")]
    pub notifications: RetentionTargetConfig,
    #[serde(default = "default_retention_webhook_event_logs")]
    pub webhook_event_logs: RetentionTargetConfig,
    #[serde(default = "default_retention_inquiries")]
    pub inquiries: RetentionTargetConfig,
    #[serde(default = "default_retention_work_queue_items")]
    pub work_queue_items: RetentionTargetConfig,
    #[serde(default = "default_retention_work_queue_dispatches")]
    pub work_queue_dispatches: RetentionTargetConfig,
    #[serde(default = "default_retention_pack_test_executions")]
    pub pack_test_executions: RetentionTargetConfig,
    #[serde(default = "default_retention_execution_admission")]
    pub execution_admission: RetentionTargetConfig,
    #[serde(default = "default_retention_workers")]
    pub workers: RetentionTargetConfig,
    #[serde(default = "default_retention_sensor_processes")]
    pub sensor_processes: RetentionTargetConfig,
}

impl Default for RetentionTargetsConfig {
    fn default() -> Self {
        Self {
            events: default_retention_events(),
            enforcements: default_retention_enforcements(),
            executions: default_retention_executions(),
            execution_history: default_retention_execution_history(),
            worker_history: default_retention_worker_history(),
            sensor_process_history: default_retention_sensor_process_history(),
            audit_events: default_retention_audit_events(),
            continuous_aggregates: default_retention_continuous_aggregates(),
            notifications: default_retention_notifications(),
            webhook_event_logs: default_retention_webhook_event_logs(),
            inquiries: default_retention_inquiries(),
            work_queue_items: default_retention_work_queue_items(),
            work_queue_dispatches: default_retention_work_queue_dispatches(),
            pack_test_executions: default_retention_pack_test_executions(),
            execution_admission: default_retention_execution_admission(),
            workers: default_retention_workers(),
            sensor_processes: default_retention_sensor_processes(),
        }
    }
}

fn retention_days(days: u64) -> RetentionTargetConfig {
    RetentionTargetConfig::with_days(days)
}

fn default_retention_events() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_enforcements() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_executions() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_execution_history() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_worker_history() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_sensor_process_history() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_audit_events() -> RetentionTargetConfig {
    retention_days(90)
}

fn default_retention_continuous_aggregates() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_notifications() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_webhook_event_logs() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_inquiries() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_work_queue_items() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_work_queue_dispatches() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_pack_test_executions() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_execution_admission() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_workers() -> RetentionTargetConfig {
    retention_days(30)
}

fn default_retention_sensor_processes() -> RetentionTargetConfig {
    retention_days(30)
}

/// Supervisor-owned runtime retention configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct RetentionConfig {
    /// Enable runtime row retention globally.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// How often the supervisor runs retention, in seconds.
    #[serde(default = "default_retention_check_interval_seconds")]
    pub check_interval_seconds: u64,

    /// Maximum rows to delete per target per cycle for regular tables.
    #[serde(default = "default_retention_batch_size")]
    pub batch_size: i64,

    /// Report candidates without deleting rows/chunks.
    #[serde(default)]
    pub dry_run: bool,

    /// Advisory lock key used to make accidental multi-supervisor deployments safe.
    #[serde(default = "default_retention_advisory_lock_key")]
    pub advisory_lock_key: i64,

    /// Per-target retention settings.
    #[serde(default)]
    pub targets: RetentionTargetsConfig,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_seconds: default_retention_check_interval_seconds(),
            batch_size: default_retention_batch_size(),
            dry_run: false,
            advisory_lock_key: default_retention_advisory_lock_key(),
            targets: RetentionTargetsConfig::default(),
        }
    }
}

fn default_retention_check_interval_seconds() -> u64 {
    3600
}

fn default_retention_batch_size() -> i64 {
    1000
}

fn default_retention_advisory_lock_key() -> i64 {
    7_821_001
}

/// Supervisor maintenance jobs beyond runtime row retention.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupervisorMaintenanceConfig {
    /// Master switch for non-retention supervisor maintenance jobs.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Delete expired artifact versions according to per-artifact time policies.
    #[serde(default = "default_true")]
    pub artifact_cleanup_enabled: bool,

    /// Maximum expired artifact versions to clean per supervisor cycle.
    #[serde(default = "default_artifact_cleanup_batch_size")]
    pub artifact_cleanup_batch_size: i64,

    /// Detect stuck executions, queue leases, dispatches, and retention lag.
    #[serde(default = "default_true")]
    pub monitoring_enabled: bool,

    /// Apply guarded corrective actions for stale runtime state.
    #[serde(default = "default_true")]
    pub corrective_actions_enabled: bool,

    /// Alert when non-terminal executions are older than this many seconds.
    #[serde(default = "default_stuck_execution_seconds")]
    pub stuck_execution_seconds: u64,

    /// Correct executions that remain stuck beyond this many seconds.
    #[serde(default = "default_execution_remediation_seconds")]
    pub execution_remediation_seconds: u64,

    /// Republish Requested executions older than this many seconds before
    /// abandoning them as stale.
    #[serde(default = "default_execution_reschedule_grace_seconds")]
    pub execution_reschedule_grace_seconds: u64,

    /// Maximum automatic/manual reschedule attempts recorded for one execution.
    #[serde(default = "default_execution_reschedule_max_attempts")]
    pub execution_reschedule_max_attempts: u32,

    /// Alert when queue leases/dispatches are stale beyond this many seconds.
    #[serde(default = "default_stuck_queue_seconds")]
    pub stuck_queue_seconds: u64,

    /// Correct queue leases/dispatches that remain stale beyond this many seconds.
    #[serde(default = "default_queue_remediation_seconds")]
    pub queue_remediation_seconds: u64,

    /// Correct execution admission entries older than this many seconds when
    /// their executions are terminal.
    #[serde(default = "default_admission_remediation_seconds")]
    pub admission_remediation_seconds: u64,

    /// Alert when retention-eligible rows remain this long beyond their policy.
    #[serde(default = "default_retention_lag_alert_seconds")]
    pub retention_lag_alert_seconds: u64,

    /// Maximum monitoring alerts emitted per supervisor cycle.
    #[serde(default = "default_maintenance_alert_limit")]
    pub alert_limit_per_cycle: i64,

    /// Suppress duplicate alerts with the same correlation id for this duration.
    #[serde(default = "default_maintenance_alert_cooldown_seconds")]
    pub alert_cooldown_seconds: u64,
}

impl Default for SupervisorMaintenanceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            artifact_cleanup_enabled: true,
            artifact_cleanup_batch_size: default_artifact_cleanup_batch_size(),
            monitoring_enabled: true,
            corrective_actions_enabled: true,
            stuck_execution_seconds: default_stuck_execution_seconds(),
            execution_remediation_seconds: default_execution_remediation_seconds(),
            execution_reschedule_grace_seconds: default_execution_reschedule_grace_seconds(),
            execution_reschedule_max_attempts: default_execution_reschedule_max_attempts(),
            stuck_queue_seconds: default_stuck_queue_seconds(),
            queue_remediation_seconds: default_queue_remediation_seconds(),
            admission_remediation_seconds: default_admission_remediation_seconds(),
            retention_lag_alert_seconds: default_retention_lag_alert_seconds(),
            alert_limit_per_cycle: default_maintenance_alert_limit(),
            alert_cooldown_seconds: default_maintenance_alert_cooldown_seconds(),
        }
    }
}

fn default_artifact_cleanup_batch_size() -> i64 {
    100
}

fn default_stuck_execution_seconds() -> u64 {
    60 * 60
}

fn default_execution_remediation_seconds() -> u64 {
    2 * 60 * 60
}

fn default_execution_reschedule_grace_seconds() -> u64 {
    5 * 60
}

fn default_execution_reschedule_max_attempts() -> u32 {
    5
}

fn default_stuck_queue_seconds() -> u64 {
    15 * 60
}

fn default_queue_remediation_seconds() -> u64 {
    30 * 60
}

fn default_admission_remediation_seconds() -> u64 {
    30 * 60
}

fn default_retention_lag_alert_seconds() -> u64 {
    24 * 60 * 60
}

fn default_maintenance_alert_limit() -> i64 {
    25
}

fn default_maintenance_alert_cooldown_seconds() -> u64 {
    60 * 60
}

/// Executor service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    /// How long an execution can remain in SCHEDULED status before timing out (seconds)
    #[serde(default)]
    pub scheduled_timeout: Option<u64>,

    /// How often to check for stale executions (seconds)
    #[serde(default)]
    pub timeout_check_interval: Option<u64>,

    /// Whether to enable the execution timeout monitor
    #[serde(default)]
    pub enable_timeout_monitor: Option<bool>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            scheduled_timeout: Some(300),     // 5 minutes
            timeout_check_interval: Some(60), // 1 minute
            enable_timeout_monitor: Some(true),
        }
    }
}

/// Main application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Service name
    #[serde(default = "default_service_name")]
    pub service_name: String,

    /// Environment (development, staging, production)
    #[serde(default = "default_environment")]
    pub environment: String,

    /// Database configuration
    #[serde(default)]
    pub database: DatabaseConfig,

    /// Message queue configuration
    #[serde(default)]
    pub message_queue: Option<MessageQueueConfig>,

    /// Server configuration
    #[serde(default)]
    pub server: ServerConfig,

    /// Logging configuration
    #[serde(default)]
    pub log: LogConfig,

    /// Security configuration
    #[serde(default)]
    pub security: SecurityConfig,

    /// Worker configuration (optional, for worker services)
    pub worker: Option<WorkerConfig>,

    /// Sensor configuration (optional, for sensor services)
    pub sensor: Option<SensorConfig>,

    /// Packs base directory (where pack directories are located)
    #[serde(default = "default_packs_base_dir")]
    pub packs_base_dir: String,

    /// Runtime environments directory (isolated envs like virtualenvs, node_modules).
    /// Pattern: {runtime_envs_dir}/{pack_ref}/{runtime_name}
    /// e.g., /opt/attune/runtime_envs/python_example/python
    #[serde(default = "default_runtime_envs_dir")]
    pub runtime_envs_dir: String,

    /// Artifacts directory (shared volume for file-based artifact storage).
    /// File-type artifacts (FileBinary, FileDatatable, FileText, Log) are stored
    /// on disk at this location rather than in the database.
    /// Pattern: {artifacts_dir}/{ref_slug}/v{version}.{ext}
    #[serde(default = "default_artifacts_dir")]
    pub artifacts_dir: String,

    /// Artifact file transport configuration.
    #[serde(default)]
    pub artifacts: ArtifactsConfig,

    /// Notifier configuration (optional, for notifier service)
    pub notifier: Option<NotifierConfig>,

    /// Pack registry configuration
    #[serde(default)]
    pub pack_registry: PackRegistryConfig,

    /// Executor configuration (optional, for executor service)
    pub executor: Option<ExecutorConfig>,

    /// Agent configuration (optional, for agent binary distribution)
    pub agent: Option<AgentConfig>,

    /// Pack upload safety limits (optional; sensible defaults are used when absent).
    #[serde(default)]
    pub pack_upload: PackUploadConfig,

    /// Runtime database row retention configuration.
    #[serde(default)]
    pub retention: RetentionConfig,

    /// Supervisor maintenance jobs beyond runtime row retention.
    #[serde(default)]
    pub maintenance: SupervisorMaintenanceConfig,

    /// Default execution timeout in seconds, applied when neither an explicit
    /// execution override, a workflow task timeout, nor an action-level
    /// `timeout_seconds` is set. Snapshotted onto `execution.timeout_seconds`
    /// at execution creation time.
    #[serde(default = "default_execution_timeout_seconds")]
    pub default_execution_timeout_seconds: u64,
}

/// Safety limits applied during `POST /api/v1/packs/upload` archive extraction.
///
/// Defaults (used when the field is `None`):
/// * `max_extracted_size_bytes`: 100 MB
/// * `max_file_count`:           10_000 entries
/// * `max_per_entry_size_bytes`: 50 MB
/// * `allow_symlinks`:           false (symlinks/hardlinks are rejected)
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PackUploadConfig {
    pub max_extracted_size_bytes: Option<u64>,
    pub max_file_count: Option<u32>,
    pub max_per_entry_size_bytes: Option<u64>,
    pub allow_symlinks: Option<bool>,
}

impl PackUploadConfig {
    pub const DEFAULT_MAX_EXTRACTED_SIZE_BYTES: u64 = 100 * 1024 * 1024;
    pub const DEFAULT_MAX_FILE_COUNT: u32 = 10_000;
    pub const DEFAULT_MAX_PER_ENTRY_SIZE_BYTES: u64 = 50 * 1024 * 1024;
    pub const DEFAULT_ALLOW_SYMLINKS: bool = false;

    pub fn max_extracted_size_bytes(&self) -> u64 {
        self.max_extracted_size_bytes
            .unwrap_or(Self::DEFAULT_MAX_EXTRACTED_SIZE_BYTES)
    }

    pub fn max_file_count(&self) -> u32 {
        self.max_file_count.unwrap_or(Self::DEFAULT_MAX_FILE_COUNT)
    }

    pub fn max_per_entry_size_bytes(&self) -> u64 {
        self.max_per_entry_size_bytes
            .unwrap_or(Self::DEFAULT_MAX_PER_ENTRY_SIZE_BYTES)
    }

    pub fn allow_symlinks(&self) -> bool {
        self.allow_symlinks.unwrap_or(Self::DEFAULT_ALLOW_SYMLINKS)
    }
}

fn default_service_name() -> String {
    "attune".to_string()
}

fn default_environment() -> String {
    "development".to_string()
}

fn default_packs_base_dir() -> String {
    "/opt/attune/packs".to_string()
}

fn default_runtime_envs_dir() -> String {
    "/opt/attune/runtime_envs".to_string()
}

fn default_artifacts_dir() -> String {
    "/opt/attune/artifacts".to_string()
}

fn default_execution_timeout_seconds() -> u64 {
    600 // 10 minutes
}

/// Process-global snapshot of the app-level default execution timeout (seconds).
///
/// Set at service startup from the loaded [`Config`]. Code paths that
/// snapshot execution timeouts but do not have direct access to the `Config`
/// (e.g. the executor's static workflow-scheduling helpers) read this value via
/// [`app_default_execution_timeout_seconds`]. Falls back to 600 if never set.
static APP_DEFAULT_EXECUTION_TIMEOUT_SECONDS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(600);

/// Record the app-level default execution timeout for global access.
pub fn set_app_default_execution_timeout_seconds(seconds: u64) {
    if seconds > 0 {
        APP_DEFAULT_EXECUTION_TIMEOUT_SECONDS.store(seconds, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Return the app-level default execution timeout in seconds, falling back to
/// 600 when it has not been initialized.
pub fn app_default_execution_timeout_seconds() -> u64 {
    APP_DEFAULT_EXECUTION_TIMEOUT_SECONDS.load(std::sync::atomic::Ordering::Relaxed)
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: default_database_url(),
            max_connections: default_max_connections(),
            min_connections: default_min_connections(),
            connect_timeout: default_connection_timeout(),
            idle_timeout: default_idle_timeout(),
            log_statements: false,
            schema: None,
        }
    }
}

impl Default for NotifierConfig {
    fn default() -> Self {
        Self {
            host: default_notifier_host(),
            port: default_notifier_port(),
            max_connections: default_max_connections_notifier(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            request_timeout: default_request_timeout(),
            enable_cors: true,
            cors_origins: vec![],
            max_body_size: default_max_body_size(),
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
            console: true,
            file: None,
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            jwt_secret: None,
            jwt_access_expiration: default_jwt_access_expiration(),
            jwt_refresh_expiration: default_jwt_refresh_expiration(),
            encryption_key: None,
            enable_auth: true,
            allow_self_registration: false,
            login_page: LoginPageConfig::default(),
            oidc: None,
            ldap: None,
        }
    }
}

impl Config {
    /// Load configuration from YAML files and environment variables
    ///
    /// Loading priority (later sources override earlier ones):
    /// 1. Base config file (config.yaml or ATTUNE_CONFIG env var)
    /// 2. Environment-specific config (config.{environment}.yaml)
    /// 3. Environment variables (ATTUNE__ prefix)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use attune_common::config::Config;
    /// // Load from default config.yaml
    /// let config = Config::load().unwrap();
    ///
    /// // Load from custom path
    /// std::env::set_var("ATTUNE_CONFIG", "/path/to/config.yaml");
    /// let config = Config::load().unwrap();
    ///
    /// // Override with environment variables
    /// std::env::set_var("ATTUNE__DATABASE__URL", "postgresql://localhost/mydb");
    /// let config = Config::load().unwrap();
    /// ```
    pub fn load() -> crate::Result<Self> {
        let mut builder = config_crate::Config::builder();

        // 1. Load base config file
        let config_path =
            std::env::var("ATTUNE_CONFIG").unwrap_or_else(|_| "config.yaml".to_string());

        // Try to load the base config file (optional)
        if std::path::Path::new(&config_path).exists() {
            builder =
                builder.add_source(config_crate::File::with_name(&config_path).required(false));
        }

        // 2. Load environment-specific config file (e.g., config.development.yaml)
        // First, we need to get the environment from env var or default
        let environment =
            std::env::var("ATTUNE__ENVIRONMENT").unwrap_or_else(|_| default_environment());

        let env_config_path = format!("config.{}.yaml", environment);
        if std::path::Path::new(&env_config_path).exists() {
            builder =
                builder.add_source(config_crate::File::with_name(&env_config_path).required(false));
        }

        // 3. Load environment variables (highest priority)
        builder = builder.add_source(
            config_crate::Environment::with_prefix("ATTUNE")
                .separator("__")
                .try_parsing(true),
        );

        let config: config_crate::Config = builder
            .build()
            .map_err(|e: config_crate::ConfigError| crate::Error::configuration(e.to_string()))?;

        config
            .try_deserialize::<Self>()
            .map_err(|e: config_crate::ConfigError| crate::Error::configuration(e.to_string()))
    }

    /// Load configuration from a specific file path
    ///
    /// This bypasses the default config file discovery and loads directly from the specified path.
    /// Environment variables can still override values.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the YAML configuration file
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use attune_common::config::Config;
    /// let config = Config::load_from_file("./config.production.yaml").unwrap();
    /// ```
    pub fn load_from_file(path: &str) -> crate::Result<Self> {
        let mut builder = config_crate::Config::builder();

        // Load from specified file
        builder = builder.add_source(config_crate::File::with_name(path).required(true));

        // Load environment variables (for overrides)
        builder = builder.add_source(
            config_crate::Environment::with_prefix("ATTUNE")
                .separator("__")
                .try_parsing(true)
                .list_separator(","),
        );

        let config: config_crate::Config = builder
            .build()
            .map_err(|e: config_crate::ConfigError| crate::Error::configuration(e.to_string()))?;

        config
            .try_deserialize::<Self>()
            .map_err(|e: config_crate::ConfigError| crate::Error::configuration(e.to_string()))
    }

    /// Validate configuration
    pub fn validate(&self) -> crate::Result<()> {
        // Validate database URL
        if self.database.url.is_empty() {
            return Err(crate::Error::validation("Database URL cannot be empty"));
        }

        // Validate JWT secret if auth is enabled
        if self.security.enable_auth && self.security.jwt_secret.is_none() {
            return Err(crate::Error::validation(
                "JWT secret is required when authentication is enabled",
            ));
        }

        if let Some(oidc) = &self.security.oidc {
            if oidc.enabled {
                if oidc
                    .discovery_url
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    return Err(crate::Error::validation(
                        "OIDC discovery URL is required when OIDC is enabled",
                    ));
                }
                if oidc.client_id.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(crate::Error::validation(
                        "OIDC client ID is required when OIDC is enabled",
                    ));
                }
                if oidc.redirect_uri.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(crate::Error::validation(
                        "OIDC redirect URI is required when OIDC is enabled",
                    ));
                }
            }
        }

        if let Some(ldap) = &self.security.ldap {
            if ldap.enabled && ldap.url.as_deref().unwrap_or("").trim().is_empty() {
                return Err(crate::Error::validation(
                    "LDAP server URL is required when LDAP is enabled",
                ));
            }
        }

        // Validate encryption key if provided
        if let Some(ref key) = self.security.encryption_key {
            if key.len() < 32 {
                return Err(crate::Error::validation(
                    "Encryption key must be at least 32 characters",
                ));
            }
        }

        // Validate log level
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.log.level.as_str()) {
            return Err(crate::Error::validation(format!(
                "Invalid log level: {}. Must be one of: {:?}",
                self.log.level, valid_levels
            )));
        }

        // Validate log format
        let valid_formats = ["json", "pretty"];
        if !valid_formats.contains(&self.log.format.as_str()) {
            return Err(crate::Error::validation(format!(
                "Invalid log format: {}. Must be one of: {:?}",
                self.log.format, valid_formats
            )));
        }

        if self.retention.check_interval_seconds == 0 {
            return Err(crate::Error::validation(
                "retention.check_interval_seconds must be greater than zero",
            ));
        }

        if self.retention.batch_size <= 0 {
            return Err(crate::Error::validation(
                "retention.batch_size must be greater than zero",
            ));
        }

        if self.maintenance.artifact_cleanup_batch_size <= 0 {
            return Err(crate::Error::validation(
                "maintenance.artifact_cleanup_batch_size must be greater than zero",
            ));
        }

        if self.default_execution_timeout_seconds == 0 {
            return Err(crate::Error::validation(
                "default_execution_timeout_seconds must be greater than zero",
            ));
        }

        if self.maintenance.alert_limit_per_cycle <= 0 {
            return Err(crate::Error::validation(
                "maintenance.alert_limit_per_cycle must be greater than zero",
            ));
        }

        if self.maintenance.stuck_execution_seconds == 0
            || self.maintenance.execution_remediation_seconds == 0
            || self.maintenance.execution_reschedule_grace_seconds == 0
            || self.maintenance.stuck_queue_seconds == 0
            || self.maintenance.queue_remediation_seconds == 0
            || self.maintenance.admission_remediation_seconds == 0
            || self.maintenance.retention_lag_alert_seconds == 0
            || self.maintenance.alert_cooldown_seconds == 0
        {
            return Err(crate::Error::validation(
                "maintenance durations must be greater than zero",
            ));
        }

        if self.maintenance.execution_reschedule_max_attempts == 0 {
            return Err(crate::Error::validation(
                "maintenance.execution_reschedule_max_attempts must be greater than zero",
            ));
        }

        if self.maintenance.execution_reschedule_grace_seconds
            >= self.maintenance.execution_remediation_seconds
        {
            return Err(crate::Error::validation(
                "maintenance.execution_reschedule_grace_seconds must be less than maintenance.execution_remediation_seconds",
            ));
        }

        Ok(())
    }

    /// Check if running in production
    pub fn is_production(&self) -> bool {
        self.environment == "production"
    }

    /// Check if running in development
    pub fn is_development(&self) -> bool {
        self.environment == "development"
    }

    /// Emit non-fatal warnings about insecure secret-management practices.
    ///
    /// This is a best-effort heuristic check intended to nudge operators toward
    /// injecting secrets via environment variables (e.g.
    /// `ATTUNE__SECURITY__LDAP__SEARCH_BIND_PASSWORD`) or a secrets manager
    /// rather than committing literal values to YAML files.
    ///
    /// The function never fails startup — it only logs `WARN` messages.
    pub fn warn_about_insecure_secrets(&self) {
        if let Some(ldap) = &self.security.ldap {
            if ldap.enabled {
                if let Some(password) = ldap.search_bind_password.as_deref() {
                    if let Some(reason) = looks_like_literal_secret(password) {
                        tracing::warn!(
                            field = "security.ldap.search_bind_password",
                            reason = reason,
                            "LDAP search-bind password appears to be a literal value in config. \
                             Inject it via the environment variable \
                             ATTUNE__SECURITY__LDAP__SEARCH_BIND_PASSWORD or your platform's \
                             secrets manager (Docker secrets, Kubernetes Secrets, systemd \
                             credentials, etc.) instead of committing it to YAML. \
                             See docs/authentication/ldap-secrets.md."
                        );
                    }
                }
            }
        }
    }
}

/// Heuristic that returns `Some(reason)` when a string looks like a literal
/// placeholder or weak secret committed to a config file.
///
/// Returns `None` when the value looks like a real, env-injected secret.
fn looks_like_literal_secret(value: &str) -> Option<&'static str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Unresolved `${VAR}` placeholder — the `config` crate does not expand
    // these natively, so the application would attempt to bind with the
    // literal `${VAR}` string. Flag it as a clear configuration mistake.
    if trimmed.starts_with("${") && trimmed.ends_with('}') {
        return Some("unresolved ${VAR} placeholder; env-var override likely missing");
    }

    if trimmed.len() < 8 {
        return Some("value is shorter than 8 characters");
    }

    let lower = trimmed.to_ascii_lowercase();
    let placeholder_markers = [
        "password",
        "change-me",
        "changeme",
        "change_me",
        "placeholder",
        "example",
        "secret",
        "todo",
    ];
    for marker in placeholder_markers {
        if lower.contains(marker) {
            return Some("value contains a placeholder-looking substring");
        }
    }

    None
}

#[cfg(test)]
mod __secret_heuristic_tests {
    use super::looks_like_literal_secret;

    #[test]
    fn flags_short_values() {
        assert!(looks_like_literal_secret("short").is_some());
    }

    #[test]
    fn flags_placeholder_substrings() {
        assert!(looks_like_literal_secret("readonly-password").is_some());
        assert!(looks_like_literal_secret("change-me-please").is_some());
        assert!(looks_like_literal_secret("my-example-value-x").is_some());
    }

    #[test]
    fn flags_unresolved_env_placeholder() {
        assert!(looks_like_literal_secret("${LDAP_PASSWORD}").is_some());
    }

    #[test]
    fn passes_realistic_secret() {
        // High-entropy-looking value with no placeholder markers.
        assert!(looks_like_literal_secret("p9k2qX7vBn4mZ8tL").is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config {
            service_name: default_service_name(),
            environment: default_environment(),
            database: DatabaseConfig::default(),
            message_queue: None,
            server: ServerConfig::default(),
            log: LogConfig::default(),
            security: SecurityConfig::default(),
            worker: None,
            sensor: None,
            packs_base_dir: default_packs_base_dir(),
            runtime_envs_dir: default_runtime_envs_dir(),
            artifacts_dir: default_artifacts_dir(),
            artifacts: ArtifactsConfig::default(),
            notifier: None,
            pack_registry: PackRegistryConfig::default(),
            executor: None,
            agent: None,
            pack_upload: PackUploadConfig::default(),
            retention: RetentionConfig::default(),
            maintenance: SupervisorMaintenanceConfig::default(),
            default_execution_timeout_seconds: default_execution_timeout_seconds(),
        };

        assert_eq!(config.service_name, "attune");
        assert_eq!(config.environment, "development");
        assert!(config.is_development());
        assert!(!config.is_production());
    }

    #[test]
    fn worker_log_retention_defaults_to_seven_days() {
        let worker: WorkerConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(
            worker.execution_log_retention_policy,
            crate::models::enums::RetentionPolicyType::Days
        );
        assert_eq!(worker.execution_log_retention_limit, 7);
    }

    #[test]
    fn test_cors_origins_deserializer() {
        use serde_json::json;

        // Test with comma-separated string
        let json_str = json!({
            "cors_origins": "http://localhost:3000,http://localhost:5173,http://test.com"
        });
        let config: ServerConfig = serde_json::from_value(json_str).unwrap();
        assert_eq!(config.cors_origins.len(), 3);
        assert_eq!(config.cors_origins[0], "http://localhost:3000");
        assert_eq!(config.cors_origins[1], "http://localhost:5173");
        assert_eq!(config.cors_origins[2], "http://test.com");

        // Test with array format
        let json_array = json!({
            "cors_origins": ["http://localhost:3000", "http://localhost:5173"]
        });
        let config: ServerConfig = serde_json::from_value(json_array).unwrap();
        assert_eq!(config.cors_origins.len(), 2);
        assert_eq!(config.cors_origins[0], "http://localhost:3000");
        assert_eq!(config.cors_origins[1], "http://localhost:5173");

        // Test with empty string
        let json_empty = json!({
            "cors_origins": ""
        });
        let config: ServerConfig = serde_json::from_value(json_empty).unwrap();
        assert_eq!(config.cors_origins.len(), 0);

        // Test with string containing spaces - should trim properly
        let json_spaces = json!({
            "cors_origins": "http://localhost:3000 , http://localhost:5173 , http://test.com"
        });
        let config: ServerConfig = serde_json::from_value(json_spaces).unwrap();
        assert_eq!(config.cors_origins.len(), 3);
        assert_eq!(config.cors_origins[0], "http://localhost:3000");
        assert_eq!(config.cors_origins[1], "http://localhost:5173");
        assert_eq!(config.cors_origins[2], "http://test.com");
    }

    #[test]
    fn test_config_validation() {
        let mut config = Config {
            service_name: default_service_name(),
            environment: default_environment(),
            database: DatabaseConfig::default(),
            message_queue: None,
            server: ServerConfig::default(),
            log: LogConfig::default(),
            security: SecurityConfig {
                jwt_secret: Some("test_secret".to_string()),
                jwt_access_expiration: 3600,
                jwt_refresh_expiration: 604800,
                encryption_key: Some("a".repeat(32)),
                enable_auth: true,
                allow_self_registration: false,
                login_page: LoginPageConfig::default(),
                oidc: None,
                ldap: None,
            },
            worker: None,
            sensor: None,
            packs_base_dir: default_packs_base_dir(),
            runtime_envs_dir: default_runtime_envs_dir(),
            artifacts_dir: default_artifacts_dir(),
            artifacts: ArtifactsConfig::default(),
            notifier: None,
            pack_registry: PackRegistryConfig::default(),
            executor: None,
            agent: None,
            pack_upload: PackUploadConfig::default(),
            retention: RetentionConfig::default(),
            maintenance: SupervisorMaintenanceConfig::default(),
            default_execution_timeout_seconds: default_execution_timeout_seconds(),
        };

        assert!(config.validate().is_ok());

        // Test invalid encryption key
        config.security.encryption_key = Some("short".to_string());
        assert!(config.validate().is_err());

        // Test missing JWT secret
        config.security.encryption_key = Some("a".repeat(32));
        config.security.jwt_secret = None;
        assert!(config.validate().is_err());
    }

    #[test]
    fn supervisor_reschedule_config_validation() {
        let mut config = Config {
            service_name: default_service_name(),
            environment: default_environment(),
            database: DatabaseConfig::default(),
            message_queue: None,
            server: ServerConfig::default(),
            log: LogConfig::default(),
            security: SecurityConfig {
                jwt_secret: Some("test_secret".to_string()),
                encryption_key: Some("a".repeat(32)),
                ..SecurityConfig::default()
            },
            worker: None,
            sensor: None,
            packs_base_dir: default_packs_base_dir(),
            runtime_envs_dir: default_runtime_envs_dir(),
            artifacts_dir: default_artifacts_dir(),
            artifacts: ArtifactsConfig::default(),
            notifier: None,
            pack_registry: PackRegistryConfig::default(),
            executor: None,
            agent: None,
            pack_upload: PackUploadConfig::default(),
            retention: RetentionConfig::default(),
            maintenance: SupervisorMaintenanceConfig::default(),
            default_execution_timeout_seconds: default_execution_timeout_seconds(),
        };

        assert!(config.validate().is_ok());

        config.maintenance.execution_reschedule_grace_seconds =
            config.maintenance.execution_remediation_seconds;
        assert!(config.validate().is_err());

        config.maintenance.execution_reschedule_grace_seconds = 60;
        config.maintenance.execution_reschedule_max_attempts = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn runtime_retention_defaults_are_safe() {
        let retention = RetentionConfig::default();
        assert!(retention.enabled);
        assert_eq!(retention.check_interval_seconds, 3600);
        assert_eq!(retention.batch_size, 1000);
        assert_eq!(
            retention.targets.events.max_age_seconds,
            Some(30 * 24 * 60 * 60)
        );
        assert_eq!(
            retention.targets.audit_events.max_age_seconds,
            Some(90 * 24 * 60 * 60)
        );
    }

    #[test]
    fn retention_target_keep_forever() {
        let retention: RetentionConfig = serde_json::from_value(serde_json::json!({
            "targets": {
                "events": {
                    "max_age_seconds": null
                },
                "audit_events": {
                    "max_age_seconds": null
                }
            }
        }))
        .unwrap();

        assert_eq!(retention.targets.events.max_age_seconds, None);
        assert_eq!(retention.targets.audit_events.max_age_seconds, None);
    }

    #[test]
    fn test_oidc_config_disabled_no_urls_required() {
        let yaml = r#"
enabled: false
"#;
        let cfg: OidcConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(!cfg.enabled);
        assert!(cfg.discovery_url.is_none());
        assert!(cfg.client_id.is_none());
        assert!(cfg.redirect_uri.is_none());
        assert!(cfg.client_secret.is_none());
        assert_eq!(cfg.provider_name, "oidc");
    }

    #[test]
    fn test_ldap_config_disabled_no_url_required() {
        let yaml = r#"
enabled: false
"#;
        let cfg: LdapConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(!cfg.enabled);
        assert!(cfg.url.is_none());
        assert_eq!(cfg.provider_name, "ldap");
    }

    #[test]
    fn test_ldap_config_defaults() {
        let yaml = r#"
enabled: true
url: "ldap://localhost:389"
client_id: "test"
"#;
        let cfg: LdapConfig = serde_yaml_ng::from_str(yaml).unwrap();

        assert!(cfg.enabled);
        assert_eq!(cfg.url.as_deref(), Some("ldap://localhost:389"));
        assert_eq!(cfg.user_filter, "(uid={login})");
        assert_eq!(cfg.login_attr, "uid");
        assert_eq!(cfg.email_attr, "mail");
        assert_eq!(cfg.display_name_attr, "cn");
        assert_eq!(cfg.group_attr, "memberOf");
        assert_eq!(cfg.provider_name, "ldap");
        assert!(!cfg.starttls);
        assert!(!cfg.danger_skip_tls_verify);
        assert!(cfg.bind_dn_template.is_none());
        assert!(cfg.user_search_base.is_none());
        assert!(cfg.search_bind_dn.is_none());
        assert!(cfg.search_bind_password.is_none());
        assert!(cfg.provider_label.is_none());
        assert!(cfg.provider_icon_url.is_none());
    }

    #[test]
    fn test_ldap_config_full_deserialization() {
        let yaml = r#"
enabled: true
url: "ldaps://ldap.corp.com:636"
bind_dn_template: "uid={login},ou=people,dc=corp,dc=com"
user_search_base: "ou=people,dc=corp,dc=com"
user_filter: "(sAMAccountName={login})"
search_bind_dn: "cn=svc,dc=corp,dc=com"
search_bind_password: "secret"
login_attr: "sAMAccountName"
email_attr: "userPrincipalName"
display_name_attr: "displayName"
group_attr: "memberOf"
starttls: true
danger_skip_tls_verify: true
provider_name: "corpldap"
provider_label: "Corporate Directory"
provider_icon_url: "https://corp.com/icon.svg"
"#;
        let cfg: LdapConfig = serde_yaml_ng::from_str(yaml).unwrap();

        assert!(cfg.enabled);
        assert_eq!(cfg.url.as_deref(), Some("ldaps://ldap.corp.com:636"));
        assert_eq!(
            cfg.bind_dn_template.as_deref(),
            Some("uid={login},ou=people,dc=corp,dc=com")
        );
        assert_eq!(
            cfg.user_search_base.as_deref(),
            Some("ou=people,dc=corp,dc=com")
        );
        assert_eq!(cfg.user_filter, "(sAMAccountName={login})");
        assert_eq!(cfg.search_bind_dn.as_deref(), Some("cn=svc,dc=corp,dc=com"));
        assert_eq!(cfg.search_bind_password.as_deref(), Some("secret"));
        assert_eq!(cfg.login_attr, "sAMAccountName");
        assert_eq!(cfg.email_attr, "userPrincipalName");
        assert_eq!(cfg.display_name_attr, "displayName");
        assert_eq!(cfg.group_attr, "memberOf");
        assert!(cfg.starttls);
        assert!(cfg.danger_skip_tls_verify);
        assert_eq!(cfg.provider_name, "corpldap");
        assert_eq!(cfg.provider_label.as_deref(), Some("Corporate Directory"));
        assert_eq!(
            cfg.provider_icon_url.as_deref(),
            Some("https://corp.com/icon.svg")
        );
    }

    #[test]
    fn test_security_config_ldap_none_by_default() {
        let yaml = r#"jwt_secret: "s""#;
        let cfg: SecurityConfig = serde_yaml_ng::from_str(yaml).unwrap();

        assert!(cfg.ldap.is_none());
    }

    #[test]
    fn test_login_page_show_ldap_default_true() {
        let cfg: LoginPageConfig = serde_yaml_ng::from_str("{}").unwrap();

        assert!(cfg.show_ldap_login);
    }

    #[test]
    fn test_login_page_show_ldap_explicit_false() {
        let cfg: LoginPageConfig = serde_yaml_ng::from_str("show_ldap_login: false").unwrap();

        assert!(!cfg.show_ldap_login);
    }
}
