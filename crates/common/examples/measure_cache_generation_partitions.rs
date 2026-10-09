//! Owned full-lifecycle cache repository comparison, driven by the Python runner.
//! Direct SQL is confined to fixture diagnostics. Application writes use repositories.

use anyhow::{ensure, Context, Result};
use attune_common::{
    config::{CacheAdmissionConfig, CacheRetentionConfig, Config},
    models::{
        CacheGeneration, ExecutionStatus, WorkflowCacheIterationState, CACHE_ENTRY_SELECT_COLUMNS,
    },
    repositories::{
        cache::{
            CacheEntryInput, CacheEntryRepository, CacheGenerationCleanupOutcome,
            CacheGenerationRepository, CacheIngestRepository, CacheNamespacePolicy,
            CacheNamespaceRepository, CacheOwnerScope, CacheStatisticsRefreshOutcome,
            CacheStorageRepository, CacheTransactionMode, CreateCacheGenerationInput,
            CreateCacheGenerationResult, CreateCacheNamespaceInput,
        },
        execution::{CreateExecutionInput, ExecutionRepository},
        pack::{CreatePackInput, PackRepository},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
        workflow_cache_iteration::{
            CreateWorkflowCacheIterationInput, WorkflowCacheIterationRepository,
        },
        Create,
    },
    test_database::TestDatabase,
};
use chrono::{Duration, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool};
use std::{
    future::Future,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::sync::{oneshot, Barrier, Mutex};

const NAMESPACES: usize = 8;
const LARGE_NAMESPACES: usize = 4;
const RETAINED: usize = 4;
const CHUNK: usize = 1000;
const SMALL: usize = 1000;
const CLIENTS: usize = 4;
const PIN_ROOTS: usize = 2;
const PIN_ROUNDS: usize = 200;
const READ_ROUNDS: usize = 200;
const CAP: usize = 128;

#[derive(Clone, Serialize)]
struct Call {
    operation: String,
    phase: String,
    ms: f64,
    outcome: String,
    during_cleanup: bool,
}

#[derive(Clone, Default)]
struct Recorder {
    calls: Arc<Mutex<Vec<Call>>>,
    cleaning: Arc<AtomicUsize>,
    cleanup_epoch: Arc<AtomicUsize>,
}

impl Recorder {
    async fn timed<T, E: std::fmt::Display>(
        &self,
        phase: &str,
        operation: &str,
        future: impl Future<Output = std::result::Result<T, E>>,
    ) -> std::result::Result<T, E> {
        let overlap = self.cleaning.load(Ordering::SeqCst) != 0;
        let epoch = self.cleanup_epoch.load(Ordering::SeqCst);
        let start = Instant::now();
        let result = future.await;
        self.calls.lock().await.push(Call {
            operation: operation.to_owned(),
            phase: phase.to_owned(),
            ms: start.elapsed().as_secs_f64() * 1000.0,
            outcome: match &result {
                Ok(_) => "ok".to_owned(),
                Err(e) => format!("error: {e}"),
            },
            during_cleanup: overlap
                || self.cleaning.load(Ordering::SeqCst) != 0
                || epoch != self.cleanup_epoch.load(Ordering::SeqCst),
        });
        result
    }

    async fn cleanup(
        &self,
        pool: &PgPool,
        id: i64,
        phase: &str,
    ) -> Result<CacheGenerationCleanupOutcome> {
        let start = Instant::now();
        self.cleaning.fetch_add(1, Ordering::SeqCst);
        self.cleanup_epoch.fetch_add(1, Ordering::SeqCst);
        let result =
            CacheGenerationRepository::drop_if_cleanup_eligible(pool, id, &retention()).await;
        self.cleaning.fetch_sub(1, Ordering::SeqCst);
        self.cleanup_epoch.fetch_add(1, Ordering::SeqCst);
        self.calls.lock().await.push(Call {
            operation: "cleanup".to_owned(),
            phase: phase.to_owned(),
            ms: start.elapsed().as_secs_f64() * 1000.0,
            outcome: match &result {
                Ok(outcome) => format!("{outcome:?}"),
                Err(e) => format!("error: {e}"),
            },
            during_cleanup: true,
        });
        Ok(result?)
    }

    async fn reclaim(&self, pool: &PgPool, id: i64, phase: &str) -> Result<Value> {
        // Explicit maintenance cycles, not a retrying SQL wrapper. Each attempt,
        // including deferrals and partially committed heap batches, is retained.
        let start = Instant::now();
        let mut outcomes = Vec::new();
        for cycle in 0..128 {
            let outcome = self.cleanup(pool, id, phase).await?;
            outcomes.push(format!("{outcome:?}"));
            match outcome {
                CacheGenerationCleanupOutcome::Dropped { records, bytes } => {
                    return Ok(json!({
                    "ms":start.elapsed().as_secs_f64()*1000.0,"records":records,"bytes":bytes,
                    "cycles":cycle+1,"outcomes":outcomes}))
                }
                CacheGenerationCleanupOutcome::DeferredBusy
                | CacheGenerationCleanupOutcome::DeferredDeadline => {}
                other => anyhow::bail!("reclamation returned {other:?}"),
            }
        }
        anyhow::bail!("128 declared maintenance cycles exhausted")
    }
}

fn retention() -> CacheRetentionConfig {
    CacheRetentionConfig {
        min_traversal_window_seconds: 0,
        ..Default::default()
    }
}

fn admission() -> CacheAdmissionConfig {
    CacheAdmissionConfig {
        max_entry_partitions: CAP as i64,
        max_physical_bytes: 256 * 1024 * 1024 * 1024,
        max_physical_bytes_per_owner: 256 * 1024 * 1024 * 1024,
        max_unpublished_generations_per_owner: 128,
        ..Default::default()
    }
}

fn input(
    namespace: i64,
    refresh: &str,
    count: usize,
    active: Option<i64>,
) -> CreateCacheGenerationInput {
    CreateCacheGenerationInput {
        namespace,
        client_refresh_id: refresh.to_owned(),
        expected_active_generation: active,
        expected_chunk_count: count.div_ceil(CHUNK) as i32,
        expected_count: Some(count as i64),
        expected_bytes: None,
        checksum_algorithm: None,
        checksum: None,
        source_revision: None,
        created_by: None,
        created_by_execution: None,
    }
}

fn payload(index: usize) -> Value {
    // Unique hash blocks prevent the TOAST cohort from compressing to one repeated block.
    let bytes = if index.is_multiple_of(32) { 4096 } else { 128 };
    let mut text = String::with_capacity(bytes);
    for block in 0..bytes / 64 {
        text.push_str(&digest_hex(
            format!("cache-measure-v1:{index}:{block}").as_bytes(),
        ));
    }
    json!({"record": index, "body": text, "labels": ["owned", "cache-v1"]})
}

fn digest_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn entries(start: usize, count: usize) -> Vec<CacheEntryInput> {
    (start..start + count)
        .map(|index| CacheEntryInput {
            external_id: format!("record-{index:010}"),
            value: payload(index),
            source_updated_at: None,
            source_checksum: None,
        })
        .collect()
}

async fn create(
    pool: &PgPool,
    recorder: &Recorder,
    phase: &str,
    input: &CreateCacheGenerationInput,
) -> Result<CacheGeneration> {
    let result = recorder
        .timed(
            phase,
            "create",
            CacheGenerationRepository::create_or_get_with_policy(pool, input, &admission()),
        )
        .await?;
    match result {
        CreateCacheGenerationResult::Created(generation) => Ok(generation),
        CreateCacheGenerationResult::Existing(_) => anyhow::bail!("unexpected refresh reuse"),
    }
}

async fn upload(
    pool: &PgPool,
    recorder: &Recorder,
    phase: &str,
    generation: &CacheGeneration,
    count: usize,
) -> Result<()> {
    for (chunk, start) in (0..count).step_by(CHUNK).enumerate() {
        let values = entries(start, CHUNK.min(count - start));
        let checksum = digest_hex(&serde_json::to_vec(
            &values.iter().map(|e| &e.value).collect::<Vec<_>>(),
        )?);
        recorder
            .timed(
                phase,
                "ingest",
                CacheIngestRepository::insert_chunk_with_policy(
                    pool,
                    generation.id,
                    chunk as i32,
                    &checksum,
                    &values,
                    &admission(),
                ),
            )
            .await?;
        if chunk == 0 {
            recorder
                .timed(
                    phase,
                    "replay",
                    CacheIngestRepository::insert_chunk_with_policy(
                        pool,
                        generation.id,
                        chunk as i32,
                        &checksum,
                        &values,
                        &admission(),
                    ),
                )
                .await?;
        }
    }
    Ok(())
}

async fn publish(
    pool: &PgPool,
    recorder: &Recorder,
    phase: &str,
    generation: &CacheGeneration,
    prior: Option<i64>,
) -> Result<()> {
    let sealed = recorder
        .timed(
            phase,
            "seal",
            CacheGenerationRepository::seal(pool, generation.id),
        )
        .await?;
    ensure!(
        sealed.record_count == generation.expected_count.unwrap_or_default(),
        "seal count mismatch"
    );
    recorder
        .timed(
            phase,
            "promote",
            CacheGenerationRepository::promote(
                pool,
                generation.namespace,
                generation.id,
                prior,
                Utc::now() - Duration::seconds(1),
            ),
        )
        .await?;
    Ok(())
}

struct PinTarget {
    namespace: i64,
    generation: i64,
}

async fn pin(
    pool: &PgPool,
    recorder: &Recorder,
    phase: &str,
    workflow: i64,
    target: PinTarget,
    name: &str,
    terminal: bool,
) -> Result<i64> {
    recorder
        .timed(phase, "iteration_pin", async {
            let mut tx = pool.begin().await?;
            CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
                .await?;
            let iteration = WorkflowCacheIterationRepository::create(
                &mut *tx,
                CreateWorkflowCacheIterationInput {
                    workflow_execution: workflow,
                    task_name: name.to_owned(),
                    namespace: target.namespace,
                    generation: target.generation,
                    page_size: 1000,
                    batch_size: 1000,
                    concurrency: 4,
                },
            )
            .await?;
            if terminal {
                WorkflowCacheIterationRepository::mark_terminal(
                    &mut *tx,
                    iteration.id,
                    WorkflowCacheIterationState::Completed,
                    None,
                )
                .await?;
            }
            tx.commit().await?;
            Ok::<_, anyhow::Error>(iteration.id)
        })
        .await
}

async fn create_workflow_root(pool: &PgPool, label: &str) -> Result<i64> {
    let pack_ref = format!("cache_measure_{label}");
    let action_ref = format!("{pack_ref}.iterate");
    let pack = PackRepository::create(
        pool,
        CreatePackInput {
            r#ref: pack_ref,
            label: "Owned cache measurement".to_owned(),
            description: None,
            version: "1.0.0".to_owned(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: vec![],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
            installers: json!({}),
        },
    )
    .await?;
    let definition = WorkflowDefinitionRepository::create(
        pool,
        CreateWorkflowDefinitionInput {
            r#ref: action_ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref,
            label: "Owned measurement".to_owned(),
            description: None,
            version: "1.0.0".to_owned(),
            param_schema: None,
            out_schema: None,
            definition: json!({}),
            tags: vec![],
        },
    )
    .await?;
    let execution = ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action_ref,
            status: ExecutionStatus::Running,
            ..Default::default()
        },
    )
    .await?;
    Ok(WorkflowExecutionRepository::create(
        pool,
        CreateWorkflowExecutionInput {
            execution: execution.id,
            workflow_def: definition.id,
            task_graph: json!({}),
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await?
    .id)
}

fn pin_generation_order(root: usize, pairs: [(i64, i64); 2]) -> [(i64, i64); 2] {
    if root == 0 {
        pairs
    } else {
        [pairs[1], pairs[0]]
    }
}

async fn pin_pair(
    pool: &PgPool,
    recorder: &Recorder,
    workflow: i64,
    root: usize,
    round: usize,
    pairs: [(i64, i64); 2],
) -> Result<()> {
    let operation = format!("pin_pair_mutation_root_{root}");
    let coordination = format!("pin_mutation_coordination_root_{root}");
    recorder
        .timed("burst", &operation, async {
            let mut tx = pool.begin().await?;
            recorder
                .timed(
                    "burst",
                    &coordination,
                    CacheEntryRepository::protect_transaction(
                        &mut tx,
                        CacheTransactionMode::PinMutation,
                    ),
                )
                .await?;
            WorkflowExecutionRepository::find_by_id_for_update(&mut *tx, workflow)
                .await?
                .context("pin root disappeared")?;
            for (namespace, generation) in pin_generation_order(root, pairs) {
                CacheGenerationRepository::find_by_id_for_share(&mut tx, generation)
                    .await?
                    .context("protected pin generation disappeared")?;
                let input = CreateWorkflowCacheIterationInput {
                    workflow_execution: workflow,
                    task_name: format!("pair-{round}-{generation}"),
                    namespace,
                    generation,
                    page_size: 1000,
                    batch_size: 1000,
                    concurrency: 4,
                };
                let iteration = WorkflowCacheIterationRepository::create_or_find_for_update(
                    &mut tx,
                    input.clone(),
                )
                .await?;
                let replay =
                    WorkflowCacheIterationRepository::create_or_find_for_update(&mut tx, input)
                        .await?;
                ensure!(
                    replay.id == iteration.id,
                    "pin replay created another iteration"
                );
                WorkflowCacheIterationRepository::mark_terminal(
                    &mut *tx,
                    iteration.id,
                    WorkflowCacheIterationState::Completed,
                    None,
                )
                .await?;
            }
            tx.commit().await?;
            Ok::<_, anyhow::Error>(())
        })
        .await
}

/// Owning fixture diagnostics. These are never substituted for application repository operations.
async fn diagnostics(pool: &PgPool) -> Result<Value> {
    Ok(sqlx::query_scalar::<_, Value>(r#"
        SELECT jsonb_build_object(
          'entries', (SELECT count(*) FROM cache_entry),
          'entry_bytes', (SELECT coalesce(sum(size_bytes),0)::bigint FROM cache_entry),
          'deployment_bytes', (SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id=1),
          'owner_bytes', (SELECT coalesce(sum(physical_bytes),0)::bigint FROM cache_owner_physical_byte_usage),
          'generations', (SELECT count(*) FROM cache_generation),
          'usage_generations', (SELECT count(*) FROM cache_generation_entry_usage),
          'usage_bytes', (SELECT coalesce(sum(physical_bytes),0)::bigint FROM cache_generation_entry_usage),
          'usage_records', (SELECT coalesce(sum(record_count),0)::bigint FROM cache_generation_entry_usage),
          'retained_iterations', (SELECT coalesce(sum(retained_iterations),0)::bigint FROM cache_generation_entry_usage),
          'partitions', (SELECT count(*) FROM pg_inherits WHERE inhparent='cache_entry'::regclass),
          'chunks', (SELECT count(*) FROM cache_ingest_chunk),
          'iterations', (SELECT count(*) FROM workflow_cache_iteration),
          'deadlocks', (SELECT deadlocks FROM pg_stat_database WHERE datname=current_database()),
          'relations', (SELECT jsonb_agg(jsonb_build_object(
             'name',c.relname,'heap_bytes',pg_relation_size(c.oid),
             'index_bytes',pg_indexes_size(c.oid),'total_bytes',pg_total_relation_size(c.oid),
             'toast_bytes',CASE WHEN c.reltoastrelid=0 THEN 0 ELSE pg_total_relation_size(c.reltoastrelid) END,
             'live',s.n_live_tup,'dead',s.n_dead_tup,'vacuum_count',s.vacuum_count,
             'autovacuum_count',s.autovacuum_count,'analyze_count',s.analyze_count,
             'autoanalyze_count',s.autoanalyze_count) ORDER BY c.relname)
           FROM pg_class c LEFT JOIN pg_stat_all_tables s ON s.relid=c.oid
           WHERE c.relnamespace=current_schema()::regnamespace AND c.relkind IN ('r','p') AND
              (c.relname='cache_entry' OR c.relname LIKE 'cache_entry_g_%'
               OR c.relname IN ('cache_ingest_chunk','workflow_cache_iteration','cache_generation'))))
    "#).fetch_one(pool).await?)
}

fn accounting_matches(value: &Value) -> bool {
    value["entry_bytes"] == value["deployment_bytes"]
        && value["entry_bytes"] == value["owner_bytes"]
        && value["entry_bytes"] == value["usage_bytes"]
        && value["entries"] == value["usage_records"]
        && value["generations"] == value["usage_generations"]
        && value["iterations"] == value["retained_iterations"]
}

async fn memory(connection: &mut PgConnection) -> Result<Value> {
    Ok(sqlx::query_scalar::<_, Value>(
        "SELECT jsonb_build_object('pid',pg_backend_pid(),'total_bytes',sum(total_bytes),
         'used_bytes',sum(used_bytes),'cached_plan_bytes',sum(total_bytes) FILTER (WHERE name LIKE '%CachedPlan%'),
         'contexts',jsonb_agg(jsonb_build_object('name',name,'total_bytes',total_bytes,'used_bytes',used_bytes)))
         FROM pg_backend_memory_contexts").fetch_one(connection).await?)
}

async fn isolated_cleanup(
    pool: &PgPool,
    recorder: &Recorder,
    namespace: i64,
    count: usize,
    evidence: &mut Value,
) -> Result<()> {
    let generation = create(
        pool,
        recorder,
        "isolated_load",
        &input(namespace, "isolated", count, None),
    )
    .await?;
    upload(pool, recorder, "isolated_load", &generation, count).await?;
    recorder
        .timed(
            "isolated_load",
            "fail",
            CacheGenerationRepository::fail(pool, generation.id, "owned measurement"),
        )
        .await?;
    evidence["isolated_before"] = diagnostics(pool).await?;
    let mut connection = pool.acquire().await?;
    let lsn: String = sqlx::query_scalar("SELECT pg_current_wal_insert_lsn()::text")
        .fetch_one(&mut *connection)
        .await?;
    let start = Instant::now();
    let outcome = recorder
        .reclaim(pool, generation.id, "isolated_cleanup")
        .await?;
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let wal_bytes: i64 = sqlx::query_scalar(
        "SELECT pg_wal_lsn_diff(pg_current_wal_insert_lsn(),$1::pg_lsn)::bigint",
    )
    .bind(&lsn)
    .fetch_one(&mut *connection)
    .await?;
    evidence["isolated_cleanup"] = json!({"ms":ms,"wal_bytes":wal_bytes,"reclamation":outcome,
        "wal_scope":"whole owned cluster, no application clients during interval, background work remains included"});
    ensure!(
        outcome["records"] == count as u64,
        "isolated cleanup incomplete"
    );
    evidence["isolated_after"] = diagnostics(pool).await?;
    let vacuum_start = Instant::now();
    sqlx::raw_sql(
        "VACUUM (ANALYZE) cache_entry,cache_ingest_chunk,cache_generation,workflow_cache_iteration",
    )
    .execute(&mut *connection)
    .await?;
    evidence["vacuum_ms"] = json!(vacuum_start.elapsed().as_secs_f64() * 1000.0);
    evidence["after_vacuum"] = diagnostics(pool).await?;
    Ok(())
}

async fn metadata_cleanup(
    pool: &PgPool,
    recorder: &Recorder,
    namespace: i64,
    workflow: i64,
    evidence: &mut Value,
) -> Result<()> {
    // Separate workloads prevent high chunk and terminal-iteration costs being
    // attributed to dropping the entry relation.
    for (label, chunks, iterations) in [
        ("high_chunk", 10_000, 0),
        ("terminal_iterations", 0, 10_000),
    ] {
        let mut contract = input(namespace, label, 0, None);
        contract.expected_chunk_count = chunks;
        let generation = create(pool, recorder, label, &contract).await?;
        for chunk in 0..chunks {
            recorder
                .timed(
                    label,
                    "empty_ingest",
                    CacheIngestRepository::insert_chunk_with_policy(
                        pool,
                        generation.id,
                        chunk,
                        &format!("empty-{chunk}"),
                        &[],
                        &admission(),
                    ),
                )
                .await?;
        }
        recorder
            .timed(
                label,
                "seal",
                CacheGenerationRepository::seal(pool, generation.id),
            )
            .await?;
        for index in 0..iterations {
            pin(
                pool,
                recorder,
                label,
                workflow,
                PinTarget {
                    namespace,
                    generation: generation.id,
                },
                &format!("{label}-{index}"),
                true,
            )
            .await?;
        }
        recorder
            .timed(
                label,
                "fail",
                CacheGenerationRepository::fail(pool, generation.id, "owned metadata measurement"),
            )
            .await?;
        let start = Instant::now();
        let outcome = recorder.cleanup(pool, generation.id, label).await?;
        evidence[label] = json!({"chunks":chunks,"terminal_iterations":iterations,
            "ms":start.elapsed().as_secs_f64()*1000.0,"outcome":format!("{outcome:?}")});
        ensure!(
            matches!(outcome, CacheGenerationCleanupOutcome::Dropped { .. }),
            "metadata cleanup deferred"
        );
    }
    Ok(())
}

fn scan_sql() -> Result<String> {
    let source = include_str!("../src/repositories/cache.rs");
    let start = source
        .find("\"WITH candidates AS MATERIALIZED")
        .context("repository bounded scan missing")?
        + 1;
    let tail = &source[start..];
    let end = tail
        .find("\",\n")
        .context("repository bounded scan end missing")?;
    let columns = CACHE_ENTRY_SELECT_COLUMNS
        .split(',')
        .map(|c| format!("e.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(tail[..end]
        .replace("\\\n", "")
        .replace("\\\"", "\"")
        .replace("{}", &columns))
}

fn visited_relations(value: &Value, names: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if object
                .get("Actual Loops")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                > 0.0
            {
                if let Some(name) = object.get("Relation Name").and_then(Value::as_str) {
                    if name.starts_with("cache_entry_g_") {
                        names.push(name.to_owned());
                    }
                }
            }
            for value in object.values() {
                visited_relations(value, names);
            }
        }
        Value::Array(values) => {
            for value in values {
                visited_relations(value, names);
            }
        }
        _ => {}
    }
}

async fn await_workers(
    tasks: Vec<tokio::task::JoinHandle<Result<Value>>>,
) -> (Vec<Value>, Vec<String>) {
    let mut results = Vec::new();
    let mut failures = Vec::new();
    for task in tasks {
        match task.await {
            Ok(Ok(value)) => results.push(value),
            Ok(Err(error)) => failures.push(error.to_string()),
            Err(error) => failures.push(error.to_string()),
        }
    }
    (results, failures)
}

async fn workload(
    pool: &PgPool,
    recorder: &Recorder,
    records: usize,
    mode: &str,
    arm: &str,
    output: &std::path::Path,
    evidence: &mut Value,
) -> Result<()> {
    let workflow = recorder
        .timed(
            "setup",
            "workflow_bootstrap",
            create_workflow_root(pool, "owner"),
        )
        .await?;
    let mut pin_roots = Vec::new();
    for root in 0..PIN_ROOTS {
        pin_roots.push(
            recorder
                .timed(
                    "setup",
                    "workflow_bootstrap",
                    create_workflow_root(pool, &format!("pinroot{root}")),
                )
                .await?,
        );
    }
    let mut namespaces = Vec::new();
    let mut inventory = Vec::new();
    let mut live_pins = Vec::new();
    for namespace_index in 0..NAMESPACES {
        let namespace = recorder
            .timed(
                "setup",
                "namespace",
                CacheNamespaceRepository::create_api_with_policy(
                    pool,
                    CreateCacheNamespaceInput {
                        owner: CacheOwnerScope::system(),
                        namespace: format!("measure-{namespace_index}"),
                        policy: CacheNamespacePolicy {
                            max_records_per_generation: records as i64,
                            max_generation_bytes: 4 * 1024 * 1024 * 1024,
                            max_retained_bytes: 64 * 1024 * 1024 * 1024,
                            max_retained_generations: 128,
                            max_staging_generations: 128,
                            ..Default::default()
                        },
                    },
                    &admission(),
                ),
            )
            .await?;
        namespaces.push(namespace.id);
        let count = if namespace_index < LARGE_NAMESPACES {
            records
        } else {
            SMALL
        };
        let mut generations = Vec::new();
        let mut prior = None;
        for retained in 0..RETAINED {
            let contract = input(namespace.id, &format!("seed-{retained}"), count, prior);
            let generation = create(pool, recorder, "setup", &contract).await?;
            recorder
                .timed(
                    "setup",
                    "refresh_retry",
                    CacheGenerationRepository::create_or_get_with_policy(
                        pool,
                        &contract,
                        &admission(),
                    ),
                )
                .await?;
            upload(pool, recorder, "setup", &generation, count).await?;
            publish(pool, recorder, "setup", &generation, prior).await?;
            if retained == 0 {
                live_pins.push(
                    pin(
                        pool,
                        recorder,
                        "setup",
                        workflow,
                        PinTarget {
                            namespace: namespace.id,
                            generation: generation.id,
                        },
                        &format!("live-{namespace_index}"),
                        false,
                    )
                    .await?,
                );
            }
            prior = Some(generation.id);
            generations.push(generation.id);
        }
        inventory.push(generations);
    }
    // Exactly 128 generations: 32 retained plus 96 empty staging/failed headroom.
    let mut headroom = Vec::new();
    for index in 0..CAP - NAMESPACES * RETAINED {
        let contract = input(
            namespaces[index % NAMESPACES],
            &format!("headroom-{index}"),
            0,
            None,
        );
        let generation = create(pool, recorder, "capacity", &contract).await?;
        if index < 16 {
            recorder
                .timed(
                    "capacity",
                    "fail",
                    CacheGenerationRepository::fail(pool, generation.id, "owned failed headroom"),
                )
                .await?;
        }
        headroom.push(generation);
    }
    evidence["at_cap"] = diagnostics(pool).await?;
    ensure!(
        evidence["at_cap"]["generations"] == CAP as i64,
        "capacity setup incomplete"
    );
    if arm == "treatment" {
        ensure!(
            evidence["at_cap"]["partitions"] == CAP as i64,
            "partition inventory mismatch"
        );
    }
    let extra = recorder
        .timed(
            "capacity",
            "cap_rejection",
            CacheGenerationRepository::create_or_get_with_policy(
                pool,
                &input(namespaces[0], "over-cap", 0, None),
                &admission(),
            ),
        )
        .await;
    ensure!(extra.is_err(), "new refresh accepted over cap");
    recorder
        .timed(
            "capacity",
            "retry_at_cap",
            CacheGenerationRepository::create_or_get_with_policy(
                pool,
                &input(namespaces[0], "headroom-0", 0, None),
                &admission(),
            ),
        )
        .await?;
    for (index, pins) in inventory.iter().enumerate() {
        let protected = recorder.cleanup(pool, pins[0], "pin_protection").await?;
        ensure!(
            matches!(protected, CacheGenerationCleanupOutcome::Ineligible),
            "live pin failed for namespace {index}"
        );
    }
    for generation in headroom.iter().take(16) {
        let outcome = recorder
            .cleanup(pool, generation.id, "capacity_release")
            .await?;
        ensure!(
            matches!(outcome, CacheGenerationCleanupOutcome::Dropped { .. }),
            "headroom cleanup deferred"
        );
    }
    // Warm scans cover the exact same full active payloads; they are reported as
    // setup, never omitted from evidence or charged to the timed burst.
    if mode == "warm" {
        for (index, namespace) in namespaces.iter().enumerate() {
            let mut cursor = None;
            let mut count = 0;
            loop {
                let page = recorder
                    .timed(
                        "warmup",
                        "page",
                        CacheEntryRepository::scan_pinned_page(
                            pool,
                            *namespace,
                            inventory[index][RETAINED - 1],
                            cursor.as_deref(),
                            1000,
                        ),
                    )
                    .await?;
                count += page.entries.len();
                if !page.has_more {
                    break;
                }
                cursor = page.entries.last().map(|entry| entry.external_id.clone());
            }
            ensure!(
                count
                    == if index < LARGE_NAMESPACES {
                        records
                    } else {
                        SMALL
                    },
                "warmup coverage mismatch"
            );
        }
    }
    evidence["before_burst"] = diagnostics(pool).await?;
    let statistics = recorder
        .timed(
            "setup",
            "statistics",
            CacheStorageRepository::refresh_statistics(pool, &retention()),
        )
        .await?;
    evidence["statistics_outcome"] = json!(format!("{statistics:?}"));
    if statistics != CacheStatisticsRefreshOutcome::Applied {
        evidence["method_gaps"]
            .as_array_mut()
            .unwrap()
            .push(json!("parent statistics did not apply before burst"));
    }
    // Protect the old small snapshots while their replacement is published.
    let mut reader_pins = Vec::new();
    for client in 0..CLIENTS {
        reader_pins.push(
            pin(
                pool,
                recorder,
                "setup",
                workflow,
                PinTarget {
                    namespace: namespaces[LARGE_NAMESPACES + client],
                    generation: inventory[LARGE_NAMESPACES + client][RETAINED - 1],
                },
                &format!("reader-{client}"),
                false,
            )
            .await?,
        );
    }
    // The owning parent restarts this disposable server for a shared-buffer-cold
    // sample. No workers or checked-out connections exist at this boundary.
    std::fs::write(
        output.with_extension("seeded"),
        "repository seed complete\n",
    )?;
    let ready_deadline = Instant::now() + std::time::Duration::from_secs(300);
    while !output.with_extension("continue").exists() {
        ensure!(
            Instant::now() < ready_deadline,
            "owning parent did not acknowledge seed readiness"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let barrier = Arc::new(Barrier::new(NAMESPACES + CLIENTS + PIN_ROOTS + 1));
    let mut senders = Vec::new();
    let mut receivers = Vec::new();
    for _ in 0..CLIENTS {
        let (sender, receiver) = oneshot::channel();
        senders.push(Some(sender));
        receivers.push(receiver);
    }
    // Finish fallible worker setup before spawning anything. A connection/setup
    // failure must not leave sibling workers waiting at an unfinishable barrier.
    let mut connections = Vec::new();
    for _ in 0..CLIENTS {
        let mut connection = pool.acquire().await?;
        sqlx::raw_sql("SET plan_cache_mode=force_generic_plan")
            .execute(&mut *connection)
            .await?;
        let before = memory(&mut connection).await?;
        connections.push((connection, before));
    }
    let mut tasks = Vec::new();
    for index in 0..NAMESPACES {
        let pool = pool.clone();
        let recorder = recorder.clone();
        let barrier = barrier.clone();
        let namespace = namespaces[index];
        let prior = inventory[index][RETAINED - 1];
        let cleanup = inventory[index][1];
        let sender = if index < CLIENTS {
            senders[index].take()
        } else {
            None
        };
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            // Creation and cleanup are separate calls, and all failures/deferrals
            // remain visible. There is no retry hidden inside the driver.
            let generation = create(
                &pool,
                &recorder,
                "burst",
                &input(
                    namespace,
                    "burst",
                    if index < LARGE_NAMESPACES {
                        records
                    } else {
                        SMALL
                    },
                    Some(prior),
                ),
            )
            .await?;
            if let Some(sender) = sender {
                sender
                    .send(generation.clone())
                    .map_err(|_| anyhow::anyhow!("ingest client stopped"))?;
            }
            let outcome = recorder.reclaim(&pool, cleanup, "burst").await?;
            if index >= LARGE_NAMESPACES {
                upload(&pool, &recorder, "burst", &generation, SMALL).await?;
                publish(&pool, &recorder, "burst", &generation, Some(prior)).await?;
            }
            Ok::<Value, anyhow::Error>(
                json!({"namespace":namespace,"generation":generation.id,"reclamation":outcome}),
            )
        }));
    }
    // Both roots mutate the same two already-live-pinned generations in opposite
    // orders. PinMutation must serialize entry before root/generation/counter
    // rows, while the four pure-read/ingest clients retain normal Read behavior.
    let pairs = [
        (namespaces[0], inventory[0][0]),
        (namespaces[1], inventory[1][0]),
    ];
    let mut pin_workers = Vec::new();
    for (root, workflow) in pin_roots.into_iter().enumerate() {
        let pool = pool.clone();
        let recorder = recorder.clone();
        let barrier = barrier.clone();
        pin_workers.push(tokio::spawn(async move {
            barrier.wait().await;
            for round in 0..PIN_ROUNDS {
                pin_pair(&pool, &recorder, workflow, root, round, pairs).await?;
            }
            Ok::<_, anyhow::Error>(json!({"pin_root":root,"workflow":workflow,
                "transactions":PIN_ROUNDS,"generations_per_transaction":2,
                "order":pin_generation_order(root,pairs)}))
        }));
    }
    // Each long-lived SQLx connection serves generic prepared repository scans
    // through the DDL burst and alternates reads with 1000-record ingest chunks.
    let mut clients = Vec::new();
    for (client, (receiver, (mut connection, before))) in
        receivers.into_iter().zip(connections).enumerate()
    {
        let pool = pool.clone();
        let recorder = recorder.clone();
        let barrier = barrier.clone();
        let small_namespace = namespaces[LARGE_NAMESPACES + client];
        let small_generation = inventory[LARGE_NAMESPACES + client][RETAINED - 1];
        let prior = inventory[client][RETAINED - 1];
        clients.push(tokio::spawn(async move {
            barrier.wait().await;
            let generation = receiver.await.context("refresh creator stopped")?;
            for round in 0..READ_ROUNDS.max(records.div_ceil(CHUNK)) {
                recorder
                    .timed(
                        "burst",
                        "small_point",
                        CacheEntryRepository::find_active(
                            &pool,
                            small_namespace,
                            &format!("record-{:010}", round % SMALL),
                        ),
                    )
                    .await?
                    .context("small point missing during cleanup")?;
                let page = recorder
                    .timed("burst", "small_page", async {
                        let mut tx = connection.begin().await?;
                        let page = CacheEntryRepository::scan_pinned_page_with_conn(
                            &mut tx,
                            small_namespace,
                            small_generation,
                            None,
                            100,
                        )
                        .await?;
                        tx.commit().await?;
                        Ok::<_, anyhow::Error>(page)
                    })
                    .await?;
                ensure!(page.entries.len() == 100, "small page coverage changed");
                if round < records.div_ceil(CHUNK) {
                    let values = entries(round * CHUNK, CHUNK);
                    let checksum = digest_hex(&serde_json::to_vec(
                        &values.iter().map(|e| &e.value).collect::<Vec<_>>(),
                    )?);
                    recorder
                        .timed("burst", "ingest", async {
                            let mut tx = connection.begin().await?;
                            CacheIngestRepository::insert_chunk_with_policy_conn(
                                &mut tx,
                                generation.id,
                                round as i32,
                                &checksum,
                                &values,
                                &admission(),
                            )
                            .await?;
                            tx.commit().await?;
                            Ok::<_, anyhow::Error>(())
                        })
                        .await?;
                    if round == 0 {
                        recorder
                            .timed(
                                "burst",
                                "replay",
                                CacheIngestRepository::insert_chunk_with_policy(
                                    &pool,
                                    generation.id,
                                    0,
                                    &checksum,
                                    &values,
                                    &admission(),
                                ),
                            )
                            .await?;
                    }
                }
            }
            publish(&pool, &recorder, "burst", &generation, Some(prior)).await?;
            let after = memory(&mut connection).await?;
            Ok::<Value, anyhow::Error>(json!({"before":before,"after":after}))
        }));
    }
    barrier.wait().await;
    let (results, failures) = await_workers(
        tasks
            .into_iter()
            .chain(clients)
            .chain(pin_workers)
            .collect(),
    )
    .await;
    evidence["workers"] = json!(results);
    evidence["worker_failures"] = json!(failures);
    // All workers have completed before returning an error or touching teardown.
    ensure!(failures.is_empty(), "burst worker failures");
    for iteration in reader_pins {
        let mut tx = pool.begin().await?;
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
            .await?;
        WorkflowCacheIterationRepository::mark_terminal(
            &mut *tx,
            iteration,
            WorkflowCacheIterationState::Completed,
            None,
        )
        .await?;
        tx.commit().await?;
    }
    evidence["after_burst"] = diagnostics(pool).await?;
    ensure!(
        accounting_matches(&evidence["after_burst"]),
        "burst accounting drift"
    );
    let mut prepared = pool.acquire().await?;
    let sql = scan_sql()?;
    sqlx::raw_sql(&format!("SET plan_cache_mode=force_generic_plan; PREPARE measured_scan(bigint,text,bigint,bigint) AS {sql}"))
        .execute(&mut *prepared).await?;
    let mut plans = Vec::new();
    for cycle in 0..8 {
        let generation = create(
            pool,
            recorder,
            "repeated_ddl",
            &input(namespaces[0], &format!("ddl-{cycle}"), 0, None),
        )
        .await?;
        recorder
            .timed(
                "repeated_ddl",
                "fail",
                CacheGenerationRepository::fail(pool, generation.id, "owned ddl cycle"),
            )
            .await?;
        let outcome = recorder
            .cleanup(pool, generation.id, "repeated_ddl")
            .await?;
        ensure!(
            matches!(outcome, CacheGenerationCleanupOutcome::Dropped { .. }),
            "repeated DDL deferred"
        );
        let current = CacheGenerationRepository::list_for_namespace(pool, namespaces[4], 128)
            .await?
            .into_iter()
            .find(|g| g.client_refresh_id == "burst")
            .context("small active generation missing")?;
        ensure!(current.id > 0, "generation is not positive i64");
        let plan: Value = sqlx::query_scalar(&format!(
            "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) EXECUTE measured_scan({},NULL,100,1048576)",
            current.id
        ))
        .fetch_one(&mut *prepared)
        .await?;
        let mut visited = Vec::new();
        visited_relations(&plan, &mut visited);
        if arm == "treatment" {
            ensure!(
                !visited.is_empty()
                    && visited
                        .iter()
                        .all(|r| r == &format!("cache_entry_g_{}", current.id)),
                "prepared plan failed generation pruning: {visited:?}"
            );
        }
        plans.push(json!({"cycle":cycle,"visited":visited,"plan":plan,"memory":memory(&mut prepared).await?}));
    }
    evidence["prepared_plans_after_ddl"] = json!(plans);
    drop(prepared);
    // Metadata profiles and isolated WAL are outside concurrent application work.
    isolated_cleanup(pool, recorder, namespaces[0], records, evidence).await?;
    metadata_cleanup(pool, recorder, namespaces[0], workflow, evidence).await?;
    for iteration in live_pins {
        let mut tx = pool.begin().await?;
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
            .await?;
        WorkflowCacheIterationRepository::mark_terminal(
            &mut *tx,
            iteration,
            WorkflowCacheIterationState::Completed,
            None,
        )
        .await?;
        tx.commit().await?;
    }
    evidence["final"] = diagnostics(pool).await?;
    ensure!(
        accounting_matches(&evidence["final"]),
        "final accounting drift"
    );
    ensure!(
        evidence["final"]["entries"]
            == (LARGE_NAMESPACES * RETAINED * records
                + (NAMESPACES - LARGE_NAMESPACES) * RETAINED * SMALL) as i64,
        "entry inventory drift"
    );
    ensure!(
        evidence["final"]["generations"] == (CAP - 16) as i64,
        "generation leak or drift"
    );
    ensure!(
        evidence["final"]["iterations"]
            == (NAMESPACES + CLIENTS + PIN_ROOTS * PIN_ROUNDS * 2) as i64,
        "pin replay accounting or iteration inventory drift"
    );
    if arm == "treatment" {
        ensure!(
            evidence["final"]["partitions"] == (CAP - 16) as i64,
            "partition leak or drift"
        );
    }
    ensure!(
        evidence["final"]["deadlocks"] == 0,
        "workload deadlock detected"
    );
    evidence["correct"] = json!(true);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 5,
        "expected record-count cold|warm baseline|treatment output.json"
    );
    let records: usize = args[1].parse()?;
    let smoke = std::env::var("CACHE_MEASURE_SMOKE").as_deref() == Ok("1");
    ensure!(
        records == 200_000 || records == 1_000_000 || (smoke && records == 1000),
        "undeclared record count"
    );
    ensure!(matches!(args[2].as_str(), "cold" | "warm"), "invalid mode");
    ensure!(
        matches!(args[3].as_str(), "baseline" | "treatment"),
        "invalid arm"
    );
    let output = PathBuf::from(&args[4]);
    ensure!(!output.exists(), "refusing existing evidence");
    ensure!(
        std::env::var("CACHE_MEASURE_CONTROLLED").as_deref() == Ok("1"),
        "use scripts/measure-cache-generation-partitions.py with an owned exclusive lane"
    );
    let mut config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    config.database.max_connections = 32;
    config.database.min_connections = 0;
    let recorder = Recorder::default();
    let mut evidence = json!({"records":records,"mode":args[2],"arm":args[3],"correct":false,"smoke_only":smoke,
        "inventory":{"namespaces":NAMESPACES,"large_namespaces":LARGE_NAMESPACES,"small_records":SMALL,
            "retained_per_namespace":RETAINED,"partition_cap":CAP,"clients":CLIENTS,
            "headroom":96,"failed_headroom":16,"live_pins":8},
        "pin_burst":{"roots":PIN_ROOTS,"transactions_per_root":PIN_ROUNDS,
            "generations_per_transaction":2,"opposite_generation_order":true,
            "replays_per_iteration":1,"protocol":"PinMutation before root/generation/counter rows"},
        "sqlx_pool_max_connections":32,
        "payload":"cache-measure-v1; 128-byte unique SHA256 body; every 32nd body 4096 bytes",
        "method_gaps":[],
        "scope_notes":["host page cache is not evicted; cold refers to PostgreSQL shared buffers",
            "one cold and one warm pair per profile, no multi-pair confidence interval",
            "executor and authenticated HTTP lifecycle interleavings require their separate correctness gates"],
        "cold_definition":"fresh owned PostgreSQL cluster, identical full repository seed and ANALYZE, server restart immediately before burst",
        "warm_definition":"fresh owned PostgreSQL cluster, same seed plus complete active-generation repository scans"});
    std::fs::write(&output, serde_json::to_string_pretty(&evidence)?)?;
    let db = match TestDatabase::create(&config.database).await {
        Ok(db) => db.with_cleanup_on_drop(),
        Err(error) => {
            evidence["error"] = json!(error.to_string());
            std::fs::write(&output, serde_json::to_string_pretty(&evidence)?)?;
            return Err(error.into());
        }
    };
    let result = workload(
        db.pool(),
        &recorder,
        records,
        &args[2],
        &args[3],
        &output,
        &mut evidence,
    )
    .await;
    if let Err(error) = &result {
        evidence["error"] = json!(format!("{error:#}"));
    }
    let calls = recorder.calls.lock().await.clone();
    evidence["calls"] = json!(calls);
    let overlap_reads = calls
        .iter()
        .filter(|c| {
            c.phase == "burst"
                && c.during_cleanup
                && matches!(c.operation.as_str(), "small_point" | "small_page")
        })
        .count();
    evidence["cleanup_overlapping_read_samples"] = json!(overlap_reads);
    if overlap_reads < 20 {
        evidence["method_gaps"]
            .as_array_mut()
            .unwrap()
            .push(json!("fewer than 20 reads overlap cleanup"));
    }
    std::fs::write(&output, serde_json::to_string_pretty(&evidence)?)?;
    let cleanup = db.cleanup().await;
    evidence["cleanup_complete"] = json!(cleanup.is_ok());
    if let Err(error) = &cleanup {
        evidence["cleanup_error"] = json!(error.to_string());
    }
    std::fs::write(&output, serde_json::to_string_pretty(&evidence)?)?;
    cleanup?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_payload_and_toast_cohort() {
        assert_eq!(payload(42), payload(42));
        assert_ne!(payload(42), payload(43));
        assert_eq!(payload(32)["body"].as_str().unwrap().len(), 4096);
        assert_eq!(payload(33)["body"].as_str().unwrap().len(), 128);
    }

    #[test]
    fn exact_inventory_and_chunk_contracts() {
        assert_eq!(NAMESPACES * RETAINED + 96, CAP);
        assert_eq!(
            input(1, "fixture", 1_000_000, None).expected_chunk_count,
            1000
        );
        assert_eq!(entries(123, 2)[0].external_id, "record-0000000123");
        let per_arm = |large| {
            (LARGE_NAMESPACES * (RETAINED + 1) + 1) * large
                + (NAMESPACES - LARGE_NAMESPACES) * (RETAINED + 1) * SMALL
        };
        assert_eq!(
            (per_arm(200_000) + per_arm(1_000_000)) * 2 * 2 * 2,
            201_920_000
        );
    }

    #[test]
    fn pin_roots_share_generations_in_opposite_orders_without_changing_entry_inventory() {
        let pairs = [(1, 10), (2, 20)];
        assert_eq!(pin_generation_order(0, pairs), pairs);
        assert_eq!(pin_generation_order(1, pairs), [(2, 20), (1, 10)]);
        assert_eq!(PIN_ROOTS * PIN_ROUNDS * 2, 800);
        assert_eq!(NAMESPACES * RETAINED + 96, CAP);
    }

    #[test]
    fn actual_repository_scan_is_selected() {
        let sql = scan_sql().unwrap();
        assert!(sql.contains("e.generation = $1"));
        assert!(sql.contains("e.external_id"));
        assert!(!sql.contains("{}"));
    }

    #[tokio::test]
    async fn failed_calls_are_not_discarded() {
        let recorder = Recorder::default();
        let result = recorder
            .timed("burst", "create", async { Err::<(), _>("fixture failure") })
            .await;
        assert!(result.is_err());
        let calls = recorder.calls.lock().await;
        assert_eq!(calls.len(), 1);
        assert!(calls[0].outcome.contains("fixture failure"));
    }

    #[tokio::test]
    async fn completed_cleanup_inside_a_call_still_counts_as_overlap() {
        let recorder = Recorder::default();
        recorder
            .timed("burst", "small_page", async {
                recorder.cleanup_epoch.fetch_add(2, Ordering::SeqCst);
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        assert!(recorder.calls.lock().await[0].during_cleanup);
    }

    #[tokio::test]
    async fn failed_worker_does_not_skip_sibling_completion() {
        let (sender, receiver) = oneshot::channel();
        let failed = tokio::spawn(async move {
            sender.send(()).unwrap();
            anyhow::bail!("owned worker failure")
        });
        let sibling = tokio::spawn(async move {
            receiver.await?;
            Ok(json!({"joined":true}))
        });
        let (results, failures) = await_workers(vec![failed, sibling]).await;
        assert_eq!(failures.len(), 1);
        assert_eq!(results, vec![json!({"joined":true})]);
    }
}
