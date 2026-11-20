//! Candidate SQL installations only: no migration-9 ledger entry/freeze yet.
use super::*;
use attune_common::{
    config::CacheAdmissionConfig,
    models::CacheMode,
    repositories::{
        cache::{
            ApplyLiveCacheInput, CacheEntryInput, CacheEntryRepository, CacheGenerationRepository,
            CacheLiveRepository, CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope,
            CacheTransactionMode, CreateCacheGenerationInput, CreateCacheGenerationResult,
            CreateCacheNamespaceInput,
        },
        Create,
    },
};
use serde_json::json;
use sqlx::Connection;

fn generation(ns: i64, key: &str) -> CreateCacheGenerationInput {
    CreateCacheGenerationInput {
        namespace: ns,
        client_refresh_id: key.into(),
        expected_active_generation: None,
        expected_chunk_count: 0,
        expected_count: Some(0),
        expected_bytes: None,
        checksum_algorithm: None,
        checksum: None,
        source_revision: None,
        created_by: None,
        created_by_execution: None,
    }
}
fn id(result: CreateCacheGenerationResult) -> i64 {
    match result {
        CreateCacheGenerationResult::Created(g) | CreateCacheGenerationResult::Existing(g) => g.id,
    }
}
async fn namespace(pool: &PgPool, mode: CacheMode) -> i64 {
    CacheNamespaceRepository::create(
        pool,
        CreateCacheNamespaceInput {
            mode,
            owner: CacheOwnerScope::system(),
            namespace: format!("guard_{}", Uuid::new_v4().simple()),
            policy: CacheNamespacePolicy::default(),
        },
    )
    .await
    .unwrap()
    .id
}
async fn denies(pool: &PgPool, sql: &str) {
    let mut tx = pool.begin().await.unwrap();
    assert!(
        sqlx::query(sql).execute(&mut *tx).await.is_err(),
        "unexpected allowed SQL: {sql}"
    );
    tx.rollback().await.unwrap();
}
async fn absent(f: &Fixture, g: i64) {
    let (rows,leaf):(i64,Option<String>)=sqlx::query_as("SELECT (SELECT count(*) FROM cache_generation WHERE id=$1),to_regclass(format('%I.cache_entry_g_%s',current_schema(),$1))::TEXT").bind(g).fetch_one(&*f.db).await.unwrap();
    assert_eq!(rows, 0);
    assert!(leaf.is_none());
}

