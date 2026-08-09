//! Shared PostgreSQL fixture for integration tests.

use crate::{config::DatabaseConfig, db::Database, Error, Result};
use sqlx::{Connection, PgConnection, PgPool};
use std::{path::PathBuf, time::Duration};

const MIGRATION_LOCK_KEY: i64 = 78_210_014;
const DEFAULT_SEARCH_PATH: &str = "SET search_path TO attune, public;";

/// A fully migrated, schema-isolated test database.
pub struct TestDatabase {
    pool: PgPool,
    schema: String,
    database_url: String,
}

impl TestDatabase {
    /// Create a unique schema and apply every migration in order.
    pub async fn create(config: &DatabaseConfig) -> Result<Self> {
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let mut connection = PgConnection::connect(&config.url).await?;

        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&mut connection)
            .await?;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut connection)
            .await?;

        for migration_path in migration_paths()? {
            let sql = std::fs::read_to_string(&migration_path)
                .map_err(|error| Error::Io(format!("{}: {error}", migration_path.display())))?
                .replace(
                    DEFAULT_SEARCH_PATH,
                    &format!("SET search_path TO {schema}, public;"),
                );

            sqlx::query(&format!("SET search_path TO {schema}, public"))
                .execute(&mut connection)
                .await?;

            for attempt in 1..=3 {
                match sqlx::raw_sql(&sql).execute(&mut connection).await {
                    Ok(_) => break,
                    Err(error) if is_deadlock(&error) && attempt < 3 => {
                        tokio::time::sleep(Duration::from_millis(100 * attempt)).await;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }

        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut connection)
            .await?;

        let mut schema_config = config.clone();
        schema_config.schema = Some(schema.clone());
        let pool = Database::new(&schema_config).await?.pool().clone();

        Ok(Self {
            pool,
            schema,
            database_url: config.url.clone(),
        })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    /// Close all schema connections and remove the isolated schema.
    pub async fn cleanup(self) -> Result<()> {
        self.pool.close().await;
        let mut connection = PgConnection::connect(&self.database_url).await?;
        sqlx::query(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema))
            .execute(&mut connection)
            .await?;
        Ok(())
    }
}

fn migration_paths() -> Result<Vec<PathBuf>> {
    let migrations_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    let entries = std::fs::read_dir(&migrations_dir)
        .map_err(|error| Error::Io(format!("{}: {error}", migrations_dir.display())))?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| Error::Io(format!("{}: {error}", migrations_dir.display())))?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("sql") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn is_deadlock(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(error) if error.code().as_deref() == Some("40P01"))
}
