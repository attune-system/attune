//! Runtime-source SQLx driver for the owned fresh-install verifier.
//! Uses the real Migrator, locking, transactional application and checksum checks.

use std::path::Path;

use attune_common::{
    config::CacheRetentionConfig,
    repositories::cache::{
        CacheEntryInput, CacheGenerationCleanupOutcome, CacheGenerationRepository,
        CacheIngestRepository, CacheNamespaceRepository, CacheOwnerScope,
        CacheStatisticsRefreshOutcome, CacheStorageRepository, CreateCacheGenerationInput,
        CreateCacheGenerationResult, CreateCacheNamespaceInput,
    },
    Error,
};
use serde_json::json;
use sqlx::{migrate::Migrator, postgres::PgPoolOptions, Connection, PgConnection, PgPool};

async fn cache_probe(
    pool: &PgPool,
    reclaim: Option<i64>,
    statistics: bool,
    statistics_denied: bool,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let roles: (String, String, bool, bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        "SELECT session_user::text,current_user::text,rolsuper,rolcreatedb,rolcreaterole,\
         pg_has_role(current_user,c.relowner,'USAGE'),has_schema_privilege(current_user,c.relnamespace,'CREATE'),rolbypassrls,pg_has_role(current_user,c.relowner,'SET') \
         FROM pg_roles JOIN pg_class c ON c.oid='cache_entry'::regclass WHERE rolname=current_user",
    ).fetch_one(pool).await?;
    if roles.0 != roles.1
        || roles.2
        || roles.3
        || roles.4
        || roles.7
        || (!statistics_denied && (!roles.5 || !roles.6 || !roles.8))
    {
        return Err("cache probe requires an unchanged non-superuser service login with effective owner membership and schema CREATE".into());
    }
    let roles = json!({"login": roles.0, "effective": roles.1, "superuser": roles.2,
        "createdb": roles.3, "createrole": roles.4, "inherits_parent_owner": roles.5, "schema_create": roles.6, "bypassrls": roles.7, "can_set_parent_owner": roles.8});
    if statistics_denied {
        match CacheStorageRepository::refresh_statistics(pool, &CacheRetentionConfig::default())
            .await
        {
            Err(Error::InvalidState(reason))
                if reason == "cache statistics maintenance requires effective parent ownership" =>
            {
                return Ok(json!({"roles": roles, "outcome": "rejected", "reason": reason}));
            }
            other => {
                return Err(format!(
                    "DML-only statistics role was not rejected by ownership guard: {other:?}"
                )
                .into())
            }
        }
    }
    if statistics {
        let config = CacheRetentionConfig::default();
        let before = CacheStorageRepository::observe(pool, &config).await?;
        if CacheStorageRepository::refresh_statistics(pool, &config).await?
            != CacheStatisticsRefreshOutcome::Applied
        {
            return Err("cache statistics refresh did not apply".into());
        }
        let after = CacheStorageRepository::observe(pool, &config).await?;
        if CacheStorageRepository::refresh_statistics(pool, &config).await?
            != CacheStatisticsRefreshOutcome::NotDue
        {
            return Err("cache statistics refresh rerun was not a no-op".into());
        }
        return Ok(
            json!({"roles": roles, "before_pending": before.statistics_pending,
            "after_pending": after.statistics_pending, "registered_partitions": after.registered_partitions,
            "partitions_created": after.partitions_created, "partitions_dropped": after.partitions_dropped,
            "cleanup_backlog": after.cleanup_backlog, "last_analyzed_at": after.last_analyzed_at,
            "refresh": "applied", "rerun": "not_due"}),
        );
    }
    if let Some(id) = reclaim {
        let result = CacheGenerationRepository::drop_if_cleanup_eligible(
            pool,
            id,
            &CacheRetentionConfig::default(),
        )
        .await?;
        let outcome = match result {
            CacheGenerationCleanupOutcome::Dropped { records, bytes } => {
                json!({"outcome": "dropped", "records": records, "bytes": bytes})
            }
            other => return Err(format!("unexpected cache reclamation result: {other:?}").into()),
        };
        return Ok(json!({"roles": roles, "reclaimed": outcome}));
    }
    let namespace = CacheNamespaceRepository::create_api(
        pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::system(),
            namespace: "native-install".into(),
            policy: Default::default(),
        },
    )
    .await?;
    let mut generations = Vec::new();
    for (refresh, count) in [("populated", 2), ("empty", 0)] {
        let input = CreateCacheGenerationInput {
            namespace: namespace.id,
            client_refresh_id: refresh.into(),
            expected_active_generation: None,
            expected_chunk_count: if count == 0 { 0 } else { 1 },
            expected_count: Some(count),
            expected_bytes: None,
            checksum_algorithm: None,
            checksum: None,
            source_revision: None,
            created_by: None,
            created_by_execution: None,
        };
        let generation = match CacheGenerationRepository::create_or_get(pool, &input).await? {
            CreateCacheGenerationResult::Created(generation) => generation,
            _ => return Err("cache creation unexpectedly reused a generation".into()),
        };
        match CacheGenerationRepository::create_or_get(pool, &input).await? {
            CreateCacheGenerationResult::Existing(existing) if existing.id == generation.id => (),
            _ => return Err("cache creation retry did not reuse its storage".into()),
        }
        if count != 0 {
            let entries = ["a", "b"].map(|external_id| CacheEntryInput {
                external_id: external_id.into(),
                value: json!({"install": external_id}),
                source_updated_at: None,
                source_checksum: None,
            });
            CacheIngestRepository::insert_chunk(pool, generation.id, 0, "install-chunk", &entries)
                .await?;
        }
        CacheGenerationRepository::fail(pool, generation.id, "owned install reclamation fixture")
            .await?;
        generations.push(json!({"id": generation.id, "records": count}));
    }
    Ok(json!({"roles": roles, "namespace": namespace.id, "generations": generations}))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .first()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!(
            "Usage: verify_native_migrations --source <migration-directory>\n\
             Cache repository probes: --cache-stage or --cache-reclaim <generation-id>\n\
             Planner statistics probe: --cache-statistics\n\
             DML-only rejection probe: --cache-statistics-denied\n\
             Cache probes use distinct non-superuser service logins inheriting native_cache_owner, without SET ROLE.\n\
             Set ATTUNE_VERIFY_DATABASE_URL to an owned database.\n\
             Example: verify_native_migrations --source /tmp/opencode/native-install/migrations\n\
             This driver does not create or reset databases."
        );
        return Ok(());
    }
    let stage = args.as_slice() == ["--cache-stage"];
    let statistics = args.as_slice() == ["--cache-statistics"];
    let statistics_denied = args.as_slice() == ["--cache-statistics-denied"];
    let reclaim = if args.len() == 2 && args[0] == "--cache-reclaim" {
        let id: i64 = args[1].parse()?;
        if id <= 0 {
            return Err("generation ID must be positive".into());
        }
        Some(id)
    } else {
        None
    };
    if stage || statistics || statistics_denied || reclaim.is_some() {
        let url = std::env::var("ATTUNE_VERIFY_DATABASE_URL")?;
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("SET search_path TO attune, public")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await?;
        let result = cache_probe(&pool, reclaim, statistics, statistics_denied).await;
        pool.close().await;
        println!("{}", result?);
        return Ok(());
    }
    if args.len() != 2 || args[0] != "--source" {
        return Err("usage: verify_native_migrations --source <migration-directory>; set ATTUNE_VERIFY_DATABASE_URL to an owned database".into());
    }
    let url = std::env::var("ATTUNE_VERIFY_DATABASE_URL")?;
    let migrator = Migrator::new(Path::new(&args[1])).await?;
    let mut connection = PgConnection::connect(&url).await?;
    // The harness bootstraps the schema as its non-superuser owner. No global
    // Config or environment mutation is needed, and history stays in attune.
    let result = async {
        sqlx::query("SET search_path TO attune, public")
            .execute(&mut connection)
            .await?;
        migrator.run(&mut connection).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    connection.close().await?;
    result
}