pub(super) struct InstalledCache {
    pub cache_owner: String,
    pub legacy: Option<i64>,
}
async fn inventory(f: &Fixture, maintenance: Option<&str>) -> serde_json::Value {
    let mut roles = Vec::new();
    for name in [&f.roles[4], &f.roles[6], &f.roles[2], &f.roles[5]] {
        let oid: i64 = sqlx::query_scalar("SELECT oid::BIGINT FROM pg_roles WHERE rolname=$1")
            .bind(name)
            .fetch_one(&*f.db)
            .await
            .unwrap();
        roles.push(json!({"name":name,"oid":oid}));
    }
    let maintenance = if let Some(name) = maintenance {
        let oid: i64 = sqlx::query_scalar("SELECT oid::BIGINT FROM pg_roles WHERE rolname=$1")
            .bind(name)
            .fetch_one(&*f.db)
            .await
            .unwrap();
        json!({"name":name,"oid":oid})
    } else {
        serde_json::Value::Null
    };
    let installation: Uuid = sqlx::query_scalar(
        "SELECT installation FROM recorded_rollout_installation_identity WHERE id=1",
    )
    .fetch_one(&*f.db)
    .await
    .unwrap();
    let database_oid: i64 =
        sqlx::query_scalar("SELECT oid::BIGINT FROM pg_database WHERE datname=current_database()")
            .fetch_one(&*f.db)
            .await
            .unwrap();
    let schema_oid: i64 =
        sqlx::query_scalar("SELECT oid::BIGINT FROM pg_namespace WHERE nspname=$1")
            .bind(f.db.schema())
            .fetch_one(&*f.db)
            .await
            .unwrap();
    let inventory = json!({"version":1,"database_oid":database_oid,"schema_oid":schema_oid,"schema":f.db.schema(),"installation":installation,"owner":roles[0],"cache_owner":roles[1],"api":roles[2],"migrator":roles[3],"maintenance":maintenance});
    inventory
}
pub(super) async fn profile(f: &Fixture, maintenance: Option<&str>) {
    let inventory = inventory(f, maintenance).await;
    if let Ok(python) = std::env::var("ATTUNE_TEST_CACHE_DBA_PYTHON") {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        use tokio::io::AsyncReadExt;
        let root = tempfile::tempdir().unwrap();
        let credentials = root.path().join("dba.json");
        let input = root.path().join("inventory.json");
        let mut url = url::Url::parse(f.db.database_url()).unwrap();
        url.query_pairs_mut()
            .clear()
            .append_pair("sslmode", "disable");
        url.set_password(Some("dummy-owned-dba-file-only")).unwrap();
        for (path, value) in [
            (&credentials, json!({"database_url":url.as_str()})),
            (&input, inventory.clone()),
        ] {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .unwrap();
            file.write_all(&serde_json::to_vec(&value).unwrap())
                .unwrap();
        }
        let mut child = tokio::process::Command::new(python)
            .args([
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../scripts/provision-rollout-admin.py"
                ),
                "--cache-runtime-profile",
                "--dba-credentials",
                credentials.to_str().unwrap(),
                "--inventory",
                input.to_str().unwrap(),
            ])
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let status = tokio::time::timeout(std::time::Duration::from_secs(40), child.wait()).await;
        if status.is_err() {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
            root.close().unwrap();
            panic!("owned cache provisioning child timed out and was joined");
        }
        let mut out = Vec::new();
        let mut err = Vec::new();
        child
            .stdout
            .take()
            .unwrap()
            .take(65536)
            .read_to_end(&mut out)
            .await
            .unwrap();
        child
            .stderr
            .take()
            .unwrap()
            .take(65536)
            .read_to_end(&mut err)
            .await
            .unwrap();
        root.close().unwrap();
        assert!(
            status.unwrap().unwrap().success(),
            "cache provisioning failed: {}",
            String::from_utf8_lossy(&err)
        );
        assert!(err.is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out).unwrap(),
            json!({"configured":true})
        );
        return;
    }
    let sql = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deployment/rollout-admin/cache-runtime-profile.sql"
    ))
    .unwrap();
    let mut tx = f.db.begin().await.unwrap();
    sqlx::query("CREATE TEMP TABLE cache_runtime_provision_input(document JSONB) ON COMMIT DROP")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO cache_runtime_provision_input VALUES($1)")
        .bind(inventory)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::raw_sql(&sql).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}
