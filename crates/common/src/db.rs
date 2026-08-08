//! Database connection and management
//!
//! This module provides database connection pooling and utilities for
//! interacting with the PostgreSQL database.

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;
use tracing::{info, warn};

use crate::config::DatabaseConfig;
use crate::error::Result;

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
                    let search_path = format!("SET search_path TO {}, public", schema);
                    sqlx::query(&search_path).execute(&mut *conn).await?;
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
    /// Note: Migrations should be in the workspace root migrations directory
    pub async fn migrate(&self) -> Result<()> {
        info!("Running database migrations");
        // TODO: Implement migrations when migration files are created
        // sqlx::migrate!("../../migrations")
        //     .run(&self.pool)
        //     .await?;
        info!("Database migrations will be implemented with migration files");
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
