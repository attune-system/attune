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