pub(super) async fn profile_denials(f: &Fixture) {
    let original = inventory(f, None).await;
    let sql = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deployment/rollout-admin/cache-runtime-profile.sql"
    ))
    .unwrap();
    for (field, value) in [
        ("version", json!(0)),
        ("database_oid", json!(0)),
        ("schema_oid", json!(0)),
        ("installation", json!(Uuid::nil())),
        ("api", original["owner"].clone()),
    ] {
        let mut wrong = original.clone();
        wrong[field] = value;
        let mut tx = f.db.begin().await.unwrap();
        sqlx::query(
            "CREATE TEMP TABLE cache_runtime_provision_input(document JSONB) ON COMMIT DROP",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("INSERT INTO cache_runtime_provision_input VALUES($1)")
            .bind(wrong)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(
            sqlx::raw_sql(&sql).execute(&mut *tx).await.is_err(),
            "invalid inventory accepted: {field}"
        );
        tx.rollback().await.unwrap();
    }
    for grant in [
        format!("GRANT TRUNCATE ON cache_namespace TO {}", f.roles[2]),
        format!(
            "GRANT SELECT ON cache_partition_creation_intent TO {}",
            f.roles[2]
        ),
        format!(
            "GRANT CREATE ON SCHEMA \"{}\" TO {}",
            f.db.schema(),
            f.roles[2]
        ),
        format!(
            "GRANT {} TO {} WITH INHERIT TRUE, SET TRUE",
            f.roles[6], f.roles[2]
        ),
        format!(
            "GRANT {} TO {} WITH INHERIT TRUE, SET TRUE",
            f.roles[4], f.roles[5]
        ),
    ] {
        let mut tx = f.db.begin().await.unwrap();
        sqlx::query(&grant).execute(&mut *tx).await.unwrap();
        sqlx::query(
            "CREATE TEMP TABLE cache_runtime_provision_input(document JSONB) ON COMMIT DROP",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("INSERT INTO cache_runtime_provision_input VALUES($1)")
            .bind(&original)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(
            sqlx::raw_sql(&sql).execute(&mut *tx).await.is_err(),
            "conflicting grants accepted"
        );
        tx.rollback().await.unwrap();
    }
}
pub(super) async fn install(f: &mut Fixture, upgrade: bool) -> InstalledCache {
    let owner = f.roles[4].clone();
    let cache_owner = f.roles[6].clone();
    sqlx::raw_sql(&format!("GRANT USAGE ON SCHEMA \"{}\" TO {cache_owner}; ALTER TABLE cache_entry OWNER TO {cache_owner}",f.db.schema())).execute(&*f.db).await.unwrap();
    let legacy = if upgrade {
        let ns = namespace(f.db.pool(), CacheMode::Snapshot).await;
        let mut input = generation(ns, "legacy");
        input.expected_chunk_count = 1;
        input.expected_count = Some(1);
        let g = id(
            CacheGenerationRepository::create_or_get(f.db.pool(), &input)
                .await
                .unwrap(),
        );
        attune_common::repositories::cache::CacheIngestRepository::insert_chunk(
            f.db.pool(),
            g,
            0,
            "legacy-data",
            &[CacheEntryInput {
                external_id: "preserved".into(),
                value: json!({"pre9":"preserved"}),
                source_updated_at: None,
                source_checksum: None,
            }],
        )
        .await
        .unwrap();
        Some(g)
    } else {
        None
    };
    let sql = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deployment/rollout-admin/cache-partition-creation.candidate.sql"
    ))
    .unwrap();
    // Syntax/object creation rollback first; rollback must leave the old invoker function.
    let migrator = &f.roles[5];
    let password = format!("dummy_{}", Uuid::new_v4().simple());
    let ddl: String =
        sqlx::query_scalar("SELECT format('ALTER ROLE %I PASSWORD %L',$1::TEXT,$2::TEXT)")
            .bind(migrator)
            .bind(&password)
            .fetch_one(&*f.db)
            .await
            .unwrap();
    sqlx::query(&ddl).execute(&*f.db).await.unwrap();
    let mut url = url::Url::parse(f.db.database_url()).unwrap();
    url.set_username(migrator).unwrap();
    url.set_password(Some(&password)).unwrap();
    url.query_pairs_mut().append_pair(
        "options",
        &format!("-crole={owner} -csearch_path={},public", f.db.schema()),
    );
    let mut migration = sqlx::PgConnection::connect(url.as_str()).await.unwrap();
    let identity: (String, String) = sqlx::query_as("SELECT session_user::TEXT,current_user::TEXT")
        .fetch_one(&mut migration)
        .await
        .unwrap();
    assert_eq!(identity, (migrator.clone(), owner.clone()));
    let mut probe = migration.begin().await.unwrap();
    sqlx::raw_sql(&sql).execute(&mut *probe).await.unwrap();
    probe.rollback().await.unwrap();
    assert!(!sqlx::query_scalar::<_,bool>("SELECT prosecdef FROM pg_proc WHERE oid='create_cache_generation_partition(bigint)'::regprocedure").fetch_one(&*f.db).await.unwrap());
    // Explicit disposable candidate installation, NOT a committed migration-9 application.
    let mut install = migration.begin().await.unwrap();
    sqlx::raw_sql(&sql).execute(&mut *install).await.unwrap();
    install.commit().await.unwrap();
    migration.close().await.unwrap();
    profile(f, None).await;
    profile(f, None).await; // Exact repeated application is idempotent.
    if let Some(g) = legacy {
        let value: serde_json::Value = sqlx::query_scalar(
            "SELECT value FROM cache_entry WHERE generation=$1 AND external_id='preserved'",
        )
        .bind(g)
        .fetch_one(&f.pools[1])
        .await
        .unwrap();
        assert_eq!(value, json!({"pre9":"preserved"}));
        let actual:String=sqlx::query_scalar("SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid=to_regclass(format('%I.cache_entry_g_%s',current_schema(),$1))").bind(g).fetch_one(&*f.db).await.unwrap();
        assert_eq!(actual, cache_owner);
    }
    InstalledCache {
        cache_owner,
        legacy,
    }
}
pub(super) async fn candidate(f: &mut Fixture, upgrade: bool) {
    let InstalledCache {
        cache_owner,
        legacy,
    } = install(f, upgrade).await;
    let api = f.roles[2].clone();
    let probe_ns = namespace(&f.pools[1], CacheMode::Snapshot).await;
    super::cache_acl_isolation::probe(f, probe_ns).await;
    validation_probes(f).await;
    let api_pool = f.pools[1].clone();
    let pool = &api_pool;
    for stmt in [
        "CREATE TABLE forbidden(id BIGINT)",
        "ALTER TABLE cache_entry ADD COLUMN forbidden TEXT",
        "DROP TABLE cache_entry",
        "SELECT create_cache_generation_partition(1)",
        "SELECT * FROM cache_partition_creation_intent",
    ] {
        denies(pool, stmt).await;
    }
    denies(&f.pools[2], "SELECT create_cache_generation_partition(1)").await;
    // Separate offline login; a closed old pool is not authorization evidence.
    let mut url = url::Url::parse(f.db.database_url()).unwrap();
    url.set_username(&f.roles[1]).unwrap();
    url.set_password(Some(&f.passwords[0])).unwrap();
    url.query_pairs_mut().append_pair(
        "options",
        &format!("-csearch_path={},public", f.db.schema()),
    );
    let offline = PgPoolOptions::new()
        .max_connections(1)
        .connect(url.as_str())
        .await
        .unwrap();
    f.pools.push(offline.clone());
    denies(&offline, "SELECT create_cache_generation_partition(1)").await;
    {
        let mut lease = offline.acquire().await.unwrap();
        attune_common::repositories::rollout_admin::RolloutAdminRepository::verify(
            &mut lease,
            f.db.schema(),
            std::slice::from_ref(&api),
        )
        .await
        .unwrap();
    }
    offline.close().await;
    // Inherited EXECUTE and SET ROLE cannot substitute the session principal.
    sqlx::query(&format!(
        "GRANT {api} TO {} WITH INHERIT TRUE, SET TRUE",
        f.roles[3]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    denies(&f.pools[2], "SELECT create_cache_generation_partition(1)").await;
    let mut delegated = f.pools[2].begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {api}"))
        .execute(&mut *delegated)
        .await
        .unwrap();
    let error = sqlx::query("SELECT create_cache_generation_partition(1)")
        .execute(&mut *delegated)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("42501")
    );
    delegated.rollback().await.unwrap();
    sqlx::query(&format!("REVOKE {api} FROM {}", f.roles[3]))
        .execute(&*f.db)
        .await
        .unwrap();
    if let Some(g) = legacy {
        let actual:String=sqlx::query_scalar("SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid=to_regclass(format('%I.cache_entry_g_%s',current_schema(),$1))").bind(g).fetch_one(&*f.db).await.unwrap();
        assert_eq!(actual, cache_owner);
        let mut tx = pool.begin().await.unwrap();
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::Attach)
            .await
            .unwrap();
        assert!(sqlx::query("SELECT create_cache_generation_partition($1)")
            .bind(g)
            .execute(&mut *tx)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        maintenance_drop(f, g).await;
    }
    let ns = namespace(pool, CacheMode::Snapshot).await;
    // Repository path, nested savepoint and complete rollback (DDL+intent+usage).
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SAVEPOINT candidate")
        .execute(&mut *tx)
        .await
        .unwrap();
    let g = id(CacheGenerationRepository::create_or_get_with_policy_conn(
        &mut tx,
        &generation(ns, "rollback"),
        &CacheAdmissionConfig::default(),
    )
    .await
    .unwrap());
    sqlx::query("ROLLBACK TO SAVEPOINT candidate")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT candidate")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    absent(f, g).await;
    // Successful insert after rollback-to in the SAME top-level transaction.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SAVEPOINT candidate")
        .execute(&mut *tx)
        .await
        .unwrap();
    let stale = id(CacheGenerationRepository::create_or_get_with_policy_conn(
        &mut tx,
        &generation(ns, "stale"),
        &CacheAdmissionConfig::default(),
    )
    .await
    .unwrap());
    sqlx::query("ROLLBACK TO SAVEPOINT candidate")
        .execute(&mut *tx)
        .await
        .unwrap();
    let keep = id(CacheGenerationRepository::create_or_get_with_policy_conn(
        &mut tx,
        &generation(ns, "keep"),
        &CacheAdmissionConfig::default(),
    )
    .await
    .unwrap());
    sqlx::query("RELEASE SAVEPOINT candidate")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT create_cache_generation_partition($1)")
        .bind(keep)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    absent(f, stale).await;
    // Read-only retry must reject ownership drift as well as parent/bound drift.
    sqlx::query(&format!(
        "ALTER TABLE ONLY cache_entry_g_{keep} OWNER TO {}",
        f.roles[0]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    assert!(
        CacheGenerationRepository::create_or_get(pool, &generation(ns, "keep"))
            .await
            .is_err()
    );
    sqlx::query(&format!(
        "ALTER TABLE ONLY cache_entry_g_{keep} OWNER TO {cache_owner}"
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    assert_eq!(
        id(
            CacheGenerationRepository::create_or_get(pool, &generation(ns, "keep"))
                .await
                .unwrap()
        ),
        keep
    );
    let mut tx = pool.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::Attach)
        .await
        .unwrap();
    assert!(sqlx::query("SELECT create_cache_generation_partition($1)")
        .bind(keep)
        .execute(&mut *tx)
        .await
        .is_err());
    tx.rollback().await.unwrap();
    // Concurrent same refresh is one committed generation; separate refreshes use own namespaces.
    let race_ns = namespace(pool, CacheMode::Snapshot).await;
    let input = generation(race_ns, "race");
    let (a, b) = tokio::join!(
        CacheGenerationRepository::create_or_get(pool, &input),
        CacheGenerationRepository::create_or_get(pool, &input)
    );
    assert_eq!(id(a.unwrap()), id(b.unwrap()));
    let other_ns = namespace(pool, CacheMode::Snapshot).await;
    let input_a = generation(namespace(pool, CacheMode::Snapshot).await, "a");
    let input_b = generation(other_ns, "b");
    let (a, b) = tokio::join!(
        CacheGenerationRepository::create_or_get(pool, &input_a),
        CacheGenerationRepository::create_or_get(pool, &input_b)
    );
    assert_ne!(id(a.unwrap()), id(b.unwrap()));
    // First live write + post-DDL quota failure roll back the namespace/leaf/intent atomically.
    let live = namespace(pool, CacheMode::Live).await;
    let entries = [CacheEntryInput {
        external_id: "one".into(),
        value: json!({"v":1}),
        source_updated_at: None,
        source_checksum: None,
    }];
    let request = || ApplyLiveCacheInput {
        namespace: live,
        request_id: "first",
        request_checksum: "first",
        expected_revision: 0,
        entries: &entries,
        deleted_external_ids: &[],
        source_revision: None,
    };
    let mut tx = pool.begin().await.unwrap();
    CacheLiveRepository::apply_conn(&mut tx, request(), &CacheAdmissionConfig::default())
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT active_generation FROM cache_namespace WHERE id=$1"
        )
        .bind(live)
        .fetch_one(&*f.db)
        .await
        .unwrap(),
        None
    );
    let mut tx = pool.begin().await.unwrap();
    CacheLiveRepository::apply_conn(&mut tx, request(), &CacheAdmissionConfig::default())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    maintenance_drop(f, keep).await;
}

pub(super) async fn maintenance_pool(f: &mut Fixture) -> PgPool {
    // Maintenance has cache ownership, never installation ownership or schema CREATE.
    let role = format!("ws2_cache_maint_{}", Uuid::new_v4().simple());
    let password = format!("dummy_{}", Uuid::new_v4().simple());
    let ddl: String =
        sqlx::query_scalar("SELECT format('CREATE ROLE %I LOGIN PASSWORD %L',$1::TEXT,$2::TEXT)")
            .bind(&role)
            .bind(&password)
            .fetch_one(&*f.db)
            .await
            .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_roles WHERE rolname=$1")
            .bind(&role)
            .fetch_one(&*f.db)
            .await
            .unwrap(),
        0
    );
    sqlx::query(&ddl).execute(&*f.db).await.unwrap();
    f.roles.push(role.clone());
    std::fs::write(&f.manifest, serde_json::to_vec(&f.roles).unwrap()).unwrap();
    profile(f, Some(&role)).await;
    let mut url = url::Url::parse(f.db.database_url()).unwrap();
    url.set_username(&role).unwrap();
    url.set_password(Some(&password)).unwrap();
    url.query_pairs_mut().append_pair(
        "options",
        &format!("-csearch_path={},public", f.db.schema()),
    );
    let maintenance = PgPoolOptions::new()
        .max_connections(1)
        .connect(url.as_str())
        .await
        .unwrap();
    f.pools.push(maintenance.clone());
    maintenance
}
pub(super) async fn maintenance_drop(f: &mut Fixture, generation: i64) {
    let maintenance = maintenance_pool(f).await;
    CacheGenerationRepository::fail(&f.pools[1], generation, "candidate cleanup")
        .await
        .unwrap();
    let dropped: String =
        sqlx::query_scalar("SELECT outcome FROM drop_cleanup_cache_generation($1,0)")
            .bind(generation)
            .fetch_one(&maintenance)
            .await
            .unwrap();
    assert_eq!(dropped, "dropped");
    sqlx::query("ANALYZE cache_entry (generation,external_id,id,size_bytes)")
        .execute(&maintenance)
        .await
        .unwrap();
    maintenance.close().await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM cache_partition_creation_intent WHERE generation=$1"
        )
        .bind(generation)
        .fetch_one(&*f.db)
        .await
        .unwrap(),
        0
    );
}

