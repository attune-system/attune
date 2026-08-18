//! Integration tests for database migrations
//!
//! These tests verify that migrations run successfully, the schema is correct,
//! and basic database operations work as expected.

mod helpers;

use attune_common::{config::Config, db::Database, Error};
use helpers::*;
use sqlx::{migrate::MigrateDatabase, Postgres, Row};

const STANDARD_PACK_INDEX_URL: &str = attune_common::pack_registry::STANDARD_PACK_INDEX_URL;
const LIVE_STANDARD_PACK_INDEX_URL: &str =
    "https://raw.githubusercontent.com/attune-system/index/main/index.json";
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

fn standard_index_migration(schema: &str) -> String {
    include_str!("../../../migrations/20250101000026_standard_pack_index.sql").replace(
        "SET search_path TO attune, public;",
        &format!("SET search_path TO {schema}, public;"),
    )
}

fn v021_forward_migration(schema: &str) -> String {
    include_str!("../../../migrations/20250101000027_v021_upgrade_compatibility.sql").replace(
        "SET search_path TO attune, public;",
        &format!("SET search_path TO {schema}, public;"),
    )
}

fn docker_setup_sql() -> &'static str {
    include_str!("../../../docker/run-migrations.sh")
        .split_once("<<-'EOSQL' || return 1\n")
        .unwrap()
        .1
        .split_once("\nEOSQL")
        .unwrap()
        .0
}

