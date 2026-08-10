//! Shared PostgreSQL fixture for integration tests.

use crate::{config::DatabaseConfig, db::Database, Error, Result};
use futures::{future::BoxFuture, stream::BoxStream};
use sqlx::{Connection, Describe, Either, Execute, Executor, PgConnection, PgPool, Postgres};
use std::{ops::Deref, path::PathBuf, time::Duration};

const MIGRATION_LOCK_KEY: i64 = 78_210_014;
const DEFAULT_SEARCH_PATH: &str = "SET search_path TO attune, public;";

/// A fully migrated, schema-isolated test database.
#[derive(Debug)]
pub struct TestDatabase {
    pool: Option<PgPool>,
    schema: Option<String>,
    database_url: String,
    cleanup_on_drop: bool,
}

impl TestDatabase {
    /// Create a unique schema and apply every migration in order.
    pub async fn create(config: &DatabaseConfig) -> Result<Self> {
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let mut connection = PgConnection::connect(&config.url).await?;

        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&mut connection)
            .await?;

        let setup_result: Result<PgPool> = async {
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
            Ok(Database::new(&schema_config).await?.pool().clone())
        }
        .await;

        let pool = match setup_result {
            Ok(pool) => pool,
            Err(setup_error) => {
                let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
                    .bind(MIGRATION_LOCK_KEY)
                    .execute(&mut connection)
                    .await;
                drop(connection);
                return match drop_schema(&config.url, &schema).await {
                    Ok(()) => Err(setup_error),
                    Err(cleanup_error) => Err(Error::InvalidState(format!(
                        "test database setup failed: {setup_error}; schema cleanup failed: {cleanup_error}"
                    ))),
                };
            }
        };

        Ok(Self {
            pool: Some(pool),
            schema: Some(schema),
            database_url: config.url.clone(),
            cleanup_on_drop: false,
        })
    }

    /// Ensure the schema is removed if the owner reaches the end of its scope.
    pub fn with_cleanup_on_drop(mut self) -> Self {
        self.cleanup_on_drop = true;
        self
    }

    pub fn pool(&self) -> &PgPool {
        self.pool
            .as_ref()
            .expect("test database already cleaned up")
    }

    pub fn schema(&self) -> &str {
        self.schema
            .as_deref()
            .expect("test database already cleaned up")
    }

    /// Close all schema connections and remove the isolated schema.
    pub async fn cleanup(mut self) -> Result<()> {
        let pool = self.pool.take();
        let schema = self
            .schema
            .take()
            .expect("test database already cleaned up");
        cleanup_parts(pool, &self.database_url, &schema).await
    }
}

impl Deref for TestDatabase {
    type Target = PgPool;

    fn deref(&self) -> &Self::Target {
        self.pool()
    }
}

impl<'p> Executor<'p> for &'p TestDatabase {
    type Database = Postgres;

    fn fetch_many<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxStream<
        'e,
        std::result::Result<
            Either<<Postgres as sqlx::Database>::QueryResult, <Postgres as sqlx::Database>::Row>,
            sqlx::Error,
        >,
    >
    where
        E: 'q + Execute<'q, Self::Database>,
    {
        self.pool().fetch_many(query)
    }

    fn fetch_optional<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxFuture<'e, std::result::Result<Option<<Postgres as sqlx::Database>::Row>, sqlx::Error>>
    where
        E: 'q + Execute<'q, Self::Database>,
    {
        self.pool().fetch_optional(query)
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [<Postgres as sqlx::Database>::TypeInfo],
    ) -> BoxFuture<'e, std::result::Result<<Postgres as sqlx::Database>::Statement<'q>, sqlx::Error>>
    {
        self.pool().prepare_with(sql, parameters)
    }

    fn describe<'e, 'q: 'e>(
        self,
        sql: &'q str,
    ) -> BoxFuture<'e, std::result::Result<Describe<Self::Database>, sqlx::Error>> {
        self.pool().describe(sql)
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        if !self.cleanup_on_drop {
            return;
        }
        let (Some(pool), Some(schema)) = (self.pool.take(), self.schema.take()) else {
            return;
        };
        let database_url = self.database_url.clone();
        let schema_label = schema.clone();
        let cleanup = std::thread::spawn(move || {
            // Other test components may retain pool clones. Dropping this owner
            // avoids waiting indefinitely while the independent connection
            // removes the schema underneath any idle clones.
            drop(pool);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?;
            runtime
                .block_on(drop_schema(&database_url, &schema))
                .map_err(|error| error.to_string())
        });

        match cleanup.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                eprintln!("Failed to clean up test schema {schema_label}: {error}")
            }
            Err(_) => eprintln!("Test schema cleanup thread panicked for {schema_label}"),
        }
    }
}

async fn cleanup_parts(pool: Option<PgPool>, database_url: &str, schema: &str) -> Result<()> {
    if let Some(pool) = pool {
        // Signal all clones to close, but do not let a leaked checkout prevent
        // the independent schema teardown from running.
        let _ = tokio::time::timeout(Duration::from_secs(5), pool.close()).await;
    }
    drop_schema(database_url, schema).await
}

async fn drop_schema(database_url: &str, schema: &str) -> Result<()> {
    let mut connection = PgConnection::connect(database_url).await?;
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = $1 AND pid <> pg_backend_pid()",
    )
    .bind(format!("attune:{schema}"))
    .execute(&mut connection)
    .await?;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&mut connection)
        .await?;
    Ok(())
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
