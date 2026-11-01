//! Database connection and management
//!
//! This module provides database connection pooling and utilities for
//! interacting with the PostgreSQL database.

use sha2::{Digest, Sha256};
use sqlx::migrate::Migrate;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool, PgPoolOptions};
use sqlx::Acquire;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{info, warn};

use crate::config::DatabaseConfig;
use crate::error::Result;

/// Attribution for one service instance and connection purpose. Do not use secrets
/// or request/user identifiers here; PostgreSQL exposes this in pg_stat_activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionIdentity {
    service: String,
    instance: String,
    purpose: String,
}

impl ConnectionIdentity {
    pub fn new(
        service: impl Into<String>,
        instance: impl Into<String>,
        purpose: impl Into<String>,
    ) -> Self {
        Self {
            service: service.into(),
            instance: instance.into(),
            purpose: purpose.into(),
        }
    }

    /// Use one random instance ID for this process, shared by all of its pools.
    pub fn for_process(service: &str, purpose: &str) -> Self {
        static INSTANCE: OnceLock<String> = OnceLock::new();
        Self::new(
            service,
            INSTANCE.get_or_init(|| uuid::Uuid::new_v4().simple().to_string()),
            purpose,
        )
    }

    /// PostgreSQL escapes non-ASCII application_name bytes and limits the result
    /// to 63 bytes. Render an ASCII prefix ourselves so the server keeps the exact
    /// name. Sanitation, truncation, or ambiguous component separators add a
    /// SHA-256 suffix of the full original identity, with length-framed fields.
    pub fn application_name(&self) -> String {
        let mut digest = Sha256::new();
        for part in [&self.service, &self.purpose, &self.instance] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part.as_bytes());
        }
        let original = format!("attune:{}:{}:{}", self.service, self.purpose, self.instance);
        let readable: String = original
            .chars()
            .map(|c| {
                if !c.is_ascii() || c.is_ascii_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        if readable.len() <= 63
            && readable == original
            && ![&self.service, &self.purpose, &self.instance]
                .iter()
                .any(|part| part.contains(':'))
        {
            return readable;
        }
        let suffix: String = digest.finalize()[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let end = readable.len().min(46);
        format!("{}~{suffix}", &readable[..end])
    }

    /// Preserve URL credentials, TLS, and startup options, replacing only the name.
    pub fn connect_options(&self, url: &str) -> std::result::Result<PgConnectOptions, sqlx::Error> {
        Ok(PgConnectOptions::from_str(url)?.application_name(&self.application_name()))
    }
}

fn configure_pool(
    options: PgPoolOptions,
    schema: String,
    application_name: String,
) -> PgPoolOptions {
    options.after_connect(move |conn, _meta| {
        let schema = schema.clone();
        let application_name = application_name.clone();
        Box::pin(async move {
            // Extension functions live in public; application tables resolve first.
            sqlx::query("SELECT set_config('search_path', $1, false), set_config('application_name', $2, false)")
                .bind(format!("{schema}, public"))
                .bind(application_name)
                .execute(&mut *conn)
                .await?;
            Ok(())
        })
    })
}

async fn pin_sqlx_history_search_path(connection: &mut PgConnection) -> Result<()> {
    let (has_attune_history, has_public_history): (bool, bool) = sqlx::query_as(
        r#"
        SELECT
            to_regclass('attune._sqlx_migrations') IS NOT NULL,
            to_regclass('public._sqlx_migrations') IS NOT NULL
        "#,
    )
    .fetch_one(&mut *connection)
    .await?;

    if has_attune_history && has_public_history {
        return Err(crate::error::Error::invalid_state(
            "Database contains SQLx migration history in both attune and public schemas",
        ));
    }

    if !has_attune_history && !has_public_history {
        sqlx::query("CREATE SCHEMA IF NOT EXISTS attune")
            .execute(&mut *connection)
            .await?;
    }

    let search_path = if has_public_history {
        "public, attune"
    } else {
        "attune, public"
    };
    sqlx::query("SELECT set_config('search_path', $1, false)")
        .bind(search_path)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

async fn run_embedded_migrations(connection: &mut PgConnection) -> Result<()> {
    Migrate::lock(connection).await.map_err(|error| {
        crate::error::Error::internal(format!("Failed to lock SQLx migrations: {error}"))
    })?;

    let migration_result = async {
        pin_sqlx_history_search_path(connection).await?;
        let mut migrator = sqlx::migrate!("../../migrations");
        migrator.set_locking(false);
        migrator.run(&mut *connection).await.map_err(|error| {
            crate::error::Error::internal(format!("Database migration failed: {error}"))
        })
    }
    .await;

    let restore_result = sqlx::query("SELECT set_config('search_path', 'attune, public', false)")
        .execute(&mut *connection)
        .await
        .map_err(crate::error::Error::from);
    let unlock_result = Migrate::unlock(connection).await.map_err(|error| {
        crate::error::Error::internal(format!("Failed to unlock SQLx migrations: {error}"))
    });

    migration_result?;
    restore_result?;
    unlock_result
}

/// Database connection pool
#[derive(Debug, Clone)]
pub struct Database {
    pool: PgPool,
    schema: String,
}

impl Database {
    /// Convenience constructor for tests and local callers. Services must use
    /// `new_named` so their connections can be attributed independently.
    pub async fn new(config: &DatabaseConfig) -> Result<Self> {
        Self::new_named(
            config,
            ConnectionIdentity::for_process("unattributed", "main"),
        )
        .await
    }

    /// Create a service pool with explicit attribution on every new connection.
    pub async fn new_named(config: &DatabaseConfig, identity: ConnectionIdentity) -> Result<Self> {
        // Default to "attune" schema for production safety
        let schema = config
            .schema
            .clone()
            .unwrap_or_else(|| "attune".to_string());

        // Validate schema name (prevent SQL injection)
        Self::validate_schema_name(&schema)?;

        // Log schema configuration prominently
        info!(
            max_connections = config.max_connections,
            schema,
            application_name = identity.application_name(),
            "Connecting to database"
        );

        let options = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(Duration::from_secs(config.connect_timeout))
            .acquire_time_level(log::LevelFilter::Debug)
            .idle_timeout(Duration::from_secs(config.idle_timeout));
        let pool = configure_pool(options, schema.clone(), identity.application_name())
            .connect_with(identity.connect_options(&config.url)?)
            .await?;

        // Run a test query to verify connection
        sqlx::query("SELECT 1").execute(&pool).await.map_err(|e| {
            warn!("Failed to verify database connection: {}", e);
            e
        })?;

        info!("Successfully connected to database");

        crate::repositories::platform_catalog::PlatformCatalogRepository::check_existing_epoch(
            &pool,
        )
        .await?;

        Ok(Self { pool, schema })
    }

    /// Validate schema name to prevent SQL injection
    fn validate_schema_name(schema: &str) -> Result<()> {
        if schema.is_empty() {
            return Err(crate::error::Error::Configuration(
                "Schema name cannot be empty".to_string(),
            ));
        }

        // Only allow alphanumeric and underscores
        if !schema.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(crate::error::Error::Configuration(format!(
                "Invalid schema name '{}': only alphanumeric and underscores allowed",
                schema
            )));
        }

        // Prevent excessively long names (PostgreSQL limit is 63 chars)
        if schema.len() > 63 {
            return Err(crate::error::Error::Configuration(format!(
                "Schema name '{}' too long (max 63 characters)",
                schema
            )));
        }

        Ok(())
    }

    /// Get a reference to the connection pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Get the current schema name
    pub fn schema(&self) -> &str {
        &self.schema
    }

    /// Close the database connection pool
    pub async fn close(&self) {
        self.pool.close().await;
        info!("Database connection pool closed");
    }

    /// Run database migrations
    /// Migrations are embedded from the workspace root for packaged binaries.
    pub async fn migrate(&self) -> Result<()> {
        if self.schema != "attune" {
            return Err(crate::error::Error::invalid_state(format!(
                "Embedded migrations only support the 'attune' schema because historical migrations set search_path explicitly; configured schema is '{}'",
                self.schema
            )));
        }

        info!("Running database migrations");
        let mut connection = self.pool.acquire().await?;

        // Atomically claim one migration runner before either history table can
        // be created. The persistent claim closes the first-run race between
        // the embedded SQLx and Docker filename-based migration paths.
        let mut claim = connection.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(78210015)")
            .execute(&mut *claim)
            .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS public._attune_migration_runner (
                id SMALLINT PRIMARY KEY CHECK (id = 1),
                runner TEXT NOT NULL CHECK (runner IN ('sqlx', 'docker')),
                claimed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&mut *claim)
        .await?;

        let docker_history: bool = sqlx::query_scalar(
            r#"
            SELECT to_regclass('attune._migrations') IS NOT NULL
                OR to_regclass('public._migrations') IS NOT NULL
            "#,
        )
        .fetch_one(&mut *claim)
        .await?;
        if docker_history {
            return Err(crate::error::Error::invalid_state(
                "Database uses Docker migration history; run the deployment's migration container instead of attune-api --migrate",
            ));
        }
        sqlx::query(
            "INSERT INTO public._attune_migration_runner (id, runner) VALUES (1, 'sqlx') ON CONFLICT (id) DO NOTHING",
        )
        .execute(&mut *claim)
        .await?;
        let claimed_runner: String =
            sqlx::query_scalar("SELECT runner FROM public._attune_migration_runner WHERE id = 1")
                .fetch_one(&mut *claim)
                .await?;
        if claimed_runner != "sqlx" {
            return Err(crate::error::Error::invalid_state(
                "Database is claimed by the Docker migration runner; run the deployment's migration container instead of attune-api --migrate",
            ));
        }
        claim.commit().await?;

        run_embedded_migrations(&mut connection).await?;
        info!("Database migrations completed");
        Ok(())
    }

    /// Check if the database connection is healthy
    pub async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Get pool statistics
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            connections: self.pool.size(),
            idle_connections: self.pool.num_idle(),
        }
    }
}

