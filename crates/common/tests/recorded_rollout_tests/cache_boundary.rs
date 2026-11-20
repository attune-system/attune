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

pub(super) async fn candidate(f: &mut Fixture, upgrade: bool) {
    let owner = f.roles[4].clone();
    let cache_owner = f.roles[6].clone();
    let api = f.roles[2].clone();
    sqlx::raw_sql(&format!("GRANT {cache_owner} TO {owner} WITH INHERIT TRUE, SET TRUE; GRANT USAGE ON SCHEMA \"{}\" TO {cache_owner}; ALTER TABLE cache_entry OWNER TO {cache_owner}",f.db.schema())).execute(&*f.db).await.unwrap();
    let legacy = if upgrade {
        let ns = namespace(f.db.pool(), CacheMode::Snapshot).await;
        Some(id(CacheGenerationRepository::create_or_get(
            f.db.pool(),
            &generation(ns, "legacy"),
        )
        .await
        .unwrap()))
    } else {
        None
    };
    let sql = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deployment/rollout-admin/cache-partition-creation.candidate.sql"
    ))
    .unwrap();
    // Syntax/object creation rollback first; rollback must leave the old invoker function.
    let mut probe = f.db.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {owner}"))
        .execute(&mut *probe)
        .await
        .unwrap();
    sqlx::raw_sql(&sql).execute(&mut *probe).await.unwrap();
    probe.rollback().await.unwrap();
    assert!(!sqlx::query_scalar::<_,bool>("SELECT prosecdef FROM pg_proc WHERE oid='create_cache_generation_partition(bigint)'::regprocedure").fetch_one(&*f.db).await.unwrap());
    // Explicit disposable candidate installation, NOT a committed migration-9 application.
    let mut install = f.db.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {owner}"))
        .execute(&mut *install)
        .await
        .unwrap();
    sqlx::raw_sql(&sql).execute(&mut *install).await.unwrap();
    install.commit().await.unwrap();
    let tables="cache_namespace,cache_generation,cache_entry,cache_ingest_chunk,cache_generation_clone,cache_generation_entry_usage,cache_owner_physical_byte_usage,cache_deployment_physical_byte_usage,cache_entry_statistics_state,cache_live_request";
    sqlx::raw_sql(&format!("GRANT SELECT,INSERT,UPDATE,DELETE ON {tables} TO {api}; GRANT SELECT ON runtime_retention_config TO {api}; GRANT USAGE,SELECT ON SEQUENCE cache_namespace_id_seq,cache_generation_id_seq,cache_entry_id_seq TO {api}; GRANT EXECUTE ON FUNCTION create_cache_generation_partition(BIGINT) TO {api}")).execute(&*f.db).await.unwrap();
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
        maintenance_drop(f, tables, &cache_owner, g).await;
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
    maintenance_drop(f, tables, &cache_owner, keep).await;
}

async fn maintenance_drop(f: &mut Fixture, tables: &str, cache_owner: &str, generation: i64) {
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
    sqlx::query(&ddl).execute(&*f.db).await.unwrap();
    f.roles.push(role.clone());
    std::fs::write(&f.manifest, serde_json::to_vec(&f.roles).unwrap()).unwrap();
    sqlx::raw_sql(&format!("GRANT {cache_owner} TO {role} WITH INHERIT TRUE, SET FALSE; GRANT USAGE ON SCHEMA \"{}\" TO {role}; GRANT SELECT,UPDATE,DELETE ON {tables} TO {role}; GRANT SELECT ON file_evidence_pin,runtime_native_cache_frontier,workflow_cache_iteration,workflow_execution TO {role}",f.db.schema())).execute(&*f.db).await.unwrap();
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
