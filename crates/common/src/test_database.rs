//! Shared PostgreSQL fixture for integration tests.

use crate::{config::DatabaseConfig, db::Database, Error, Result};
use futures::{future::BoxFuture, stream::BoxStream};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Describe, Either, Execute, Executor, PgConnection, PgPool, Postgres};
use std::{ops::Deref, path::PathBuf, sync::OnceLock, time::Duration};
use url::Url;

const MIGRATION_LOCK_KEY: i64 = 78_210_014;
const TEST_SCHEMA: &str = "attune";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
// Four test workers can legitimately queue behind template creation on
// constrained container hosts. Keep the wait bounded while
// allowing the owned parallel lane to drain instead of failing at 30 seconds.
const MIGRATION_LOCK_TIMEOUT: Duration = Duration::from_secs(300);
const POOL_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const DATABASE_DDL_TIMEOUT: Duration = Duration::from_secs(30);

/// A fully migrated, database-isolated test fixture.
///
/// Migrations are applied once to a run-owned template database. Each test then
/// receives a physical PostgreSQL clone, preserving production schema,
/// extension, trigger, and TimescaleDB behavior without replaying every
/// migration hundreds of times.
#[derive(Debug)]
pub struct TestDatabase {
    pool: Option<PgPool>,
    database_name: Option<String>,
    database_url: String,
    admin_url: String,
    cleanup_on_drop: bool,
}

/// A test database clone whose lifecycle is managed by another process.
#[derive(Debug)]
pub struct DetachedTestDatabase {
    database_name: String,
    database_url: String,
    admin_url: String,
}

impl DetachedTestDatabase {
    pub fn database_name(&self) -> &str {
        &self.database_name
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }
}

impl TestDatabase {
    /// Clone a fully migrated template into a uniquely owned test database.
    pub async fn create(config: &DatabaseConfig) -> Result<Self> {
        let detached = Self::create_detached(config).await?;
        let DetachedTestDatabase {
            database_name,
            database_url,
            admin_url,
        } = detached;

        let mut database_config = config.clone();
        database_config.url = database_url.clone();
        database_config.schema = Some(TEST_SCHEMA.to_string());
        let pool = match Database::new(&database_config).await {
            Ok(database) => database.pool().clone(),
            Err(setup_error) => {
                return match drop_database(&admin_url, &database_name).await {
                    Ok(()) => Err(setup_error),
                    Err(cleanup_error) => Err(Error::InvalidState(format!(
                        "test database setup failed: {setup_error}; database cleanup failed: {cleanup_error}"
                    ))),
                };
            }
        };

        Ok(Self {
            pool: Some(pool),
            database_name: Some(database_name),
            database_url,
            admin_url,
            cleanup_on_drop: false,
        })
    }

    /// Create a migrated clone without opening a runtime-bound connection pool.
    pub async fn create_detached(config: &DatabaseConfig) -> Result<DetachedTestDatabase> {
        let migrations = migration_sql()?;
        let run_token = test_run_token()?;
        let template_name = template_database_name(&run_token, migrations);
        let database_name = test_database_name(&run_token);
        let admin_url = database_url_with_name(&config.url, "postgres")?;
        let database_url = database_url_with_name(&config.url, &database_name)?;

        ensure_template_database(&admin_url, &template_name, migrations).await?;

        let mut admin = connect_with_timeout(&admin_url, "test database clone").await?;
        let clone_result = tokio::time::timeout(
            DATABASE_DDL_TIMEOUT,
            sqlx::query(&format!(
                "CREATE DATABASE {database_name} TEMPLATE {template_name}"
            ))
            .execute(&mut admin),
        )
        .await
        .map_err(|_| {
            Error::InvalidState(format!(
                "timed out after {}s cloning test database {database_name}",
                DATABASE_DDL_TIMEOUT.as_secs()
            ))
        })?;
        if let Err(error) = clone_result {
            return Err(error.into());
        }
        drop(admin);

        Ok(DetachedTestDatabase {
            database_name,
            database_url,
            admin_url,
        })
    }

    /// Drop a detached clone created for this test run.
    pub async fn cleanup_detached(config: &DatabaseConfig, database_name: &str) -> Result<()> {
        let run_token = test_run_token()?;
        let owned_prefix = format!("attune_db_{run_token}_");
        let suffix = database_name
            .strip_prefix(&owned_prefix)
            .unwrap_or_default();
        if suffix.len() != 32
            || !suffix
                .chars()
                .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
        {
            return Err(Error::InvalidState(format!(
                "refusing to drop database outside this test run: {database_name}"
            )));
        }

        let database_url = database_url_with_name(&config.url, database_name)?;
        let admin_url = database_url_with_name(&config.url, "postgres")?;
        cleanup_parts(None, &database_url, &admin_url, database_name).await
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
        TEST_SCHEMA
    }