async fn execute_docker_setup(database: &Database) -> Result<(), sqlx::Error> {
    let mut connection = database.pool().acquire().await.unwrap();
    let result = sqlx::raw_sql(docker_setup_sql())
        .execute(&mut *connection)
        .await;
    if result.is_err() {
        sqlx::query("ROLLBACK")
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    result.map(|_| ())
}

async fn create_embedded_migration_database() -> (Database, String) {
    let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
    let mut config = Config::load_from_file(&config_path).unwrap();
    let database_name = format!("attune_migration_{}", uuid::Uuid::new_v4().simple());
    let mut database_url = url::Url::parse(&config.database.url).unwrap();
    database_url.set_path(&format!("/{database_name}"));
    let database_url = database_url.to_string();

    Postgres::create_database(&database_url).await.unwrap();
    config.database.url.clone_from(&database_url);
    config.database.schema = None;
    config.database.max_connections = 2;

    let database = Database::new(&config.database).await.unwrap();
    (database, database_url)
}

async fn set_migration_checksum(database: &Database, version: i64, checksum: &str) {
    sqlx::query("UPDATE _sqlx_migrations SET checksum = decode($2, 'hex') WHERE version = $1")
        .bind(version)
        .bind(checksum)
        .execute(database.pool())
        .await
        .unwrap();
}

async fn prepare_v021_sqlx_database(database: &Database, attune_scoped_history: bool) {
    if attune_scoped_history {
        sqlx::query("CREATE SCHEMA IF NOT EXISTS attune")
            .execute(database.pool())
            .await
            .unwrap();
    }
    let history_table = if attune_scoped_history {
        "attune._sqlx_migrations"
    } else {
        "public._sqlx_migrations"
    };
    let create_history = format!(
        r#"
        CREATE TABLE {history_table} (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
            success BOOLEAN NOT NULL,
            checksum BYTEA NOT NULL,
            execution_time BIGINT NOT NULL
        )
        "#
    );
    sqlx::query(&create_history)
        .execute(database.pool())
        .await
        .unwrap();

    let migrator = sqlx::migrate!("../../migrations");
    for migration in migrator
        .iter()
        .filter(|migration| (20250101000001..=20250101000020).contains(&migration.version))
    {
        sqlx::raw_sql(&migration.sql)
            .execute(database.pool())
            .await
            .unwrap();
        let checksum = V021_MIGRATION_CHECKSUMS
            .iter()
            .find(|(version, _, _)| *version == migration.version)
            .map(|(_, legacy, _)| (*legacy).to_string())
            .unwrap_or_else(|| {
                migration
                    .checksum
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect()
            });
        let insert_history = format!(
            r#"
            INSERT INTO {history_table}
                (version, description, success, checksum, execution_time)
            VALUES ($1, $2, TRUE, decode($3, 'hex'), 0)
            "#
        );
        sqlx::query(&insert_history)
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(checksum)
            .execute(database.pool())
            .await
            .unwrap();
    }

    sqlx::query("DROP FUNCTION enforce_dashboard_default_home() CASCADE")
        .execute(database.pool())
        .await
        .unwrap();
}

async fn assert_dashboard_default_home_behavior(pool: &sqlx::PgPool) {
    sqlx::query(
        r#"
        INSERT INTO dashboard (ref, label, is_default_home, spec_version, spec)
        VALUES ('compat.first', 'First', TRUE, 1, '{}'::jsonb)
        "#,
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO dashboard (ref, label, is_default_home, spec_version, spec)
        VALUES ('compat.second', 'Second', TRUE, 1, '{}'::jsonb)
        "#,
    )
    .execute(pool)
    .await
    .unwrap();

    let rows = sqlx::query(
        "SELECT ref, is_default_home, revision FROM dashboard WHERE ref LIKE 'compat.%' ORDER BY ref",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<String, _>("ref"), "compat.first");
    assert!(!rows[0].get::<bool, _>("is_default_home"));
    assert_eq!(rows[0].get::<i32, _>("revision"), 2);
    assert_eq!(rows[1].get::<String, _>("ref"), "compat.second");
    assert!(rows[1].get::<bool, _>("is_default_home"));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_migrations_applied() {
    let pool = create_test_pool().await.unwrap();

    // Verify migrations were applied by checking that core tables exist
    // We check for multiple tables to ensure the schema is properly set up
    let tables = vec!["pack", "action", "trigger", "rule", "execution"];

    for table_name in tables {
        let row = sqlx::query(&format!(
            r#"
            SELECT EXISTS (
                SELECT FROM information_schema.tables
                WHERE table_schema = current_schema()
                AND table_name = '{}'
            ) as exists
            "#,
            table_name
        ))
        .fetch_one(&pool)
        .await
        .unwrap();

        let exists: bool = row.get("exists");
        assert!(
            exists,
            "Table '{}' does not exist - migrations may not have run",
            table_name
        );
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn standard_pack_index_is_seeded() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT name, url, position, enabled, headers
        FROM pack_registry_index
        WHERE url = $1
        "#,
    )
    .bind(STANDARD_PACK_INDEX_URL)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.get::<String, _>("name"), "Attune Standard Pack Index");
    assert_eq!(row.get::<String, _>("url"), STANDARD_PACK_INDEX_URL);
    assert_eq!(row.get::<i32, _>("position"), 0);
    assert!(row.get::<bool, _>("enabled"));
    assert_eq!(
        row.get::<serde_json::Value, _>("headers"),
        serde_json::json!({})
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn embedded_migrator_rejects_custom_schema() {
    let pool = create_test_pool().await.unwrap();

    sqlx::query("DELETE FROM pack_registry_index WHERE url = $1")
        .bind(STANDARD_PACK_INDEX_URL)
        .execute(&pool)
        .await
        .unwrap();

    let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
    let mut config = Config::load_from_file(&config_path).unwrap();
    config.database.schema = Some(pool.schema().to_string());
    let database = Database::new(&config.database).await.unwrap();

    let error = database.migrate().await.unwrap_err();
    assert!(matches!(&error, Error::InvalidState(_)));
    assert!(error.to_string().contains(pool.schema()));

    let seed_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pack_registry_index WHERE url = $1")
            .bind(STANDARD_PACK_INDEX_URL)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(seed_count, 0);

    database.close().await;
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn embedded_migrator_bridges_v021_checksums_and_applies_forward_delta() {
    let (database, database_url) = create_embedded_migration_database().await;
    prepare_v021_sqlx_database(&database, false).await;

    set_migration_checksum(&database, 20250101000014, &"00".repeat(48)).await;
    let error = database.migrate().await.unwrap_err();
    assert!(matches!(&error, Error::InvalidState(_)));
    assert!(error.to_string().contains("20250101000014"));

    let version_9_checksum: String = sqlx::query_scalar(
        "SELECT encode(checksum, 'hex') FROM _sqlx_migrations WHERE version = 20250101000009",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(version_9_checksum, V021_MIGRATION_CHECKSUMS[0].1);

    set_migration_checksum(&database, 20250101000014, V021_MIGRATION_CHECKSUMS[2].1).await;
    database.migrate().await.unwrap();

    for &(version, _, current_checksum) in V021_MIGRATION_CHECKSUMS {
        let checksum: String = sqlx::query_scalar(
            "SELECT encode(checksum, 'hex') FROM _sqlx_migrations WHERE version = $1",
        )
        .bind(version)
        .fetch_one(database.pool())
        .await
        .unwrap();
        assert_eq!(checksum, current_checksum);
    }
    let forward_migration_applied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version = 20250101000027 AND success)",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert!(forward_migration_applied);
    assert_dashboard_default_home_behavior(database.pool()).await;

    database.migrate().await.unwrap();
    database.close().await;
    Postgres::force_drop_database(&database_url).await.unwrap();
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn embedded_migrator_uses_attune_history_on_fresh_database() {
    let (database, database_url) = create_embedded_migration_database().await;

    database.migrate().await.unwrap();

    let (has_attune_history, has_public_history, runner): (bool, bool, String) = sqlx::query_as(
        r#"
            SELECT
                to_regclass('attune._sqlx_migrations') IS NOT NULL,
                to_regclass('public._sqlx_migrations') IS NOT NULL,
                (SELECT runner FROM public._attune_migration_runner WHERE id = 1)
            "#,
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert!(has_attune_history);
    assert!(!has_public_history);
    assert_eq!(runner, "sqlx");

    let runner_claim_recorded: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM attune._sqlx_migrations WHERE version = 20240101000000 AND success)",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert!(runner_claim_recorded);

    database.migrate().await.unwrap();
    database.close().await;
    Postgres::force_drop_database(&database_url).await.unwrap();
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn embedded_migrator_upgrades_attune_scoped_v021_history() {
    let (database, database_url) = create_embedded_migration_database().await;
    prepare_v021_sqlx_database(&database, true).await;

    database.migrate().await.unwrap();

    let runner_claim_recorded: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM attune._sqlx_migrations WHERE version = 20240101000000 AND success)",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert!(runner_claim_recorded);
    let has_public_history: bool =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(database.pool())
            .await
            .unwrap();
    assert!(!has_public_history);
    assert_dashboard_default_home_behavior(database.pool()).await;

    database.close().await;
    Postgres::force_drop_database(&database_url).await.unwrap();
}

#[test]
fn docker_migration_runner_uses_an_isolated_temporary_log() {
    let script = include_str!("../../../docker/run-migrations.sh");

    assert!(script.contains("mktemp \"${TMPDIR:-/tmp}/attune-migration.XXXXXX\""));
    assert!(script.contains("trap cleanup EXIT"));
    assert!(script.contains("> \"$MIGRATION_OUTPUT\" 2>&1"));
    assert!(!script.contains("/tmp/migration_output.log"));
}

#[test]
fn docker_migration_runner_hashes_and_validates_exact_file_bytes() {
    let script = include_str!("../../../docker/run-migrations.sh");

    assert!(script.contains("\\lo_import :migration_filepath"));
    assert!(script.contains("encode(sha384(lo_get(:LASTOID)), 'hex')"));
    assert!(script.contains("checksum_sha384 = :'migration_checksum'"));
    assert!(script.contains("Migration SHA-384 checksum does not match the stored history"));
    assert!(script.contains("SET legacy_checksum_adoption = FALSE"));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn docker_runner_moves_public_history_to_attune() {
    let (database, database_url) = create_embedded_migration_database().await;
    sqlx::raw_sql(
        r#"
        CREATE TABLE public._migrations (
            id SERIAL PRIMARY KEY,
            filename VARCHAR(255) UNIQUE NOT NULL,
            applied_at TIMESTAMP DEFAULT NOW()
        );
        INSERT INTO public._migrations (filename) VALUES ('legacy.sql');
        "#,
    )
    .execute(database.pool())
    .await
    .unwrap();

    execute_docker_setup(&database).await.unwrap();
    // A retry before the full migration-file pass completes must retain the
    // compatibility window for legacy rows not reached by the first process.
    execute_docker_setup(&database).await.unwrap();

    let (has_attune, has_public, runner, filename, checksum, adoption): (
        bool,
        bool,
        String,
        String,
        Option<String>,
        bool,
    ) = sqlx::query_as(
        r#"
            SELECT
                to_regclass('attune._migrations') IS NOT NULL,
                to_regclass('public._migrations') IS NOT NULL,
                (SELECT runner FROM public._attune_migration_runner WHERE id = 1),
                (SELECT filename FROM attune._migrations WHERE filename = 'legacy.sql'),
                (SELECT checksum_sha384 FROM attune._migrations WHERE filename = 'legacy.sql'),
                (SELECT legacy_checksum_adoption FROM public._attune_migration_runner WHERE id = 1)
            "#,
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert!(has_attune);
    assert!(!has_public);
    assert_eq!(runner, "docker");
    assert_eq!(filename, "legacy.sql");
    assert_eq!(checksum, None);
    assert!(adoption);

    sqlx::query(
        "UPDATE public._attune_migration_runner SET legacy_checksum_adoption = FALSE WHERE id = 1",
    )
    .execute(database.pool())
    .await
    .unwrap();
    execute_docker_setup(&database).await.unwrap();
    let adoption_reopened: bool = sqlx::query_scalar(
        "SELECT legacy_checksum_adoption FROM public._attune_migration_runner WHERE id = 1",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert!(!adoption_reopened);

    database.close().await;
    Postgres::force_drop_database(&database_url).await.unwrap();
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn docker_runner_fresh_history_requires_sha384_checksums() {
    let (database, database_url) = create_embedded_migration_database().await;

    execute_docker_setup(&database).await.unwrap();

    let (checksum_nullable, adoption, sha384_abc): (String, bool, String) = sqlx::query_as(
        r#"
            SELECT
                (SELECT is_nullable
                 FROM information_schema.columns
                 WHERE table_schema = 'attune'
                   AND table_name = '_migrations'
                   AND column_name = 'checksum_sha384'),
                (SELECT legacy_checksum_adoption
                 FROM public._attune_migration_runner
                 WHERE id = 1),
                encode(sha384(convert_to('abc', 'UTF8')), 'hex')
            "#,
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(checksum_nullable, "NO");
    assert!(!adoption);
    assert_eq!(
        sha384_abc,
        "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
    );

    let missing_checksum =
        sqlx::query("INSERT INTO attune._migrations (filename) VALUES ('missing.sql')")
            .execute(database.pool())
            .await;
    assert!(missing_checksum.is_err());

    database.close().await;
    Postgres::force_drop_database(&database_url).await.unwrap();
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn docker_runner_rejects_dual_docker_history_without_claim() {
    let (database, database_url) = create_embedded_migration_database().await;
    sqlx::raw_sql(
        r#"
        CREATE SCHEMA attune;
        CREATE TABLE attune._migrations (filename TEXT PRIMARY KEY);
        CREATE TABLE public._migrations (filename TEXT PRIMARY KEY);
        "#,
    )
    .execute(database.pool())
    .await
    .unwrap();

    let error = execute_docker_setup(&database).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("Docker migration history in both attune and public schemas"));

    let has_claim: bool =
        sqlx::query_scalar("SELECT to_regclass('public._attune_migration_runner') IS NOT NULL")
            .fetch_one(database.pool())
            .await
            .unwrap();
    assert!(
        !has_claim,
        "rejected detection must roll back the runner claim"
    );

    database.close().await;
    Postgres::force_drop_database(&database_url).await.unwrap();
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn docker_runner_rejects_dual_sqlx_history_without_claim() {
    let (database, database_url) = create_embedded_migration_database().await;
    sqlx::raw_sql(
        r#"
        CREATE SCHEMA attune;
        CREATE TABLE attune._sqlx_migrations (version BIGINT PRIMARY KEY);
        CREATE TABLE public._sqlx_migrations (version BIGINT PRIMARY KEY);
        "#,
    )
    .execute(database.pool())
    .await
    .unwrap();

    let error = execute_docker_setup(&database).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("SQLx migration history in both attune and public schemas"));

    let has_claim: bool =
        sqlx::query_scalar("SELECT to_regclass('public._attune_migration_runner') IS NOT NULL")
            .fetch_one(database.pool())
            .await
            .unwrap();
    assert!(
        !has_claim,
        "rejected detection must roll back the runner claim"
    );

    database.close().await;
    Postgres::force_drop_database(&database_url).await.unwrap();
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn docker_runner_rejects_mixed_sqlx_and_docker_history_without_claim() {
    let (database, database_url) = create_embedded_migration_database().await;
    sqlx::raw_sql(
        r#"
        CREATE SCHEMA attune;
        CREATE TABLE attune._migrations (filename TEXT PRIMARY KEY);
        CREATE TABLE public._sqlx_migrations (version BIGINT PRIMARY KEY);
        "#,
    )
    .execute(database.pool())
    .await
    .unwrap();

    let error = execute_docker_setup(&database).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("mixed SQLx and Docker migration history"));

    let has_claim: bool =
        sqlx::query_scalar("SELECT to_regclass('public._attune_migration_runner') IS NOT NULL")
            .fetch_one(database.pool())
            .await
            .unwrap();
    assert!(
        !has_claim,
        "rejected detection must roll back the runner claim"
    );

    database.close().await;
    Postgres::force_drop_database(&database_url).await.unwrap();
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn v021_forward_migration_is_idempotent_for_filename_runner() {
    let pool = create_test_pool().await.unwrap();
    sqlx::query("DROP FUNCTION enforce_dashboard_default_home() CASCADE")
        .execute(&pool)
        .await
        .unwrap();

    let migration = v021_forward_migration(pool.schema());
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();

    let trigger_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_trigger WHERE tgrelid = 'dashboard'::regclass AND tgname = 'enforce_dashboard_default_home_trigger' AND NOT tgisinternal",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(trigger_count, 1);
    assert_dashboard_default_home_behavior(pool.pool()).await;
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn dashboard_cross_scope_default_moves_fail_fast_for_retry() {
    let pool = create_test_pool().await.unwrap();
    let migration = v021_forward_migration(pool.schema());
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();

    let first_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO dashboard
            (ref, scope_type, scope_ref, label, is_default_home, spec_version, spec)
        VALUES ('concurrency.first', 'global', 'scope-a', 'First', TRUE, 1, '{}'::jsonb)
        RETURNING id
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let second_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO dashboard
            (ref, scope_type, scope_ref, label, is_default_home, spec_version, spec)
        VALUES ('concurrency.second', 'global', 'scope-b', 'Second', TRUE, 1, '{}'::jsonb)
        RETURNING id
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let mut first_tx = pool.begin().await.unwrap();
    let mut second_tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM dashboard WHERE id = $1 FOR UPDATE")
        .bind(first_id)
        .execute(&mut *first_tx)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM dashboard WHERE id = $1 FOR UPDATE")
        .bind(second_id)
        .execute(&mut *second_tx)
        .await
        .unwrap();

    let move_results = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(
            sqlx::query("UPDATE dashboard SET scope_ref = 'scope-b' WHERE id = $1")
                .bind(first_id)
                .execute(&mut *first_tx),
            sqlx::query("UPDATE dashboard SET scope_ref = 'scope-a' WHERE id = $1")
                .bind(second_id)
                .execute(&mut *second_tx),
        )
    })
    .await;
    let (first_move, second_move) = match move_results {
        Ok(results) => results,
        Err(_) => {
            first_tx.rollback().await.unwrap();
            second_tx.rollback().await.unwrap();
            panic!("cross-scope moves did not complete within the deadlock bound");
        }
    };

    let first_won = first_move.is_ok();
    let second_won = second_move.is_ok();
    let first_error_code = first_move
        .as_ref()
        .err()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .map(|code| code.into_owned());
    let second_error_code = second_move
        .as_ref()
        .err()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .map(|code| code.into_owned());

    // Close every transaction before asserting so a failure cannot leak row,
    // advisory, or DDL-blocking locks into the next migration test.
    let mut first_tx = Some(first_tx);
    let mut second_tx = Some(second_tx);
    if !first_won {
        first_tx.take().unwrap().rollback().await.unwrap();
    }
    if !second_won {
        second_tx.take().unwrap().rollback().await.unwrap();
    }
    if first_won {
        first_tx.take().unwrap().commit().await.unwrap();
    }
    if second_won {
        second_tx.take().unwrap().commit().await.unwrap();
    }

    assert!(
        !first_won || !second_won,
        "at least one cross-scope move must lose and retry"
    );
    if !first_won {
        assert_eq!(first_error_code.as_deref(), Some("55P03"));
    }
    if !second_won {
        assert_eq!(second_error_code.as_deref(), Some("55P03"));
    }

    let duplicate_default_scopes: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM (
            SELECT scope_type, scope_ref
            FROM dashboard
            WHERE is_default_home = TRUE
              AND id = ANY($1)
            GROUP BY scope_type, scope_ref
            HAVING COUNT(*) > 1
        ) duplicates
        "#,
    )
    .bind(&[first_id, second_id][..])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(duplicate_default_scopes, 0);

    let winner_count = i64::from(first_won) + i64::from(second_won);
    let default_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dashboard WHERE id = ANY($1) AND is_default_home = TRUE",
    )
    .bind(&[first_id, second_id][..])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(default_count, 2 - winner_count);

    sqlx::query("UPDATE dashboard SET scope_ref = 'scope-b', is_default_home = TRUE WHERE id = $1")
        .bind(first_id)
        .execute(&pool)
        .await
        .unwrap();
    let destination_default: String = sqlx::query_scalar(
        "SELECT ref FROM dashboard WHERE scope_type = 'global' AND scope_ref = 'scope-b' AND is_default_home = TRUE",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(destination_default, "concurrency.first");
    let duplicate_default_scopes_after_retry: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM (
            SELECT scope_type, scope_ref
            FROM dashboard
            WHERE is_default_home = TRUE
              AND id = ANY($1)
            GROUP BY scope_type, scope_ref
            HAVING COUNT(*) > 1
        ) duplicates
        "#,
    )
    .bind(&[first_id, second_id][..])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(duplicate_default_scopes_after_retry, 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn standard_pack_index_seed_appends_once_and_preserves_admin_state() {
    let pool = create_test_pool().await.unwrap();
    let standard_url = STANDARD_PACK_INDEX_URL;

    sqlx::query("DELETE FROM pack_registry_index WHERE url = $1")
        .bind(standard_url)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO pack_registry_index (name, url, position, enabled)
        VALUES ('Company Packs', 'https://packs.example.com/index.json', 7, TRUE)
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let migration = standard_index_migration(pool.schema());
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();

    let seeded = sqlx::query("SELECT id, position FROM pack_registry_index WHERE url = $1")
        .bind(standard_url)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(seeded.get::<i32, _>("position"), 8);

    let seeded_id = seeded.get::<i64, _>("id");
    sqlx::query(
        "UPDATE pack_registry_index SET name = 'Admin Standard', enabled = FALSE WHERE id = $1",
    )
    .bind(seeded_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();

    let preserved = sqlx::query(
        "SELECT COUNT(*) AS count, MIN(name) AS name, BOOL_AND(NOT enabled) AS disabled FROM pack_registry_index WHERE url = $1",
    )
    .bind(standard_url)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(preserved.get::<i64, _>("count"), 1);
    assert_eq!(preserved.get::<String, _>("name"), "Admin Standard");
    assert!(preserved.get::<bool, _>("disabled"));

    sqlx::query("DELETE FROM pack_registry_index WHERE url = $1")
        .bind(standard_url)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE pack_registry_index SET position = $1 WHERE url = $2")
        .bind(i32::MAX)
        .bind("https://packs.example.com/index.json")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();

    let saturated_position: i32 =
        sqlx::query_scalar("SELECT position FROM pack_registry_index WHERE url = $1")
            .bind(standard_url)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(saturated_position, i32::MAX);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn standard_pack_index_seed_preserves_canonical_equivalent_row() {
    let pool = create_test_pool().await.unwrap();
    sqlx::query("DELETE FROM pack_registry_index WHERE url = $1")
        .bind(STANDARD_PACK_INDEX_URL)
        .execute(&pool)
        .await
        .unwrap();
    let equivalent = format!(
        "HTTPS://RAW.GITHUBUSERCONTENT.COM.:443/attune-system/index/{}/index.json",
        "793aabcc0eb537af7681a386b591de6c4fafd7a1"
    );
    sqlx::query(
        "INSERT INTO pack_registry_index (name, url, position, enabled, headers) VALUES ($1, $2, 4, FALSE, '{}'::jsonb)",
    )
    .bind("Administrator Standard")
    .bind(&equivalent)
    .execute(&pool)
    .await
    .unwrap();

    let migration = standard_index_migration(pool.schema());
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();

    let rows = sqlx::query(
        "SELECT name, url, position, enabled, headers FROM pack_registry_index WHERE url = $1 OR url = $2",
    )
    .bind(STANDARD_PACK_INDEX_URL)
    .bind(&equivalent)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String, _>("name"), "Administrator Standard");
    assert_eq!(rows[0].get::<String, _>("url"), equivalent);
    assert_eq!(rows[0].get::<i32, _>("position"), 4);
    assert!(!rows[0].get::<bool, _>("enabled"));
    assert_eq!(
        rows[0].get::<serde_json::Value, _>("headers"),
        serde_json::json!({})
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn pinned_standard_snapshot_is_distinct_from_existing_live_index() {
    let pool = create_test_pool().await.unwrap();
    sqlx::query("DELETE FROM pack_registry_index WHERE url = $1")
        .bind(STANDARD_PACK_INDEX_URL)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO pack_registry_index (name, url, position, enabled, headers) VALUES ('Live Standard', $1, 3, FALSE, '{}'::jsonb)",
    )
    .bind(LIVE_STANDARD_PACK_INDEX_URL)
    .execute(&pool)
    .await
    .unwrap();

    let migration = standard_index_migration(pool.schema());
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();

    let live = sqlx::query("SELECT position, enabled FROM pack_registry_index WHERE url = $1")
        .bind(LIVE_STANDARD_PACK_INDEX_URL)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(live.get::<i32, _>("position"), 3);
    assert!(!live.get::<bool, _>("enabled"));

    let pinned_position: i32 =
        sqlx::query_scalar("SELECT position FROM pack_registry_index WHERE url = $1")
            .bind(STANDARD_PACK_INDEX_URL)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pinned_position, 4);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn standard_index_migration_scrubs_legacy_query_credentials() {
    let pool = create_test_pool().await.unwrap();
    sqlx::query("ALTER TABLE pack ALTER COLUMN meta DROP NOT NULL")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO pack_registry_index (name, url, position, enabled, headers)
        VALUES
            ('Canonical Secret First', 'https://registry-canonical.example.com/index.json?token=secret', 8, TRUE, '{"Authorization":"Bearer preferred"}'::jsonb),
            ('Canonical Clean', 'HTTPS://REGISTRY-CANONICAL.EXAMPLE.COM.:443/index.json', 9, TRUE, '{"X-Lower-Priority":"discard"}'::jsonb),
            ('Clean', 'https://registry.example.com/index.json', 10, TRUE, '{"X-Admin":"preserve"}'::jsonb),
            ('Duplicate Secret', 'https://registry.example.com/index.json?token=secret', 11, TRUE, '{"X-Lower-Priority":"discard"}'::jsonb),
            ('Review Required', 'https://private.example.com/index.json?token=secret', 12, TRUE, '{}'::jsonb),
            ('Clean Canonical First', 'https://clean-only.example.com/index.json', 13, TRUE, '{"Authorization":"first"}'::jsonb),
            ('Clean Canonical Second', 'HTTPS://CLEAN-ONLY.EXAMPLE.COM.:443/index.json', 14, TRUE, '{"Authorization":"second"}'::jsonb)
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let preferred_canonical_id: i64 = sqlx::query_scalar(
        "SELECT id FROM pack_registry_index WHERE name = 'Canonical Secret First'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO pack (ref, label, version, source_type, source_url, source_ref, meta)
        VALUES
            ('legacy_query_pack', 'Legacy Query Pack', '1.0.0', 'archive', 'https://downloads.example.com/pack.tgz?token=secret', 'v1.0.0', '{"owner":"admin","_attune":{"existing":"preserved"}}'::jsonb),
            ('local_query_pack', 'Local Query Pack', '1.0.0', 'archive', '/srv/packs/release?candidate/pack.tgz', NULL, '{}'::jsonb),
            ('scalar_meta_pack', 'Scalar Meta Pack', '1.0.0', 'archive', 'https://downloads.example.com/scalar.tgz?token=secret', NULL, '"legacy-scalar"'::jsonb),
            ('array_meta_pack', 'Array Meta Pack', '1.0.0', 'archive', 'https://downloads.example.com/array.tgz?token=secret', NULL, '["legacy", 7]'::jsonb),
            ('null_meta_pack', 'Null Meta Pack', '1.0.0', 'archive', 'https://downloads.example.com/null.tgz?token=secret', NULL, NULL)
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO audit_event (category, event_type, outcome, details)
        VALUES
            ('pack', 'pack.installed', 'success', '{"source":"https://downloads.example.com/pack.tgz?token=secret"}'::jsonb),
            ('pack', 'pack.updated', 'success', '{"source":"https://unrelated.example.com/pack-update?token=preserved"}'::jsonb),
            ('api', 'pack.installed', 'success', '{"source":"https://unrelated.example.com/api-request?token=preserved"}'::jsonb)
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let migration = standard_index_migration(pool.schema());
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();

    let duplicate_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pack_registry_index WHERE url = 'https://registry.example.com/index.json'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(duplicate_count, 1);

    let canonical_rows = sqlx::query(
        "SELECT id, name, url, position, enabled, headers FROM pack_registry_index WHERE name LIKE 'Canonical %'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(canonical_rows.len(), 1);
    assert_eq!(
        canonical_rows[0].get::<i64, _>("id"),
        preferred_canonical_id
    );
    assert_eq!(
        canonical_rows[0].get::<String, _>("name"),
        "Canonical Secret First"
    );
    assert_eq!(
        canonical_rows[0].get::<String, _>("url"),
        "https://registry-canonical.example.com/index.json"
    );
    assert_eq!(canonical_rows[0].get::<i32, _>("position"), 8);
    assert!(!canonical_rows[0].get::<bool, _>("enabled"));
    assert_eq!(
        canonical_rows[0].get::<serde_json::Value, _>("headers"),
        serde_json::json!({"Authorization": "Bearer preferred"})
    );

    let clean_survivor = sqlx::query(
        "SELECT name, position, enabled, headers FROM pack_registry_index WHERE url = 'https://registry.example.com/index.json'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(clean_survivor.get::<String, _>("name"), "Clean");
    assert_eq!(clean_survivor.get::<i32, _>("position"), 10);
    assert!(!clean_survivor.get::<bool, _>("enabled"));
    assert_eq!(
        clean_survivor.get::<serde_json::Value, _>("headers"),
        serde_json::json!({"X-Admin": "preserve"})
    );

    let clean_canonical_rows = sqlx::query(
        "SELECT name, enabled, headers FROM pack_registry_index WHERE name LIKE 'Clean Canonical %' ORDER BY position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(clean_canonical_rows.len(), 2);
    assert!(clean_canonical_rows
        .iter()
        .all(|row| row.get::<bool, _>("enabled")));
    assert_eq!(
        clean_canonical_rows[0].get::<serde_json::Value, _>("headers"),
        serde_json::json!({"Authorization": "first"})
    );
    assert_eq!(
        clean_canonical_rows[1].get::<serde_json::Value, _>("headers"),
        serde_json::json!({"Authorization": "second"})
    );

    let review_enabled: bool = sqlx::query_scalar(
        "SELECT enabled FROM pack_registry_index WHERE url = 'https://private.example.com/index.json'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!review_enabled);

    let source_url: String =
        sqlx::query_scalar("SELECT source_url FROM pack WHERE ref = 'legacy_query_pack'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(source_url, "https://downloads.example.com/pack.tgz");

    let (source_ref, pack_meta): (String, serde_json::Value) =
        sqlx::query_as("SELECT source_ref, meta FROM pack WHERE ref = 'legacy_query_pack'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(source_ref, "v1.0.0");
    assert_eq!(pack_meta["owner"], "admin");
    assert_eq!(pack_meta["_attune"]["existing"], "preserved");
    assert_eq!(pack_meta["_attune_source_query_redacted"], true);

    let remediated_non_object_meta: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT ref, meta FROM pack WHERE ref IN ('scalar_meta_pack', 'array_meta_pack', 'null_meta_pack') ORDER BY ref",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(remediated_non_object_meta.len(), 3);
    assert_eq!(remediated_non_object_meta[0].0, "array_meta_pack");
    assert_eq!(
        remediated_non_object_meta[0].1["_attune_legacy_meta"],
        serde_json::json!(["legacy", 7])
    );
    assert_eq!(
        remediated_non_object_meta[0].1["_attune_source_query_redacted"],
        true
    );
    assert_eq!(remediated_non_object_meta[1].0, "null_meta_pack");
    assert!(remediated_non_object_meta[1].1["_attune_legacy_meta"].is_null());
    assert_eq!(
        remediated_non_object_meta[1].1["_attune_source_query_redacted"],
        true
    );
    assert_eq!(remediated_non_object_meta[2].0, "scalar_meta_pack");
    assert_eq!(
        remediated_non_object_meta[2].1["_attune_legacy_meta"],
        "legacy-scalar"
    );
    assert_eq!(
        remediated_non_object_meta[2].1["_attune_source_query_redacted"],
        true
    );

    let local_source_url: String =
        sqlx::query_scalar("SELECT source_url FROM pack WHERE ref = 'local_query_pack'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(local_source_url, "/srv/packs/release?candidate/pack.tgz");

    let audit_details: serde_json::Value = sqlx::query_scalar(
        "SELECT details FROM audit_event WHERE category = 'pack' AND event_type = 'pack.installed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit_details["source"],
        "https://downloads.example.com/pack.tgz"
    );
    assert_eq!(audit_details["source_query_redacted"], true);

    let unrelated_audit_details: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT details FROM audit_event WHERE category <> 'pack' OR event_type <> 'pack.installed' ORDER BY event_type, category",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(unrelated_audit_details.len(), 2);
    assert_eq!(
        unrelated_audit_details[0]["source"],
        "https://unrelated.example.com/api-request?token=preserved"
    );
    assert!(unrelated_audit_details[0]
        .get("source_query_redacted")
        .is_none());
    assert_eq!(
        unrelated_audit_details[1]["source"],
        "https://unrelated.example.com/pack-update?token=preserved"
    );
    assert!(unrelated_audit_details[1]
        .get("source_query_redacted")
        .is_none());

    let remaining_queries: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM pack_registry_index WHERE strpos(url, '?') > 0) + (SELECT COUNT(*) FROM pack WHERE source_url ~* '^https?://' AND strpos(source_url, '?') > 0) + (SELECT COUNT(*) FROM audit_event WHERE category = 'pack' AND event_type = 'pack.installed' AND strpos(details ->> 'source', '?') > 0)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining_queries, 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'pack'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "pack table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_action_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'action'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "action table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_trigger_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'trigger'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "trigger table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_sensor_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'sensor'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "sensor table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_rule_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'rule'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "rule table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_execution_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'execution'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "execution table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_event_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'event'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "event table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_enforcement_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'enforcement'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "enforcement table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_inquiry_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'inquiry'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "inquiry table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_identity_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'identity'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "identity table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_key_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'key'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "key table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_notification_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'notification'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "notification table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_runtime_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'runtime'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "runtime table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_worker_table_exists() {
    let pool = create_test_pool().await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT FROM information_schema.tables
            WHERE table_schema = current_schema()
            AND table_name = 'worker'
        ) as exists
        "#,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let exists: bool = row.get("exists");
    assert!(exists, "worker table does not exist");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_columns() {
    let pool = create_test_pool().await.unwrap();

    // Verify all expected columns exist in pack table
    let columns: Vec<String> = sqlx::query(
        r#"
        SELECT column_name
        FROM information_schema.columns
        WHERE table_schema = current_schema() AND table_name = 'pack'
        ORDER BY column_name
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .iter()
    .map(|row| row.get("column_name"))
    .collect();

    let expected_columns = vec![
        "conf_schema",
        "config",
        "created",
        "dependencies",
        "description",
        "id",
        "is_standard",
        "label",
        "meta",
        "ref",
        "runtime_deps",
        "tags",
        "updated",
        "version",
    ];

    for col in &expected_columns {
        assert!(
            columns.contains(&col.to_string()),
            "Column '{}' not found in pack table",
            col
        );
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_action_columns() {
    let pool = create_test_pool().await.unwrap();

    // Verify all expected columns exist in action table
    let columns: Vec<String> = sqlx::query(
        r#"
        SELECT column_name
        FROM information_schema.columns
        WHERE table_schema = current_schema() AND table_name = 'action'
        ORDER BY column_name
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .iter()
    .map(|row| row.get("column_name"))
    .collect();

    let expected_columns = vec![
        "created",
        "description",
        "entrypoint",
        "id",
        "label",
        "out_schema",
        "pack",
        "pack_ref",
        "param_schema",
        "ref",
        "runtime",
        "updated",
    ];

    for col in &expected_columns {
        assert!(
            columns.contains(&col.to_string()),
            "Column '{}' not found in action table",
            col
        );
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_timestamps_auto_populated() {
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();

    // Create a pack and verify timestamps are set
    let pack = PackFixture::new("timestamp_pack")
        .create(&pool)
        .await
        .unwrap();

    // Timestamps should be set to current time
    let now = chrono::Utc::now();
    assert!(pack.created <= now);
    assert!(pack.updated <= now);
    assert!(pack.created <= pack.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_json_column_storage() {
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();

    // Create pack with JSON data
    let pack = PackFixture::new("json_pack")
        .with_description("Pack with JSON data")
        .create(&pool)
        .await
        .unwrap();

    // Verify JSON data is stored and retrieved correctly
    assert!(pack.conf_schema.is_object());
    assert!(pack.config.is_object());
    assert!(pack.meta.is_object());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_array_column_storage() {
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();

    // Create pack with arrays
    let pack = PackFixture::new("array_pack")
        .with_tags(vec![
            "test".to_string(),
            "example".to_string(),
            "demo".to_string(),
        ])
        .create(&pool)
        .await
        .unwrap();

    // Verify arrays are stored correctly
    assert_eq!(pack.tags.len(), 3);
    assert!(pack.tags.contains(&"test".to_string()));
    assert!(pack.tags.contains(&"example".to_string()));
    assert!(pack.tags.contains(&"demo".to_string()));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_unique_constraints() {
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();

    // Create a pack
    PackFixture::new("unique_pack").create(&pool).await.unwrap();

    // Try to create another pack with the same ref - should fail
    let result = PackFixture::new("unique_pack").create(&pool).await;

    assert!(result.is_err(), "Should not allow duplicate pack refs");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_foreign_key_constraints() {
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();

    // Try to create an action with non-existent pack_id - should fail
    let result = sqlx::query(
        r#"
        INSERT INTO action (ref, pack, pack_ref, label, description, entrypoint)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind("test_pack.test_action")
    .bind(99999i64) // Non-existent pack ID
    .bind("test_pack")
    .bind("Test Action")
    .bind("Test action description")
    .bind("main.py")
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "Should not allow action with non-existent pack"
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_enum_types_exist() {
    let pool = create_test_pool().await.unwrap();

    // Check that custom enum types are created
    let enums: Vec<String> = sqlx::query(
        r#"
        SELECT typname
        FROM pg_type
        WHERE typnamespace = (SELECT oid FROM pg_namespace WHERE nspname = current_schema())
        AND typtype = 'e'
        ORDER BY typname
        "#,
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .iter()
    .map(|row| row.get("typname"))
    .collect();

    let expected_enums = vec![
        "artifact_retention_enum",
        "artifact_type_enum",
        "enforcement_condition_enum",
        "enforcement_status_enum",
        "execution_status_enum",
        "inquiry_status_enum",
        "notification_status_enum",
        "owner_type_enum",
        "policy_method_enum",
        "worker_status_enum",
        "worker_type_enum",
    ];

    for enum_type in &expected_enums {
        assert!(
            enums.contains(&enum_type.to_string()),
            "Enum type '{}' not found",
            enum_type
        );
    }
}