/// Database pool statistics
#[derive(Debug, Clone)]
pub struct PoolStats {
    pub connections: u32,
    pub idle_connections: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_identity_short_names_are_readable() {
        assert_eq!(
            ConnectionIdentity::new("api", "instance-1", "listener").application_name(),
            "attune:api:listener:instance-1"
        );
    }

    #[test]
    fn connection_identity_truncation_is_byte_safe_and_stable() {
        for instance in [
            "a".repeat(47),
            "a".repeat(48),
            "a".repeat(49),
            "界".repeat(80),
            "🦀".repeat(80),
        ] {
            let identity = ConnectionIdentity::new("api", instance, "main");
            let name = identity.application_name();
            assert!(name.is_ascii(), "{name}");
            assert!(name.len() <= 63, "{name}");
            assert_eq!(name, identity.application_name());
            assert!(name.starts_with("attune:api:main:"));
        }
        let prefix = "x".repeat(100);
        assert_eq!(
            ConnectionIdentity::new("api", format!("{prefix}a"), "main").application_name(),
            "attune:api:main:xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx~018586a35e64ed47"
        );
        assert_ne!(
            ConnectionIdentity::new("api", format!("{prefix}a"), "main").application_name(),
            ConnectionIdentity::new("api", format!("{prefix}b"), "main").application_name()
        );
        // A suffix hashes all fields, including fields beyond the readable prefix.
        assert_ne!(
            ConnectionIdentity::new(prefix.clone(), "one", "main").application_name(),
            ConnectionIdentity::new(prefix, "one", "listener").application_name()
        );
    }