    pub fn database_name(&self) -> &str {
        self.database_name
            .as_deref()
            .expect("test database already cleaned up")
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    /// Close all clone connections and remove the isolated database.
    pub async fn cleanup(mut self) -> Result<()> {
        let pool = self.pool.take();
        let database_name = self
            .database_name
            .take()
            .expect("test database already cleaned up");
        cleanup_parts(pool, &self.database_url, &self.admin_url, &database_name).await
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
        let (Some(pool), Some(database_name)) = (self.pool.take(), self.database_name.take())
        else {
            return;
        };
        let database_url = self.database_url.clone();
        let admin_url = self.admin_url.clone();
        let database_label = database_name.clone();
        let cleanup = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?;
            runtime
                .block_on(fallback_cleanup(
                    pool,
                    &database_url,
                    &admin_url,
                    &database_name,
                ))
                .map_err(|error| error.to_string())
        });

        match cleanup.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                eprintln!("Failed to clean up test database {database_label}: {error}")
            }
            Err(_) => eprintln!("Test database cleanup thread panicked for {database_label}"),
        }
    }
}

async fn cleanup_parts(
    pool: Option<PgPool>,
    database_url: &str,
    admin_url: &str,
    database_name: &str,
) -> Result<()> {
    let mut failures = Vec::new();
    if let Err(error) = stop_background_workers(database_url).await {
        failures.push(format!("background worker stop failed: {error}"));
    }
    if let Some(pool) = pool {
        // Signal all clones to close, but always continue to the independently
        // bounded database teardown so one leaked checkout cannot hang cleanup.
        if tokio::time::timeout(POOL_CLOSE_TIMEOUT, pool.close())
            .await
            .is_err()
        {
            failures.push(format!(
                "pool close timed out after {}s",
                POOL_CLOSE_TIMEOUT.as_secs()
            ));
        }
    }
    if let Err(error) = drop_database(admin_url, database_name).await {
        failures.push(format!("database drop failed: {error}"));
    }

    cleanup_result(database_name, failures)
}

async fn fallback_cleanup(
    pool: PgPool,
    database_url: &str,
    admin_url: &str,
    database_name: &str,
) -> Result<()> {
    let mut failures = Vec::new();
    if let Err(error) = stop_background_workers(database_url).await {
        failures.push(format!("background worker stop failed: {error}"));
    }
    drop(pool);
    if let Err(error) = drop_database(admin_url, database_name).await {
        failures.push(format!("database drop failed: {error}"));
    }
    cleanup_result(database_name, failures)
}

async fn stop_background_workers(database_url: &str) -> Result<()> {
    let mut connection =
        connect_with_timeout(database_url, "test background worker cleanup").await?;
    tokio::time::timeout(
        POOL_CLOSE_TIMEOUT,
        sqlx::query("SELECT _timescaledb_functions.stop_background_workers()")
            .execute(&mut connection),
    )
    .await
    .map_err(|_| {
        Error::InvalidState(format!(
            "timed out after {}s stopping test database background workers",
            POOL_CLOSE_TIMEOUT.as_secs()
        ))
    })??;
    Ok(())
}

fn cleanup_result(database_name: &str, failures: Vec<String>) -> Result<()> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::InvalidState(format!(
            "test database cleanup for {database_name} was incomplete: {}",
            failures.join("; ")
        )))
    }
}

async fn connect_with_timeout(database_url: &str, stage: &str) -> Result<PgConnection> {
    tokio::time::timeout(CONNECT_TIMEOUT, PgConnection::connect(database_url))
        .await
        .map_err(|_| {
            Error::InvalidState(format!(
                "{stage} connection timed out after {}s",
                CONNECT_TIMEOUT.as_secs()
            ))
        })?
        .map_err(Into::into)
}

