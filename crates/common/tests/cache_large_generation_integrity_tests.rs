//! Full-value integrity through protected reads and atomic generation reclamation.

mod helpers;

use anyhow::{ensure, Context, Result};
use attune_common::{
    config::{CacheRetentionConfig, Config},
    models::{CacheGeneration, ExecutionStatus, WorkflowCacheIterationState},
    repositories::{
        cache::{
            CacheEntryInput, CacheEntryRepository, CacheGenerationCleanupOutcome,
            CacheGenerationRepository, CacheIngestRepository, CacheNamespacePolicy,
            CacheNamespaceRepository, CacheOwnerScope, CacheTransactionMode,
            CreateCacheGenerationInput, CreateCacheGenerationResult, CreateCacheNamespaceInput,
            InsertCacheChunkResult,
        },
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            UpdateWorkflowExecutionInput, WorkflowDefinitionRepository,
            WorkflowExecutionRepository,
        },
        workflow_cache_iteration::{
            CreateWorkflowCacheIterationInput, WorkflowCacheIterationRepository,
        },
        Create, FindById, Update,
    },
    test_database::TestDatabase,
    Error,
};
use chrono::{DateTime, Utc};
use futures::FutureExt;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use std::{panic::AssertUnwindSafe, time::Duration};

const RECORDS: usize = 1_000_000;
const CHUNK_RECORDS: usize = 1_000;
const PAGE_RECORDS: i64 = 1_000;
const FAULT_MESSAGE: &str = "large integrity fault after atomic DROP";

fn lowercase_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn record(index: usize) -> CacheEntryInput {
    // Include non-repeating TOAST-sized values without making the whole cohort large.
    let blocks = if index.is_multiple_of(64) { 64 } else { 1 };
    let mut body = String::with_capacity(blocks * 64);
    for block in 0..blocks {
        body.push_str(&lowercase_hex(
            Sha256::digest(format!("large-integrity:{index}:{block}").as_bytes()).as_ref(),
        ));
    }
    let value = json!({
        "ordinal": index,
        "body": body,
        "nested": {"even": index.is_multiple_of(2), "label": format!("row-{index}")},
        "tags": [index % 7, "cache", null],
    });
    CacheEntryInput {
        external_id: format!("record-{index:010}"),
        source_checksum: (!index.is_multiple_of(7))
            .then(|| lowercase_hex(Sha256::digest(serde_jcs::to_vec(&value).unwrap()).as_ref())),
        value,
        source_updated_at: (!index.is_multiple_of(5)).then(|| {
            DateTime::from_timestamp(946_684_800 + i64::try_from(index).unwrap(), 123_000_000)
                .unwrap()
        }),
    }
}

fn hash_value(hash: &mut Sha256, value: &impl Serialize) -> Result<()> {
    let bytes = serde_jcs::to_vec(value)?;
    hash.update(u64::try_from(bytes.len())?.to_be_bytes());
    hash.update(bytes);
    Ok(())
}

fn hash_source(hash: &mut Sha256, entry: &CacheEntryInput) -> Result<()> {
    hash_value(
        hash,
        &(
            &entry.external_id,
            &entry.value,
            entry.source_updated_at,
            &entry.source_checksum,
        ),
    )
}

fn chunk_checksum(entries: &[CacheEntryInput]) -> Result<String> {
    let mut hash = Sha256::new();
    for entry in entries {
        hash_source(&mut hash, entry)?;
    }
    Ok(lowercase_hex(hash.finalize().as_ref()))
}

