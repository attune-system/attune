//! Database connection and management
//!
//! This module provides database connection pooling and utilities for
//! interacting with the PostgreSQL database.

use sqlx::migrate::Migrate;
use sqlx::postgres::{PgConnection, PgPool, PgPoolOptions};
use sqlx::Acquire;
use std::time::Duration;
use tracing::{info, warn};

use crate::config::DatabaseConfig;
use crate::error::Result;

const V021_MIGRATION_CHECKSUMS: &[(i64, &str, &str)] = &[
    (
        20250101000009,
        "21226d6a5c436c95cfd19277d7f5e4f6f54fc30c9690c28d3f3f4c07343a078b14bdcb0e3f60bc2a2c2b197b716765f2",
        "ac857a353cc0a325788c54a89ff4dd594af8e13567516a3e334d4d5cc6f38ad8af358001c82ce713367034e79895780b",
    ),
    (
        20250101000013,
        "275d15eb2f9af232f869eb9a4da30f35a9312f9b8206e9e4ce59a9eb244323a071e39251e31155180a027ceaab3c8788",
        "3e68a2d74ccc74f7fb182db0ee92b9390e8ca75499fc6b8ac0ea8f2ce629650967d99a7f12f211035cf5ceb80bb63947",
    ),
    (
        20250101000014,
        "f8d7e71cc4a79bbb69262033a2b881f4110dc53aa39031465c9d2fe1a61c9fc431cec486323baf8265a9bdf8b9d71994",
        "061a9178e561ca4e5f2c8a814aa326300ba6fe6b1dd40bb6514e1486198a1049aac59529095aa0f864c755463f084f8d",
    ),
    (
        20250101000018,
        "fa4c9a91965ccd647b6c4372c5db7296d650e7ed4bd717664652174365b85813d417fb2527d84cdebb6f2975c6dbd78d",
        "429fe44ef8a7d5ae6bcdd1a2f4111e54f660edf17e7c392e15ea69f3ae5236e73a657e291bcdd23547132be083c123cb",
    ),
];

async fn bridge_v021_migration_checksums(connection: &mut PgConnection) -> Result<()> {
    let mut transaction = connection.begin().await?;
    let history_exists: bool =
        sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut *transaction)
            .await?;

    if !history_exists {
        transaction.commit().await?;
        return Ok(());
    }

    for &(version, legacy_checksum, current_checksum) in V021_MIGRATION_CHECKSUMS {
        let checksum_state: Option<(bool, bool, bool)> = sqlx::query_as(
            r#"
            SELECT
                success,
                checksum = decode($2, 'hex') AS is_legacy,
                checksum = decode($3, 'hex') AS is_current
            FROM _sqlx_migrations
            WHERE version = $1
            FOR UPDATE
            "#,
        )
        .bind(version)
        .bind(legacy_checksum)
        .bind(current_checksum)
        .fetch_optional(&mut *transaction)
        .await?;

        let Some((success, is_legacy, is_current)) = checksum_state else {
            continue;
        };
        if !success {
            return Err(crate::error::Error::invalid_state(format!(
                "SQLx migration {version} is marked unsuccessful; refusing checksum compatibility rewrite"
            )));
        }
        if !is_legacy && !is_current {
            return Err(crate::error::Error::invalid_state(format!(
                "SQLx migration {version} has an unrecognized checksum; refusing compatibility rewrite"
            )));
        }
        if is_legacy {
            sqlx::query(
                "UPDATE _sqlx_migrations SET checksum = decode($2, 'hex') WHERE version = $1",
            )
            .bind(version)
            .bind(current_checksum)
            .execute(&mut *transaction)
            .await?;
        }
    }

    transaction.commit().await?;
    Ok(())
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
        bridge_v021_migration_checksums(connection).await?;
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
    /// Create a new database connection from configuration
    pub async fn new(config: &DatabaseConfig) -> Result<Self> {
        // Default to "attune" schema for production safety
        let schema = config
            .schema
            .clone()
            .unwrap_or_else(|| "attune".to_string());

        // Validate schema name (prevent SQL injection)
        Self::validate_schema_name(&schema)?;

        // Log schema configuration prominently
        info!(
            "Connecting to database with max_connections={}, schema={}",
            config.max_connections, schema
        );

        // Clone schema for use in closure
        let schema_for_hook = schema.clone();

        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(Duration::from_secs(config.connect_timeout))
            .idle_timeout(Duration::from_secs(config.idle_timeout))
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    // Extension functions are installed in public, while unqualified
                    // application tables continue to resolve in the configured schema.
                    sqlx::query("SELECT set_config('search_path', $1, false), set_config('application_name', $2, false)")
                        .bind(format!("{schema}, public"))
                        .bind(format!("attune:{schema}"))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&config.url)
            .await?;

        // Run a test query to verify connection
        sqlx::query("SELECT 1").execute(&pool).await.map_err(|e| {
            warn!("Failed to verify database connection: {}", e);
            e
        })?;

        info!("Successfully connected to database");

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