async fn ensure_template_database(
    admin_url: &str,
    template_name: &str,
    migrations: &[(PathBuf, String)],
) -> Result<()> {
    let mut admin = connect_with_timeout(admin_url, "test template setup").await?;
    tokio::time::timeout(
        MIGRATION_LOCK_TIMEOUT,
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut admin),
    )
    .await
    .map_err(|_| {
        Error::InvalidState(format!(
            "timed out after {}s waiting for the test template lock",
            MIGRATION_LOCK_TIMEOUT.as_secs()
        ))
    })??;

    let setup_result: Result<()> = async {
        let template_state: Option<(bool, bool)> = sqlx::query_as(
            "SELECT datallowconn, datistemplate FROM pg_database WHERE datname = $1",
        )
        .bind(template_name)
        .fetch_optional(&mut admin)
        .await?;
        if template_state == Some((false, true)) {
            return Ok(());
        }
        if template_state.is_some() {
            sqlx::query(&format!("DROP DATABASE {template_name} WITH (FORCE)"))
                .execute(&mut admin)
                .await?;
        }

        sqlx::query(&format!(
            "CREATE DATABASE {template_name} TEMPLATE template0"
        ))
        .execute(&mut admin)
        .await?;

        let template_url = database_url_with_name(admin_url, template_name)?;
        let migration_result: Result<()> = async {
            let mut template =
                connect_with_timeout(&template_url, "test template migration").await?;
            for (_, migration_sql) in migrations {
                for attempt in 1..=3 {
                    match sqlx::raw_sql(migration_sql).execute(&mut template).await {
                        Ok(_) => break,
                        Err(error) if is_deadlock(&error) && attempt < 3 => {
                            tokio::time::sleep(Duration::from_millis(100 * attempt)).await;
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            template.close().await?;
            Ok(())
        }
        .await;

        if let Err(error) = migration_result {
            let _ = sqlx::query(&format!(
                "DROP DATABASE IF EXISTS {template_name} WITH (FORCE)"
            ))
            .execute(&mut admin)
            .await;
            return Err(error);
        }

        // TimescaleDB background workers can retain a template connection.
        // Disallow new sessions first, then terminate any existing workers so
        // PostgreSQL can clone the database safely and deterministically.
        sqlx::query(&format!(
            "ALTER DATABASE {template_name} WITH ALLOW_CONNECTIONS false"
        ))
        .execute(&mut admin)
        .await?;
        sqlx::query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1")
            .bind(template_name)
            .execute(&mut admin)
            .await?;
        sqlx::query(&format!("ALTER DATABASE {template_name} IS_TEMPLATE true"))
            .execute(&mut admin)
            .await?;
        Ok(())
    }
    .await;

    let unlock_result = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut admin)
        .await;
    match (setup_result, unlock_result) {
        (Ok(()), Ok(_)) => Ok(()),
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
    }
}

async fn drop_database(admin_url: &str, database_name: &str) -> Result<()> {
    let mut connection = connect_with_timeout(admin_url, "test database cleanup").await?;
    tokio::time::timeout(
        DATABASE_DDL_TIMEOUT,
        sqlx::query(&format!(
            "DROP DATABASE IF EXISTS {database_name} WITH (FORCE)"
        ))
        .execute(&mut connection),
    )
    .await
    .map_err(|_| {
        Error::InvalidState(format!(
            "timed out after {}s dropping database {database_name}",
            DATABASE_DDL_TIMEOUT.as_secs()
        ))
    })??;
    Ok(())
}

fn test_run_token() -> Result<String> {
    let Some(run_id) = std::env::var_os("ATTUNE_TEST_RUN_ID") else {
        return Ok("local".to_string());
    };
    let run_id = run_id
        .into_string()
        .map_err(|_| Error::InvalidState("ATTUNE_TEST_RUN_ID must be UTF-8".to_string()))?;
    if run_id.is_empty()
        || run_id.len() > 20
        || !run_id.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')
        })
    {
        return Err(Error::InvalidState(
            "ATTUNE_TEST_RUN_ID must be 1-20 lowercase ASCII letters/digits with optional non-leading '-'"
                .to_string(),
        ));
    }
    Ok(run_id.replace('-', "_"))
}

fn template_database_name(run_token: &str, migrations: &[(PathBuf, String)]) -> String {
    let mut digest = Sha256::new();
    for (path, sql) in migrations {
        digest.update(path.as_os_str().as_encoded_bytes());
        digest.update([0]);
        digest.update(sql.as_bytes());
        digest.update([0]);
    }
    let hash = digest
        .finalize()
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("attune_tpl_{run_token}_{hash}")
}

fn test_database_name(run_token: &str) -> String {
    format!("attune_db_{run_token}_{}", uuid::Uuid::new_v4().simple())
}

pub fn migration_database_name(nonce: &str) -> Result<String> {
    if nonce.len() != 24 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::InvalidState(
            "migration database nonce must be exactly 24 ASCII hex characters".to_string(),
        ));
    }
    Ok(format!(
        "attune_migration_{}_{}",
        test_run_token()?,
        nonce.to_ascii_lowercase()
    ))
}

fn database_url_with_name(database_url: &str, database_name: &str) -> Result<String> {
    let mut url = Url::parse(database_url)
        .map_err(|error| Error::InvalidState(format!("invalid test database URL: {error}")))?;
    url.set_path(&format!("/{database_name}"));
    Ok(url.into())
}

fn migration_sql() -> Result<&'static Vec<(PathBuf, String)>> {
    static MIGRATIONS: OnceLock<Vec<(PathBuf, String)>> = OnceLock::new();
    if let Some(migrations) = MIGRATIONS.get() {
        return Ok(migrations);
    }

    let migrations_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    let entries = std::fs::read_dir(&migrations_dir)
        .map_err(|error| Error::Io(format!("{}: {error}", migrations_dir.display())))?;
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| Error::Io(format!("{}: {error}", migrations_dir.display())))?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("sql")
            && !path
                .file_name()
                .is_some_and(|name| name == "20240101000000_migration_runner_claim.sql")
        {
            paths.push(path);
        }
    }
    paths.sort();
    let loaded = paths
        .into_iter()
        .map(|path| {
            let sql = std::fs::read_to_string(&path)
                .map_err(|error| Error::Io(format!("{}: {error}", path.display())))?;
            Ok((path, sql))
        })
        .collect::<Result<Vec<_>>>()?;
    let _ = MIGRATIONS.set(loaded);
    Ok(MIGRATIONS
        .get()
        .expect("migration cache initialized by this or a concurrent caller"))
}

fn is_deadlock(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(error) if error.code().as_deref() == Some("40P01"))
}