async fn generation(
    pool: &PgPool,
    namespace: i64,
    refresh: &str,
    count: usize,
    previous: Option<i64>,
    execution: i64,
) -> Result<CacheGeneration> {
    let result = CacheGenerationRepository::create_or_get(
        pool,
        &CreateCacheGenerationInput {
            namespace,
            client_refresh_id: refresh.into(),
            expected_active_generation: previous,
            expected_chunk_count: i32::try_from(count.div_ceil(CHUNK_RECORDS))?,
            expected_count: Some(i64::try_from(count)?),
            expected_bytes: None,
            checksum_algorithm: None,
            checksum: None,
            source_revision: Some(format!("large-integrity-{refresh}")),
            created_by: None,
            created_by_execution: Some(execution),
        },
    )
    .await?;
    match result {
        CreateCacheGenerationResult::Created(generation) => Ok(generation),
        CreateCacheGenerationResult::Existing(_) => anyhow::bail!("fresh fixture reused storage"),
    }
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Accounting {
    deployment_bytes: i64,
    owner_bytes: i64,
    partitions: i64,
    partitions_created: i64,
    partitions_dropped: i64,
    requested_revision: i64,
    completed_revision: i64,
}

async fn accounting(pool: &PgPool) -> Result<Accounting> {
    // Read-only observations on this fixture's physical clone, not service queries.
    Ok(sqlx::query_as(
        "SELECT (SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id=1) AS deployment_bytes,
         COALESCE((SELECT physical_bytes FROM cache_owner_physical_byte_usage
                   WHERE owner_type='system' AND owner='system'),0)::BIGINT AS owner_bytes,
         (SELECT count(*) FROM pg_inherits WHERE inhparent='cache_entry'::regclass) AS partitions,
         partitions_created, partitions_dropped, requested_revision, completed_revision
         FROM cache_entry_statistics_state WHERE id=TRUE",
    )
    .fetch_one(pool)
    .await?)
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    accounting: Accounting,
    physical_leaf: Option<i64>,
    attached_leaf: Option<(i64, String)>,
    usage: Option<(i64, i64, i64)>,
    generation: Option<Value>,
    namespace: Value,
    iteration: Option<Value>,
    chunks: usize,
    chunk_metadata_sha256: [u8; 32],
}

async fn snapshot(
    pool: &PgPool,
    namespace: i64,
    generation: i64,
    workflow: i64,
) -> Result<Snapshot> {
    let usage = sqlx::query_as(
        "SELECT record_count, physical_bytes, retained_iterations
         FROM cache_generation_entry_usage WHERE generation=$1",
    )
    .bind(generation)
    .fetch_optional(pool)
    .await?;
    let chunks = CacheIngestRepository::list_chunks(pool, generation).await?;
    let mut chunk_hash = Sha256::new();
    for (index, chunk) in chunks.iter().enumerate() {
        ensure!(
            chunk.chunk_index == i32::try_from(index)?,
            "chunk identity/order changed"
        );
        hash_value(&mut chunk_hash, chunk)?;
    }
    Ok(Snapshot {
        accounting: accounting(pool).await?,
        physical_leaf: sqlx::query_scalar(
            "SELECT to_regclass(cache_generation_partition_name($1))::OID::BIGINT",
        )
        .bind(generation)
        .fetch_one(pool)
        .await?,
        attached_leaf: sqlx::query_as(
            "SELECT child.oid::BIGINT, pg_get_expr(child.relpartbound,child.oid)
             FROM pg_class child JOIN pg_inherits i ON i.inhrelid=child.oid
             WHERE i.inhparent='cache_entry'::regclass
               AND child.relname=cache_generation_partition_name($1)",
        )
        .bind(generation)
        .fetch_optional(pool)
        .await?,
        usage,
        generation: CacheGenerationRepository::find_by_id(pool, generation)
            .await?
            .map(serde_json::to_value)
            .transpose()?,
        namespace: serde_json::to_value(
            CacheNamespaceRepository::find_by_id(pool, namespace)
                .await?
                .context("fixture namespace disappeared")?,
        )?,
        iteration: WorkflowCacheIterationRepository::find_by_workflow_task(
            pool, workflow, "consume",
        )
        .await?
        .map(serde_json::to_value)
        .transpose()?,
        chunks: chunks.len(),
        chunk_metadata_sha256: chunk_hash.finalize().into(),
    })
}

#[derive(Debug, PartialEq, Eq)]
struct Fingerprint {
    records: usize,
    bytes: i64,
    source_sha256: [u8; 32],
    row_metadata_sha256: [u8; 32],
}

async fn fingerprint(
    conn: &mut PgConnection,
    generation: &CacheGeneration,
    expected_source_sha256: [u8; 32],
    created_between: (DateTime<Utc>, DateTime<Utc>),
) -> Result<Fingerprint> {
    let expected_generation = serde_json::to_value(generation)?;
    let mut cursor = None;
    let mut records = 0_usize;
    let mut bytes = 0_i64;
    let mut source_hash = Sha256::new();
    let mut row_hash = Sha256::new();
    loop {
        let page = CacheEntryRepository::scan_pinned_page_with_conn(
            conn,
            generation.namespace,
            generation.id,
            cursor.as_deref(),
            PAGE_RECORDS,
        )
        .await?;
        ensure!(
            serde_json::to_value(&page.generation)? == expected_generation,
            "page generation metadata changed"
        );
        ensure!(
            !page.entries.is_empty() && page.entries.len() <= CHUNK_RECORDS,
            "invalid full-generation page size"
        );
        for entry in &page.entries {
            ensure!(
                records < RECORDS,
                "scan returned extra or duplicate identities"
            );
            let expected = record(records);
            // Fixed-width bytewise IDs define the oracle independently of insert order.
            ensure!(
                entry.external_id == expected.external_id,
                "missing, duplicate or out-of-order identity at {records}"
            );
            ensure!(entry.value == expected.value, "value differs at {records}");
            ensure!(
                entry.source_updated_at == expected.source_updated_at,
                "source timestamp differs at {records}"
            );
            ensure!(
                entry.source_checksum == expected.source_checksum,
                "source checksum differs at {records}"
            );
            ensure!(
                entry.generation == generation.id && entry.id > 0 && entry.size_bytes > 0,
                "invalid stored row metadata at {records}"
            );
            ensure!(
                entry.created >= created_between.0 && entry.created <= created_between.1,
                "creation time outside committed ingest at {records}"
            );
            hash_value(
                &mut source_hash,
                &(
                    &entry.external_id,
                    &entry.value,
                    entry.source_updated_at,
                    &entry.source_checksum,
                ),
            )?;
            // Include every returned field, including DB-assigned ID, time and size.
            hash_value(&mut row_hash, entry)?;
            bytes = bytes
                .checked_add(entry.size_bytes)
                .context("scan byte overflow")?;
            records += 1;
        }
        cursor = page.entries.last().map(|entry| entry.external_id.clone());
        ensure!(
            page.has_more == (records < RECORDS),
            "continuation disagrees with exact coverage"
        );
        if !page.has_more {
            break;
        }
    }
    let result = Fingerprint {
        records,
        bytes,
        source_sha256: source_hash.finalize().into(),
        row_metadata_sha256: row_hash.finalize().into(),
    };
    ensure!(
        result.records == RECORDS && generation.record_count == i64::try_from(RECORDS)?,
        "full scan count mismatch"
    );
    ensure!(
        result.bytes == generation.size_bytes,
        "row bytes disagree with sealed usage"
    );
    ensure!(
        result.source_sha256 == expected_source_sha256,
        "full source digest mismatch"
    );
    Ok(result)
}

async fn install_drop_fault(
    pool: &PgPool,
    generation: i64,
    remaining_bytes: (i64, i64),
) -> Result<()> {
    // This trigger is exclusive to the clone. It proves the exception occurs after
    // physical DROP, checked quota release, chunk deletion and terminal pin cascade.
    let mut tx = pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation).await?;
    sqlx::raw_sql(
        "CREATE FUNCTION fail_large_integrity_drop() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           IF OLD.id=TG_ARGV[0]::BIGINT THEN
             IF to_regclass(cache_generation_partition_name(OLD.id)) IS NOT NULL
                OR EXISTS(SELECT 1 FROM cache_generation_entry_usage WHERE generation=OLD.id)
                OR EXISTS(SELECT 1 FROM cache_ingest_chunk WHERE generation=OLD.id)
                OR EXISTS(SELECT 1 FROM workflow_cache_iteration WHERE generation=OLD.id)
                OR (SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id=1)
                     IS DISTINCT FROM TG_ARGV[1]::BIGINT
                OR (SELECT physical_bytes FROM cache_owner_physical_byte_usage
                    WHERE owner_type='system' AND owner='system') IS DISTINCT FROM TG_ARGV[2]::BIGINT THEN
               RAISE EXCEPTION 'large integrity fault reached before atomic reclamation' USING ERRCODE='P0002';
             END IF;
             RAISE EXCEPTION 'large integrity fault after atomic DROP' USING ERRCODE='P0001';
           END IF;
           RETURN OLD;
         END; $$;",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::raw_sql(&format!(
        "CREATE TRIGGER zz_fail_large_integrity_drop AFTER DELETE ON cache_generation
         FOR EACH ROW EXECUTE FUNCTION fail_large_integrity_drop('{generation}','{}','{}');",
        remaining_bytes.0, remaining_bytes.1,
    ))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn verify_large_generation(pool: &PgPool) -> Result<()> {
    let retention = CacheRetentionConfig {
        // An owned short traversal window expires naturally, without rewriting
        // lifecycle timestamps or changing any production lock/statement budget.
        min_traversal_window_seconds: 1,
        ..Default::default()
    };
    let pack = helpers::PackFixture::new_unique("large_integrity")
        .create(pool)
        .await?;
    let action = helpers::ActionFixture::new_unique(pack.id, &pack.r#ref, "consume")
        .create(pool)
        .await?;
    let execution = ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            status: ExecutionStatus::Running,
            ..Default::default()
        },
    )
    .await?;
    let definition = WorkflowDefinitionRepository::create(
        pool,
        CreateWorkflowDefinitionInput {
            r#ref: format!("{}.workflow", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref,
            label: "Large cache integrity".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            definition: json!({}),
            tags: vec![],
        },
    )
    .await?;
    let workflow = WorkflowExecutionRepository::create(
        pool,
        CreateWorkflowExecutionInput {
            execution: execution.id,
            workflow_def: definition.id,
            task_graph: json!({}),
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await?;
    let namespace = CacheNamespaceRepository::create(
        pool,
        CreateCacheNamespaceInput {
            owner: CacheOwnerScope::system(),
            namespace: "large-integrity".into(),
            policy: CacheNamespacePolicy {
                max_records_per_generation: i64::try_from(RECORDS)?,
                ..Default::default()
            },
        },
    )
    .await?;
    let baseline_accounting = accounting(pool).await?;
    let large = generation(pool, namespace.id, "million", RECORDS, None, execution.id).await?;
    let ingest_started: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await?;
    let mut expected_source_hash = Sha256::new();
    for chunk in 0..RECORDS / CHUNK_RECORDS {
        let mut entries = (chunk * CHUNK_RECORDS..(chunk + 1) * CHUNK_RECORDS)
            .map(record)
            .collect::<Vec<_>>();
        for entry in &entries {
            hash_source(&mut expected_source_hash, entry)?;
        }
        // Assigned numeric IDs deliberately do not follow the public scan order.
        entries.reverse();
        let checksum = chunk_checksum(&entries)?;
        let inserted = CacheIngestRepository::insert_chunk(
            pool,
            large.id,
            i32::try_from(chunk)?,
            &checksum,
            &entries,
        )
        .await?;
        ensure!(
            matches!(inserted, InsertCacheChunkResult::Inserted(ref metadata) if metadata.record_count == i64::try_from(CHUNK_RECORDS)?),
            "chunk was not inserted exactly once"
        );
    }
    let ingest_finished = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await?;
    let created_between = (ingest_started, ingest_finished);
    let expected_source_sha256: [u8; 32] = expected_source_hash.finalize().into();
    let ingested = snapshot(pool, namespace.id, large.id, workflow.id).await?;
    ensure!(
        ingested.chunks == RECORDS / CHUNK_RECORDS,
        "chunk metadata missing"
    );
    ensure!(
        ingested.physical_leaf.is_some()
            && ingested.attached_leaf.as_ref().map(|leaf| leaf.0) == ingested.physical_leaf,
        "ingested generation has no attached physical leaf"
    );
    let (records, bytes, pins) = ingested.usage.context("missing admitted usage")?;
    ensure!(
        records == i64::try_from(RECORDS)? && bytes > 0 && pins == 0,
        "incorrect ingest usage"
    );
    ensure!(
        ingested.accounting.deployment_bytes == baseline_accounting.deployment_bytes + bytes,
        "deployment ingest delta differs"
    );
    ensure!(
        ingested.accounting.owner_bytes == baseline_accounting.owner_bytes + bytes,
        "owner ingest delta differs"
    );

    let mut replay_entries = (0..CHUNK_RECORDS).map(record).collect::<Vec<_>>();
    replay_entries.reverse();
    let checksum = chunk_checksum(&replay_entries)?;
    ensure!(
        matches!(
            CacheIngestRepository::insert_chunk(pool, large.id, 0, &checksum, &replay_entries)
                .await?,
            InsertCacheChunkResult::Replayed(_)
        ),
        "identical chunk did not replay"
    );
    ensure!(
        snapshot(pool, namespace.id, large.id, workflow.id).await? == ingested,
        "replay changed admitted rows or metadata"
    );
    replay_entries[0].value["body"] = json!("divergent owned fixture payload");
    let divergence = CacheIngestRepository::insert_chunk(
        pool,
        large.id,
        0,
        &chunk_checksum(&replay_entries)?,
        &replay_entries,
    )
    .await;
    ensure!(
        matches!(divergence, Err(Error::AlreadyExists { ref entity, .. }) if entity == "cache_ingest_chunk"),
        "divergent checksum did not return the chunk conflict"
    );
    ensure!(
        snapshot(pool, namespace.id, large.id, workflow.id).await? == ingested,
        "rejected divergence changed storage or metadata"
    );
    drop(replay_entries);

    CacheGenerationRepository::seal(pool, large.id).await?;
    let active = CacheGenerationRepository::promote(pool, namespace.id, large.id, None, Utc::now())
        .await?
        .activated_generation;
    let mut scan = pool.begin().await?;
    let original = fingerprint(&mut scan, &active, expected_source_sha256, created_between).await?;
    scan.commit().await?;
    eprintln!("million-row integrity baseline: {original:?}");
    let active_snapshot = snapshot(pool, namespace.id, large.id, workflow.id).await?;
    ensure!(
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, large.id, &retention).await?
            == CacheGenerationCleanupOutcome::Ineligible,
        "active generation was reclaimable"
    );
    ensure!(
        snapshot(pool, namespace.id, large.id, workflow.id).await? == active_snapshot,
        "active protection changed storage"
    );

    let successor = generation(
        pool,
        namespace.id,
        "empty-successor",
        0,
        Some(large.id),
        execution.id,
    )
    .await?;
    CacheGenerationRepository::seal(pool, successor.id).await?;
    let readable_until: DateTime<Utc> =
        sqlx::query_scalar("SELECT clock_timestamp()+INTERVAL '5 seconds'")
            .fetch_one(pool)
            .await?;
    CacheGenerationRepository::promote(
        pool,
        namespace.id,
        successor.id,
        Some(large.id),
        readable_until,
    )
    .await?;
    let retired = CacheGenerationRepository::find_by_id(pool, large.id)
        .await?
        .context("retired generation missing")?;
    let readable_snapshot = snapshot(pool, namespace.id, large.id, workflow.id).await?;
    ensure!(
        readable_snapshot.usage == Some((records, bytes, 0)),
        "readability control already has a durable pin"
    );
    ensure!(
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, large.id, &retention).await?
            == CacheGenerationCleanupOutcome::Ineligible,
        "readable generation was reclaimable without a workflow pin"
    );
    ensure!(
        snapshot(pool, namespace.id, large.id, workflow.id).await? == readable_snapshot,
        "readability protection changed storage"
    );
    let mut pin = pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut pin, CacheTransactionMode::PinMutation).await?;
    let iteration = WorkflowCacheIterationRepository::create_or_find_for_update(
        &mut pin,
        CreateWorkflowCacheIterationInput {
            workflow_execution: workflow.id,
            task_name: "consume".into(),
            namespace: namespace.id,
            generation: large.id,
            page_size: 1000,
            batch_size: 1000,
            concurrency: 1,
        },
    )
    .await?;
    pin.commit().await?;

    // Reserve transaction time before expiry, but take no cache/source-row locks.
    // After the failed DROP rolls back, this valid pre-expiry read transaction
    // acquires parent protection before its first generation/page snapshot.
    let mut verification = pool.begin().await?;
    let verification_time: DateTime<Utc> = sqlx::query_scalar("SELECT NOW()")
        .fetch_one(&mut *verification)
        .await?;
    let mut reader = pool.begin().await?;
    CacheGenerationRepository::find_by_id_for_share(&mut reader, large.id)
        .await?
        .context("reader generation disappeared")?;
    let reader_time: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await?;
    ensure!(
        verification_time < readable_until && reader_time < readable_until,
        "read transactions did not start before expiry"
    );
    let retired_at = retired
        .retired
        .context("canonical retirement has no timestamp")?;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let expired: bool = sqlx::query_scalar(
                "SELECT clock_timestamp() >= $1
                 AND clock_timestamp() >= $2 + INTERVAL '1 second'",
            )
            .bind(readable_until)
            .bind(retired_at)
            .fetch_one(pool)
            .await?;
            if expired {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("database-time readability expiry was not observed")??;
    let before_reader_deferral = snapshot(pool, namespace.id, large.id, workflow.id).await?;
    ensure!(
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, large.id, &retention).await?
            == CacheGenerationCleanupOutcome::DeferredBusy,
        "held reader did not defer physical DROP"
    );
    ensure!(
        snapshot(pool, namespace.id, large.id, workflow.id).await? == before_reader_deferral,
        "reader deferral released quota or storage"
    );
    let protected = fingerprint(
        &mut reader,
        &retired,
        expected_source_sha256,
        created_between,
    )
    .await?;
    ensure!(
        protected == original,
        "protected reader lost values or row metadata: {protected:?}, baseline {original:?}"
    );
    eprintln!("million-row integrity protected reader: {protected:?}");
    reader.commit().await?;
    let pinned_snapshot = snapshot(pool, namespace.id, large.id, workflow.id).await?;
    ensure!(
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, large.id, &retention).await?
            == CacheGenerationCleanupOutcome::Ineligible,
        "durable nonterminal pin did not protect expired data"
    );
    ensure!(
        snapshot(pool, namespace.id, large.id, workflow.id).await? == pinned_snapshot,
        "pin protection changed storage"
    );

    let mut terminal = pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut terminal, CacheTransactionMode::PinMutation)
        .await?;
    // Cancel the owning workflow, rather than claiming a million child actions
    // completed. The durable scanner has served only the owned reader fixture.
    WorkflowCacheIterationRepository::mark_terminal(
        &mut *terminal,
        iteration.id,
        WorkflowCacheIterationState::Cancelled,
        None,
    )
    .await?
    .context("iteration did not cancel")?;
    WorkflowExecutionRepository::update(
        &mut *terminal,
        workflow.id,
        UpdateWorkflowExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            ..Default::default()
        },
    )
    .await?;
    ExecutionRepository::update(
        &mut *terminal,
        execution.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            ..Default::default()
        },
    )
    .await?;
    terminal.commit().await?;
    let before_fault = snapshot(pool, namespace.id, large.id, workflow.id).await?;
    ensure!(
        before_fault.usage == Some((records, bytes, 1)),
        "terminal pin metadata or usage disappeared early"
    );
    install_drop_fault(
        pool,
        large.id,
        (
            before_fault.accounting.deployment_bytes - bytes,
            before_fault.accounting.owner_bytes - bytes,
        ),
    )
    .await?;
    let failure =
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, large.id, &retention).await;
    match failure {
        Err(Error::Database(error)) => {
            let database_error = error
                .as_database_error()
                .context("fault was not a PostgreSQL error")?;
            ensure!(
                database_error.code().as_deref() == Some("P0001")
                    && database_error.message() == FAULT_MESSAGE,
                "wrong reclaim failure: {database_error}"
            );
        }
        other => anyhow::bail!("post-DROP fault was not reached: {other:?}"),
    }
    ensure!(
        snapshot(pool, namespace.id, large.id, workflow.id).await? == before_fault,
        "failed DROP did not restore exact counters, head and subordinate metadata"
    );
    ensure!(
        matches!(
            CacheEntryRepository::scan_pinned_page(
                pool,
                namespace.id,
                large.id,
                None,
                PAGE_RECORDS
            )
            .await,
            Err(Error::CacheSnapshotExpired(_))
        ),
        "rollback improperly reopened expired data to a new reader"
    );
    let restored = fingerprint(
        &mut verification,
        &retired,
        expected_source_sha256,
        created_between,
    )
    .await?;
    ensure!(
        restored == original,
        "rolled-back DROP changed any row value or metadata: {restored:?}, baseline {original:?}"
    );
    eprintln!("million-row integrity restored after rollback: {restored:?}");
    verification.commit().await?;
    let mut clear_fault = pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut clear_fault, CacheTransactionMode::PinMutation)
        .await?;
    sqlx::raw_sql("DROP TRIGGER zz_fail_large_integrity_drop ON cache_generation; DROP FUNCTION fail_large_integrity_drop();")
        .execute(&mut *clear_fault).await?;
    clear_fault.commit().await?;

    ensure!(
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, large.id, &retention).await?
            == CacheGenerationCleanupOutcome::Dropped {
                records: u64::try_from(records)?,
                bytes: u64::try_from(bytes)?
            },
        "confirmed reclaim totals differ from admitted usage"
    );
    let after_drop = snapshot(pool, namespace.id, large.id, workflow.id).await?;
    ensure!(
        after_drop.physical_leaf.is_none()
            && after_drop.attached_leaf.is_none()
            && after_drop.generation.is_none()
            && after_drop.usage.is_none()
            && after_drop.iteration.is_none()
            && after_drop.chunks == 0,
        "committed DROP retained storage or subordinate metadata"
    );
    ensure!(
        after_drop.namespace == before_fault.namespace,
        "reclaim changed successor/head metadata"
    );
    let mut expected_accounting = before_fault.accounting;
    expected_accounting.deployment_bytes -= bytes;
    expected_accounting.owner_bytes -= bytes;
    expected_accounting.partitions -= 1;
    expected_accounting.partitions_dropped += 1;
    expected_accounting.requested_revision += 1;
    ensure!(
        after_drop.accounting == expected_accounting,
        "committed accounting/catalog delta differs"
    );
    let empty = CacheEntryRepository::scan_pinned_page(
        pool,
        namespace.id,
        successor.id,
        None,
        PAGE_RECORDS,
    )
    .await?;
    ensure!(
        empty.generation.id == successor.id && empty.entries.is_empty() && !empty.has_more,
        "empty successor was damaged"
    );
    ensure!(
        CacheGenerationRepository::drop_if_cleanup_eligible(pool, large.id, &retention).await?
            == CacheGenerationCleanupOutcome::Absent,
        "reclaim retry did not report absence"
    );
    ensure!(
        snapshot(pool, namespace.id, large.id, workflow.id).await? == after_drop,
        "reclaim retry released quota twice"
    );
    let source_sha256 = lowercase_hex(&original.source_sha256);
    let row_metadata_sha256 = lowercase_hex(&original.row_metadata_sha256);
    eprintln!("million-row integrity verified: rows={}, bytes={}, source_sha256={source_sha256}, row_metadata_sha256={row_metadata_sha256}", original.records, original.bytes);
    Ok(())
}

#[test]
fn lowercase_hex_preserves_digest_width_and_leading_zeroes() {
    assert_eq!(lowercase_hex(&[0x00, 0x01, 0x0f, 0x10, 0xff]), "00010f10ff");
    assert_eq!(
        lowercase_hex(Sha256::digest(b"").as_ref()),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[tokio::test]
async fn million_row_values_metadata_and_usage_survive_protection_and_reclaim_rollback() {
    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))
    .expect("load normal database test configuration");
    let db = TestDatabase::create(&config.database)
        .await
        .expect("create owned million-row fixture")
        .with_cleanup_on_drop();
    let result = AssertUnwindSafe(verify_large_generation(db.pool()))
        .catch_unwind()
        .await;
    if let Ok(Err(error)) = &result {
        eprintln!("million-row integrity failed before teardown: {error:#}");
    }
    // No spawned writers or observers. Dropped transactions queue rollback before
    // this awaited pool close and clone removal, including assertion/error paths.
    db.cleanup()
        .await
        .expect("await owned million-row fixture cleanup");
    match result {
        Ok(result) => result.expect("million-row generation integrity"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