    #[test]
    fn connection_identity_short_unicode_is_sanitized_and_keeps_original_digest() {
        let name = ConnectionIdentity::new("api", "界", "main").application_name();
        assert!(name.is_ascii());
        assert!(name.len() <= 63);
        assert_eq!(name, "attune:api:main:_~c751e46d3c7296fa");
        assert_ne!(
            name,
            ConnectionIdentity::new("api", "_", "main").application_name()
        );
        assert_ne!(
            name,
            ConnectionIdentity::new("api", "海", "main").application_name()
        );
        let all_fields = ConnectionIdentity::new("apï", "界", "maïn").application_name();
        assert!(all_fields.is_ascii());
        assert!(all_fields.len() <= 63);
        assert!(all_fields.starts_with("attune:ap_:ma_n:_~"));
    }

    #[test]
    fn connection_identity_controls_and_component_boundaries_do_not_alias() {
        let name = ConnectionIdentity::new("api", "one\0\n", "main").application_name();
        assert!(name.is_ascii());
        assert!(name.len() <= 63);
        assert!(!name.chars().any(char::is_control));
        assert_ne!(
            name,
            ConnectionIdentity::new("api", "one__", "main").application_name()
        );
        assert_ne!(
            ConnectionIdentity::new("a:b", "d", "c").application_name(),
            ConnectionIdentity::new("a", "d", "b:c").application_name()
        );
    }

    #[test]
    fn connection_identity_preserves_connect_options() {
        let identity = ConnectionIdentity::new("api", "one", "listener");
        let options = identity.connect_options("postgresql://user:secret@db.example:5433/production?sslmode=verify-full&application_name=old&options=-c%20search_path%3Dprivate").unwrap();
        assert_eq!(options.get_host(), "db.example");
        assert_eq!(options.get_port(), 5433);
        assert_eq!(options.get_username(), "user");
        assert_eq!(options.get_database(), Some("production"));
        assert!(matches!(
            options.get_ssl_mode(),
            sqlx::postgres::PgSslMode::VerifyFull
        ));
        assert_eq!(options.get_options(), Some("-c search_path=private"));
        assert_eq!(
            options.get_application_name(),
            Some(identity.application_name().as_str())
        );
    }

    #[test]
    fn process_pools_share_an_instance_but_not_a_purpose() {
        let working = ConnectionIdentity::for_process("notifier", "working");
        let listener = ConnectionIdentity::for_process("notifier", "listener");
        assert_eq!(working.instance, listener.instance);
        assert_ne!(working.application_name(), listener.application_name());
    }

    #[test]
    fn test_pool_stats() {
        // Test that PoolStats can be created
        let stats = PoolStats {
            connections: 10,
            idle_connections: 5,
        };
        assert_eq!(stats.connections, 10);
        assert_eq!(stats.idle_connections, 5);
    }
}