async fn run_candidate(upgrade: bool) {
    use futures::FutureExt;
    let mut fixture = Fixture::new(false).await;
    let result = std::panic::AssertUnwindSafe(async {
        super::admin_process::provision(&mut fixture).await;
        super::admin_process::provisioning_assets(&mut fixture).await;
        candidate(&mut fixture, upgrade).await;
    })
    .catch_unwind()
    .await;
    fixture.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
#[tokio::test]
async fn candidate_fresh_cache_role_boundary() {
    run_candidate(false).await;
}
#[tokio::test]
async fn candidate_pre9_cache_role_boundary() {
    run_candidate(true).await;
}

async fn reject_creation(f: &Fixture, namespace: i64, label: &str) {
    let error =
        CacheGenerationRepository::create_or_get(&f.pools[1], &generation(namespace, label))
            .await
            .unwrap_err();
    match error {
        attune_common::Error::Database(e) => assert_eq!(
            e.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("42501")
        ),
        other => panic!("wrong guard rejection: {other:?}"),
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM cache_generation WHERE namespace=$1")
            .bind(namespace)
            .fetch_one(&*f.db)
            .await
            .unwrap(),
        0
    );
}
async fn validation_probes(f: &Fixture) {
    let cache_owner = &f.roles[6];
    let worker = &f.roles[3];
    let wrong_owner = &f.roles[0];
    let ns = namespace(&f.pools[1], CacheMode::Snapshot).await;
    let before: (i64, i64, Option<String>) = sqlx::query_as(
        "SELECT oid::BIGINT,nspowner::BIGINT,nspacl::TEXT FROM pg_namespace WHERE nspname=$1",
    )
    .bind(f.db.schema())
    .fetch_one(&*f.db)
    .await
    .unwrap();
    let creator:(i64,i64,Option<String>)=sqlx::query_as("SELECT oid::BIGINT,proowner::BIGINT,proacl::TEXT FROM pg_proc WHERE oid='create_cache_generation_partition(bigint)'::regprocedure").fetch_one(&*f.db).await.unwrap();
    sqlx::query(&format!(
        "GRANT CREATE ON SCHEMA \"{}\" TO {cache_owner}",
        f.db.schema()
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    reject_creation(f, ns, "bad-existing-schema-acl").await;
    sqlx::query(&format!(
        "REVOKE CREATE ON SCHEMA \"{}\" FROM {cache_owner}",
        f.db.schema()
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    sqlx::query(&format!(
        "GRANT EXECUTE ON FUNCTION create_cache_generation_partition(BIGINT) TO {worker}"
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    reject_creation(f, ns, "excess-execute").await;
    sqlx::query(&format!(
        "REVOKE EXECUTE ON FUNCTION create_cache_generation_partition(BIGINT) FROM {worker}"
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    sqlx::query(&format!(
        "ALTER TABLE ONLY cache_entry OWNER TO {wrong_owner}"
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    reject_creation(f, ns, "wrong-parent-owner").await;
    sqlx::query(&format!(
        "ALTER TABLE ONLY cache_entry OWNER TO {cache_owner}"
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    let after: (i64, i64, Option<String>) = sqlx::query_as(
        "SELECT oid::BIGINT,nspowner::BIGINT,nspacl::TEXT FROM pg_namespace WHERE nspname=$1",
    )
    .bind(f.db.schema())
    .fetch_one(&*f.db)
    .await
    .unwrap();
    assert_eq!(before, after);
    let after_creator:(i64,i64,Option<String>)=sqlx::query_as("SELECT oid::BIGINT,proowner::BIGINT,proacl::TEXT FROM pg_proc WHERE oid='create_cache_generation_partition(bigint)'::regprocedure").fetch_one(&*f.db).await.unwrap();
    assert_eq!(creator, after_creator);
}
