//! Owned actual-repository acceptance. Invoked by measure-native-maintenance.py.
//! SQL here is fixture setup or an independent oracle, never a timed read/write substitute.
use std::{fs::OpenOptions, io::Write, path::Path, sync::Arc, time::Instant};

use anyhow::{bail, ensure, Result};
use attune_common::{
    config::{
        Config, DatabaseConfig, NativeMaintenanceConfig, RetentionConfig, RetentionTargetsConfig,
    },
    db::Database,
    models::ExecutionStatus,
    repositories::{
        analytics::{AnalyticsRepository, AnalyticsTimeRange},
        event::{CreateEventInput, EventRepository},
        execution::{CreateExecutionInput, ExecutionRepository},
        native_maintenance::{
            partitions::{utc_day, PartitionRepairOutcome, PartitionRepository},
            read::{self, ReadMode, ReadRow},
            summaries::{SummaryCommitDeadlineExceeded, SummaryRepository},
            ManagedTable, SummaryKind,
        },
        retention::{RetentionRepository, RetentionTarget},
        Create,
    },
    test_database::TestDatabase,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection, PgPool};
use tokio::sync::Barrier;
use tokio_util::sync::CancellationToken;

const PAIRS: usize = 6;
const TRANSACTIONS: usize = 100;
const BATCH_ROWS: usize = 25;
const SETUP_ATTEMPTS_PER_DAY: usize = 3;
const SETUP_DEADLINE_SECONDS: u64 = 600;
const CATCHUP_DEADLINE_SECONDS: u64 = 600;
const MAX_CATCHUP_CYCLES: usize = 96;

struct Evidence(std::fs::File);
impl Evidence {
    fn record(&mut self, value: Value) -> Result<()> {
        writeln!(self.0, "{}", value)?;
        self.0.flush()?;
        Ok(())
    }
}

async fn cleanup_owned(
    db: TestDatabase,
    config: &DatabaseConfig,
    evidence: &mut Evidence,
) -> Result<()> {
    let name = db.database_name().to_string();
    let mut result = db.cleanup().await;
    // FORCE can race a newly starting autovacuum backend on PG18. This is
    // explicit owner-validated teardown recovery, never a measurement retry.
    for attempt in 1..=3 {
        match result {
            Ok(()) => return Ok(()),
            Err(error) => evidence.record(json!({"type":"cleanup_retry","database":name,"attempt":attempt,"error":error.to_string()}))?,
        }
        result = TestDatabase::cleanup_detached(config, &name).await;
    }
    result?;
    Ok(())
}

fn anchor() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap()
}

fn row_values(rows: &[ReadRow]) -> Vec<Value> {
    rows.iter().map(|r| json!({"bucket":r.bucket,"reference":r.reference,"status":r.status,"count":r.count})).collect()
}

async fn ensure_days(
    pool: &PgPool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    evidence: &mut Evidence,
) -> Result<()> {
    for table in ManagedTable::ALL {
        ensure_table_days(
            pool,
            table,
            start,
            end,
            evidence,
            Instant::now() + std::time::Duration::from_secs(SETUP_DEADLINE_SECONDS),
        )
        .await?;
    }
    Ok(())
}

async fn ensure_table_days(
    pool: &PgPool,
    table: ManagedTable,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    evidence: &mut Evidence,
    deadline: Instant,
) -> Result<()> {
    let config = NativeMaintenanceConfig::default();
    let mut day = utc_day(start);
    while day <= utc_day(end) {
        let mut prepared = false;
        for attempt in 1..=SETUP_ATTEMPTS_PER_DAY {
            if Instant::now() >= deadline {
                evidence.record(json!({"type":"partition_prepare_exhausted","parent":table,"day":day,"reason":"overall setup deadline","next_attempt":attempt,"import_started":false}))?;
                bail!("setup deadline exhausted; import refused");
            }
            let started = Instant::now();
            let outcome = PartitionRepository::ensure_day(pool, table, day, &config).await;
            match outcome {
                Ok(outcome) => {
                    prepared = matches!(
                        outcome,
                        PartitionRepairOutcome::Applied { .. }
                            | PartitionRepairOutcome::AlreadyPresent
                    );
                    let retryable = matches!(
                        outcome,
                        PartitionRepairOutcome::DeferredBusy
                            | PartitionRepairOutcome::DeferredDeadline
                    );
                    evidence.record(json!({"type":"partition_prepare","parent":table,"day":day,"attempt":attempt,"outcome":outcome,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"config":config,"rollback_awaited":retryable,"setup_reconciliation":true}))?;
                    if prepared {
                        break;
                    }
                    ensure!(
                        retryable,
                        "partition setup cannot reconcile {outcome:?}; import refused"
                    );
                }
                Err(error) => {
                    evidence.record(json!({"type":"partition_prepare_error","parent":table,"day":day,"attempt":attempt,"error":error.to_string(),"elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"rollback_awaited":true,"setup_reconciliation":true}))?;
                    return Err(error.into());
                }
            }
        }
        if !prepared {
            evidence.record(json!({"type":"partition_prepare_exhausted","parent":table,"day":day,"attempts":SETUP_ATTEMPTS_PER_DAY,"import_started":false}))?;
            bail!("partition setup exhausted three attempts for {table:?} {day}; import refused");
        }
        day += Duration::days(1);
    }
    Ok(())
}

async fn actual_read(
    conn: &mut PgConnection,
    kind: SummaryKind,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<(Vec<ReadRow>, read::ReadMetadata)> {
    let range = AnalyticsTimeRange {
        since: start,
        until: end - Duration::microseconds(1),
    };
    if kind == SummaryKind::EventVolume {
        let result = AnalyticsRepository::event_volume_hourly(&mut *conn, &range).await?;
        Ok((
            result
                .data
                .into_iter()
                .map(|r| ReadRow {
                    bucket: r.bucket,
                    reference: r.trigger_ref,
                    status: None,
                    count: r.event_count,
                })
                .collect(),
            result.metadata,
        ))
    } else {
        let result = AnalyticsRepository::execution_status_hourly(&mut *conn, &range).await?;
        Ok((
            result
                .data
                .into_iter()
                .map(|r| ReadRow {
                    bucket: r.bucket,
                    reference: r.action_ref,
                    status: r.new_status,
                    count: r.transition_count,
                })
                .collect(),
            result.metadata,
        ))
    }
}

async fn oracle(
    pool: &PgPool,
    kind: SummaryKind,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<ReadRow>> {
    let sql = if kind == SummaryKind::EventVolume {
        "SELECT date_trunc('hour', created, 'UTC') bucket, NULL::text reference, NULL::text status, count(*)::bigint count FROM event WHERE created >= $1 AND created < $2 GROUP BY 1 ORDER BY 1"
    } else {
        "SELECT date_trunc('hour', time, 'UTC') bucket, NULL::text reference, new_values->>'status' status, count(*)::bigint count FROM execution_history WHERE time >= $1 AND time < $2 AND 'status'=ANY(changed_fields) GROUP BY 1,3 ORDER BY 1,3 NULLS LAST"
    };
    Ok(sqlx::query_as(sql)
        .bind(start)
        .bind(end)
        .fetch_all(pool)
        .await?)
}

async fn reads(
    pool: &PgPool,
    evidence: &mut Evidence,
    profile: &str,
    phase: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    mode: ReadMode,
) -> Result<()> {
    for kind in [SummaryKind::EventVolume, SummaryKind::ExecutionStatus] {
        let expected = Arc::new(oracle(pool, kind, start, end).await?);
        evidence.record(json!({"type":"oracle", "profile":profile, "phase":phase, "kind":kind, "rows":row_values(&expected)}))?;
        // First-observed after import/oracle is explicitly not a cold-cache sample.
        for wave in 0..6 {
            let concurrency = if wave == 0 { 1 } else { 4 };
            let barrier = Arc::new(Barrier::new(concurrency));
            let mut tasks = Vec::new();
            for lane in 0..concurrency {
                let mut conn = pool.acquire().await?;
                let sample = if wave == 0 {
                    "first".to_string()
                } else {
                    ((wave - 1) * 4 + lane).to_string()
                };
                let tag = format!("nw_{profile}_{phase}_{kind:?}_{sample}");
                sqlx::query("SET log_min_duration_statement = 0")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("SELECT set_config('application_name',$1,false)")
                    .bind(&tag)
                    .execute(&mut *conn)
                    .await?;
                let barrier = barrier.clone();
                let expected = expected.clone();
                tasks.push(async move {
                    barrier.wait().await;
                    let clock = Instant::now();
                    let result = actual_read(&mut conn, kind, start, end).await;
                    let elapsed = clock.elapsed().as_secs_f64() * 1000.0;
                    let value = match result {
                        Ok((data, metadata)) => json!({"tag":tag,"sample":sample,"repository_ms":elapsed,"metadata":metadata,"correct":data==*expected,"count":data.iter().map(|r|r.count).sum::<i64>(),"mode_correct":metadata.mode==mode}),
                        Err(error) => json!({"tag":tag,"sample":sample,"repository_ms":elapsed,"error":error.to_string(),"correct":false}),
                    };
                    sqlx::query("SET application_name = 'native_workload_idle'").execute(&mut *conn).await?;
                    sqlx::query("SET log_min_duration_statement = -1").execute(&mut *conn).await?;
                    Ok::<_, anyhow::Error>(value)
                });
            }
            for result in futures::future::join_all(tasks).await {
                let mut value = result?;
                value["type"] = json!("read");
                value["profile"] = json!(profile);
                value["phase"] = json!(phase);
                value["kind"] = json!(kind);
                evidence.record(value)?;
            }
        }
    }
    Ok(())
}

fn reconciliation_error(error: &attune_common::Error) -> bool {
    match error {
        attune_common::Error::Timeout(_) => true,
        attune_common::Error::Database(sqlx::Error::Database(db)) => matches!(
            db.code().as_deref(),
            Some("55P03" | "57014" | "40001" | "40P01")
        ),
        attune_common::Error::Other(inner) => inner
            .downcast_ref::<SummaryCommitDeadlineExceeded>()
            .is_some(),
        _ => false,
    }
}

async fn coverage_ledger(
    pool: &PgPool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<std::collections::BTreeMap<(String, DateTime<Utc>), DateTime<Utc>>> {
    let rows: Vec<(String,DateTime<Utc>,DateTime<Utc>)> = sqlx::query_as("SELECT kind::text,bucket,refreshed_at FROM native_summary_hour WHERE bucket >= $1 AND bucket < $2 ORDER BY kind,bucket").bind(start).bind(end).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(kind, bucket, refreshed)| ((kind, bucket), refreshed))
        .collect())
}

async fn materialize(
    db: &TestDatabase,
    evidence: &mut Evidence,
    name: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<()> {
    let pool = db.pool();
    let config = NativeMaintenanceConfig::default();
    let began = Instant::now();
    let deadline = began + std::time::Duration::from_secs(CATCHUP_DEADLINE_SECONDS);
    let hours = (end - start).num_hours();
    let mut attempted = 0u64;
    let mut completed = 0u64;
    let mut first_failures = 0u64;
    let mut late_commits = 0u64;
    // One first attempt per kind/hour. Failures retain dirty state for a later
    // bounded production cycle; no timed read or ingestion sample is retried.
    for h in 0..hours {
        for kind in SummaryKind::ALL {
            ensure!(
                Instant::now() < deadline,
                "materialization overall deadline before first pass completed"
            );
            let bucket = start + Duration::hours(h);
            attempted += 1;
            match SummaryRepository::refresh_bucket_at(pool, kind, bucket, &config, end).await {
                Ok(result) => {
                    completed += 1;
                    evidence.record(json!({"type":"refresh","profile":name,"stage":"first_attempt","attempt":1,"cumulative_attempts":attempted,"result":result}))?;
                }
                Err(error) => {
                    first_failures += 1;
                    let committed = match &error {
                        attune_common::Error::Other(inner) => inner
                            .downcast_ref::<SummaryCommitDeadlineExceeded>()
                            .map(|late| &late.committed),
                        _ => None,
                    };
                    late_commits += u64::from(committed.is_some());
                    let retryable = reconciliation_error(&error);
                    evidence.record(json!({"type":"refresh_error","profile":name,"stage":"first_attempt","kind":kind,"bucket":bucket,"attempt":1,"error":error.to_string(),"reconciliation_eligible":retryable,"commit_acknowledged":committed.is_some(),"commit_outcome_unknown":!retryable,"committed_result":committed,"cumulative_attempts":attempted,"rollback_awaited":committed.is_none() && retryable}))?;
                    ensure!(retryable, "non-reconcilable materialization error: {error}");
                }
            }
        }
    }
    evidence.record(json!({"type":"materialization_first_pass","profile":name,"attempted_bucket_operations":attempted,"completed_bucket_operations":completed,"first_attempt_failures":first_failures,"committed_late_bucket_operations":late_commits,"status":SummaryRepository::status(pool).await?}))?;
    let mut targets = RetentionTargetsConfig::default();
    // Fixed logical clock includes the baseline's complete boundary hour.
    // Preserve the fixture start when cycle planning applies raw-retention bounds.
    for target in [
        &mut targets.events,
        &mut targets.execution_history,
        &mut targets.worker_history,
    ] {
        target.max_age_seconds = Some((end - start).num_seconds() as u64);
    }
    let mut cycle_failures = 0usize;
    let mut cycle_processed = 0u64;
    let mut cycles = 0usize;
    for cycle in 1..=MAX_CATCHUP_CYCLES {
        ensure!(
            Instant::now() < deadline,
            "materialization catch-up deadline exhausted"
        );
        let before = coverage_ledger(pool, start, end).await?;
        // A fresh pool models a restarted builder with no client-local progress.
        // Only persisted ledger/dirty state and the owning refresh_cycle plan choose work.
        let mut database_config = Config::load_from_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config.test.yaml"
        ))?
        .database;
        database_config.url = db.database_url().to_string();
        database_config.schema = Some(db.schema().to_string());
        let restarted = Database::new(&database_config).await?;
        let cycle_start = Instant::now();
        let result = SummaryRepository::refresh_cycle(
            restarted.pool(),
            &config,
            &targets,
            end,
            &CancellationToken::new(),
        )
        .await;
        restarted.pool().close().await;
        let result = result?;
        let after = coverage_ledger(pool, start, end).await?;
        let mut publications: Vec<Value>=after.iter().filter(|(key,time)| before.get(*key)!=Some(*time)).map(|((kind,bucket),time)|json!({"kind":kind,"bucket":bucket,"refreshed_at":time,"commit_acknowledged":true})).collect();
        publications.sort_by_key(|row| row["refreshed_at"].as_str().unwrap().to_string());
        // Every publication corresponds to one committed refresh attempt. Failed
        // attempts and planning failures are individually returned by the repository.
        ensure!(
            publications.len() as u64 == result.buckets_processed,
            "cycle publication count differs from confirmed repository progress"
        );
        cycle_failures += result.failures.len();
        cycle_processed += result.buckets_processed;
        cycles = cycle;
        let status = SummaryRepository::status(pool).await?;
        let complete = status
            .iter()
            .all(|s| s.coverage_hours == hours && s.dirty_notifications == 0);
        evidence.record(json!({"type":"materialization_cycle","profile":name,"cycle":cycle,"builder_restarted":true,"logical_now":end,"repository_result":result,"published_bucket_attempts":publications,"status":status,"complete":complete,"cycle_wall_ms":cycle_start.elapsed().as_secs_f64()*1000.0,"cumulative_cycle_commits":cycle_processed,"cumulative_cycle_failures":cycle_failures,"config":config,"retention_targets":targets}))?;
        if complete {
            break;
        }
    }
    let status = SummaryRepository::status(pool).await?;
    let complete = status
        .iter()
        .all(|s| s.coverage_hours == hours && s.dirty_notifications == 0);
    evidence.record(json!({"type":if complete {"materialization"} else {"materialization_incomplete"},"profile":name,"attempted_first_pass_bucket_operations":attempted,"completed_first_pass_bucket_operations":completed,"first_attempt_failures":first_failures,"committed_late_bucket_operations":late_commits,"catchup_cycles":cycles,"catchup_committed_bucket_operations":cycle_processed,"catchup_failures":cycle_failures,"all_first_attempts_on_budget":first_failures==0,"all_cycle_attempts_on_budget":cycle_failures==0,"elapsed_ms":began.elapsed().as_secs_f64()*1000.0,"status":status,"config":config}))?;
    ensure!(
        complete,
        "materialization failed to converge within declared catch-up bounds"
    );
    Ok(())
}

async fn profile(db: &TestDatabase, evidence: &mut Evidence, dir: &Path, days: i64) -> Result<()> {
    let pool = db.pool();
    let name = format!("d{days}");
    let start = anchor() - Duration::days(days);
    let audit_days = if days == 30 { 90 } else { days };
    let setup_deadline = Instant::now() + std::time::Duration::from_secs(SETUP_DEADLINE_SECONDS);
    for table in ManagedTable::ALL {
        let source_days = if table == ManagedTable::AuditEvent {
            audit_days
        } else {
            days
        };
        ensure_table_days(
            pool,
            table,
            anchor() - Duration::days(source_days),
            anchor() + Duration::hours(2),
            evidence,
            setup_deadline,
        )
        .await?;
        let extra_old: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM native_partition_registry WHERE parent=$1 AND lower_bound<$2",
        )
        .bind(table)
        .bind(utc_day(anchor() - Duration::days(source_days)))
        .fetch_one(pool)
        .await?;
        ensure!(
            extra_old == 0,
            "fixture has excess old partitions for {table:?}"
        );
    }
    let topology: Value = sqlx::query_scalar("SELECT jsonb_object_agg(parent,n) FROM (SELECT parent,count(*) n FROM native_partition_registry GROUP BY parent) s").fetch_one(pool).await?;
    evidence.record(json!({"type":"partition_topology","profile":name,"registered":topology}))?;
    let sql = std::fs::read_to_string(dir.join(format!("fixture-{days}.sql")))?;
    evidence.record(json!({"type":"stage","profile":name,"stage":"import_start"}))?;
    // Historical fixture import is not service ingestion. Suppress only fanout;
    // native source invalidation remains on. Ingest samples use untouched clones.
    sqlx::raw_sql("ALTER TABLE event DISABLE TRIGGER event_created_notify; ALTER TABLE enforcement DISABLE TRIGGER enforcement_created_notify;").execute(pool).await?;
    sqlx::raw_sql(&sql).execute(pool).await?;
    sqlx::raw_sql("ALTER TABLE event ENABLE TRIGGER event_created_notify; ALTER TABLE enforcement ENABLE TRIGGER enforcement_created_notify;").execute(pool).await?;
    sqlx::raw_sql("SELECT setval(pg_get_serial_sequence('event','id'),(SELECT max(id) FROM event)); SELECT setval(pg_get_serial_sequence('execution','id'),(SELECT max(id) FROM execution)); ANALYZE;").execute(pool).await?;
    let counts: Value = sqlx::query_scalar("SELECT jsonb_build_object('event',(SELECT count(*) FROM event),'execution_history',(SELECT count(*) FROM execution_history),'execution',(SELECT count(*) FROM execution),'audit_event',(SELECT count(*) FROM audit_event),'enforcement',(SELECT count(*) FROM enforcement),'worker_history',(SELECT count(*) FROM worker_history),'default_event',(SELECT count(*) FROM ONLY event_default),'default_history',(SELECT count(*) FROM ONLY execution_history_default),'default_audit',(SELECT count(*) FROM ONLY audit_event_default))").fetch_one(pool).await?;
    ensure!(
        counts["default_event"] == 0
            && counts["default_history"] == 0
            && counts["default_audit"] == 0,
        "DEFAULT backlog invalidates fixture: {counts}"
    );
    evidence.record(
        json!({"type":"fixture","profile":name,"counts":counts,"database":db.database_name()}),
    )?;
    let query_start = anchor() - Duration::hours(24);
    let query_end = anchor() + Duration::hours(1);
    reads(
        pool,
        evidence,
        &name,
        "raw",
        query_start,
        query_end,
        ReadMode::RawOnly,
    )
    .await?;
    materialize(db, evidence, &name, start, query_end).await?;
    let status = SummaryRepository::status(pool).await?;
    // Include the fixture's boundary hour, even for kinds whose source hour is empty.
    ensure!(
        status
            .iter()
            .all(|s| s.coverage_hours == days * 24 + 1 && s.dirty_notifications == 0),
        "incomplete coverage {status:?}"
    );
    for kind in SummaryKind::ALL {
        let (reference, status, predicate) = match kind {
            SummaryKind::EventVolume => ("trigger_ref", "NULL::text", "TRUE"),
            SummaryKind::ExecutionStatus | SummaryKind::WorkerStatus => (
                "entity_ref",
                "new_values->>'status'",
                "'status'=ANY(changed_fields)",
            ),
            SummaryKind::ExecutionCreation => ("entity_ref", "NULL::text", "operation='INSERT'"),
        };
        let sql = format!("SELECT date_trunc('hour',{},'UTC') bucket, {reference} reference, {status} status, count(*)::bigint count FROM {} WHERE {} >= $1 AND {} < $2 AND ({predicate}) GROUP BY 1,2,3 ORDER BY 1,2 NULLS LAST,3 NULLS LAST",kind.time_column(),kind.source_table(),kind.time_column(),kind.time_column());
        let raw: Vec<ReadRow> = sqlx::query_as(&sql)
            .bind(start)
            .bind(query_end)
            .fetch_all(pool)
            .await?;
        let actual = read::read(pool, kind, start, query_end, None, None, true).await?;
        ensure!(
            actual.metadata.mode == ReadMode::SummaryOnly && actual.data == raw,
            "full-window oracle mismatch for {kind:?}"
        );
        evidence.record(json!({"type":"full_oracle","profile":name,"kind":kind,"raw":row_values(&raw),"actual":row_values(&actual.data),"metadata":actual.metadata}))?;
    }
    reads(
        pool,
        evidence,
        &name,
        "summary",
        query_start,
        query_end,
        ReadMode::SummaryOnly,
    )
    .await?;
    // Replay a complete source hour at normal fixture density into the uncovered
    // tail. This is setup, not a timed INSERT substitute for the writer gate.
    let events = sqlx::query("INSERT INTO event(created,trigger_ref,payload) SELECT created + interval '2 hours',trigger_ref,payload FROM event WHERE created >= $1 AND created < $2").bind(anchor()-Duration::hours(1)).bind(anchor()).execute(pool).await?.rows_affected();
    let history = sqlx::query("INSERT INTO execution_history(time,operation,entity_id,entity_ref,changed_fields,old_values,new_values) SELECT time + interval '2 hours',operation,entity_id + $3,entity_ref,changed_fields,old_values,new_values FROM execution_history WHERE time >= $1 AND time < $2").bind(anchor()-Duration::hours(1)).bind(anchor()).bind(days*20_000).execute(pool).await?.rows_affected();
    ensure!(
        events >= 1666 && history >= 2666,
        "tail below declared normal arrival density"
    );
    evidence.record(json!({"type":"recent_tail_fixture","profile":name,"events":events,"history":history,"start":query_end,"end":query_end+Duration::hours(1)}))?;
    reads(
        pool,
        evidence,
        &name,
        "mixed",
        query_start,
        query_end + Duration::hours(1),
        ReadMode::SummaryPlusRaw,
    )
    .await?;
    // Nullable/filter correctness uses the real planner, outside the timed fixture.
    for kind in [SummaryKind::EventVolume, SummaryKind::ExecutionStatus] {
        let refs = vec![if kind == SummaryKind::EventVolume {
            "evidence.trigger_1"
        } else {
            "evidence.action_1"
        }
        .to_string()];
        let filtered = read::read(pool, kind, start, anchor(), Some(&refs), None, true).await?;
        ensure!(
            filtered.metadata.mode == ReadMode::SummaryOnly,
            "filtered read not summary-only"
        );
        evidence.record(json!({"type":"filtered","profile":name,"kind":kind,"metadata":filtered.metadata,"rows":row_values(&filtered.data)}))?;
    }
    Ok(())
}

async fn expiry(
    db: &TestDatabase,
    evidence: &mut Evidence,
    rows: i64,
    drop_partition: bool,
) -> Result<()> {
    let pool = db.pool();
    let day = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    ensure_days(pool, day, day + Duration::days(1), evidence).await?;
    sqlx::query("ALTER TABLE event DISABLE TRIGGER event_created_notify")
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO event(created,trigger_ref,payload) SELECT $1,'expiry.fixture',jsonb_build_object('id',g,'records',repeat(md5(g::text),32)) FROM generate_series(1,$2::bigint) g").bind(day+Duration::hours(12)).bind(rows).execute(pool).await?;
    sqlx::query(
        "INSERT INTO event(created,trigger_ref,payload) VALUES ($1,'expiry.retained','{}')",
    )
    .bind(day + Duration::days(1) + Duration::hours(12))
    .execute(pool)
    .await?;
    sqlx::query("ALTER TABLE event ENABLE TRIGGER event_created_notify")
        .execute(pool)
        .await?;
    sqlx::raw_sql("ANALYZE; CHECKPOINT;").execute(pool).await?;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM event")
        .fetch_one(pool)
        .await?;
    if drop_partition {
        let mut blocker = pool.begin().await?;
        sqlx::query("LOCK TABLE ONLY event IN ACCESS SHARE MODE")
            .execute(&mut *blocker)
            .await?;
        let waiting = Instant::now();
        let rejected = PartitionRepository::expire_before(
            pool,
            ManagedTable::Event,
            day + Duration::days(1),
            &NativeMaintenanceConfig::default(),
            false,
            || false,
        )
        .await;
        let wait_ms = waiting.elapsed().as_secs_f64() * 1000.0;
        blocker.rollback().await?;
        let error = rejected.expect_err("blocked expiry must fail");
        evidence.record(json!({"type":"expiry_lock_wait","rows":rows,"configured_limit_ms":250,"repository_ms":wait_ms,"error":error.to_string(),"confirmed_partitions":error.partitions_dropped}))?;
        ensure!(
            error.partitions_dropped == 0,
            "blocked expiry committed a drop"
        );
    }
    let wal: String = sqlx::query_scalar("SELECT pg_current_wal_insert_lsn()::text")
        .fetch_one(pool)
        .await?;
    let mut config = RetentionConfig::default();
    config.native_maintenance.enabled = drop_partition;
    let start = Instant::now();
    let mut cycles = Vec::new();
    let age = (Utc::now() - (day + Duration::days(1))).num_seconds() as u64;
    for _ in 0..(rows / 100_000 + 2) {
        let result = RetentionRepository::run_target_bounded(
            pool,
            RetentionTarget::Events,
            age,
            &config,
            || false,
        )
        .await;
        match result {
            Ok(r) => {
                let done = r.deleted == 0 && r.partitions_dropped == 0;
                cycles.push(json!({"rows_deleted":r.deleted,"partitions_dropped":r.partitions_dropped,"cutoff":r.cutoff}));
                if done {
                    break;
                }
            }
            Err(e) => {
                evidence.record(json!({"type":"expiry_error","rows":rows,"partition":drop_partition,"error":e.to_string(),"confirmed_rows":e.deleted,"confirmed_partitions":e.partitions_dropped}))?;
                return Err(e.into());
            }
        }
    }
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    let wal_bytes: i64 = sqlx::query_scalar(
        "SELECT pg_wal_lsn_diff(pg_current_wal_insert_lsn(),$1::pg_lsn)::bigint",
    )
    .bind(wal)
    .fetch_one(pool)
    .await?;
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM event")
        .fetch_one(pool)
        .await?;
    ensure!(
        before == rows + 1 && after == 1,
        "expiry count mismatch before={before}, after={after}"
    );
    evidence.record(json!({"type":"expiry","rows":rows,"partition":drop_partition,"repository_ms":elapsed,"parent_lock_hold_upper_bound_ms":if drop_partition {Some(elapsed)} else {None},"wal_bytes":wal_bytes,"actual_rows_before":before,"actual_rows_after":after,"actual_rows_removed":before-after,"cycles":cycles,"config":config}))?;
    Ok(())
}

async fn protocol(db: &TestDatabase, evidence: &mut Evidence) -> Result<()> {
    let pool = db.pool();
    let day = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    ensure_days(pool, day, day, evidence).await?;
    let mut blocker = pool.begin().await?;
    sqlx::query("LOCK TABLE ONLY event IN ACCESS SHARE MODE")
        .execute(&mut *blocker)
        .await?;
    let rejected = ensure_table_days(
        pool,
        ManagedTable::Event,
        day + Duration::days(1),
        day + Duration::days(1),
        evidence,
        Instant::now() + std::time::Duration::from_secs(SETUP_DEADLINE_SECONDS),
    )
    .await;
    blocker.rollback().await?;
    ensure!(
        rejected.is_err(),
        "fixture preparation must reject a deferred day"
    );
    evidence.record(json!({"type":"fixture_guard_proof","expected_rejection":true,"error":rejected.unwrap_err().to_string(),"import_started":false}))?;
    let mut conn = pool.acquire().await?;
    sqlx::raw_sql(
        "SET application_name='native_workload_protocol'; SET log_min_duration_statement=0;",
    )
    .execute(&mut *conn)
    .await?;
    let catalog: Value = sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_object('source',c.relname,'name',t.tgname,'enabled',t.tgenabled,'statement_level',(t.tgtype & 1)=0,'new_transition',t.tgnewtable,'old_transition',t.tgoldtable) ORDER BY c.relname,t.tgname) FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid WHERE t.tgname LIKE 'native_summary_%' AND NOT t.tgisinternal").fetch_one(&mut *conn).await?;
    let fks: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_constraint WHERE conrelid='native_summary_invalidation'::regclass AND contype='f'").fetch_one(&mut *conn).await?;
    ensure!(fks == 0, "invalidation log has a state-locking foreign key");
    sqlx::query("INSERT INTO event(created,trigger_ref,payload) VALUES ($1,'protocol.a','{}'),($1,'protocol.b','{}'),($1 + interval '1 hour','protocol.a','{}')").bind(day).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO execution_history(time,operation,entity_id,entity_ref,changed_fields,new_values) VALUES ($1,'INSERT',1,NULL,ARRAY['status'],'{\"status\":null}'),($1,'INSERT',2,'',ARRAY['status'],'{\"status\":\"completed\"}'),($1,'UPDATE',3,'protocol.a',ARRAY['status'],'{\"status\":\"failed\"}')").bind(day).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO worker_history(time,operation,entity_id,entity_ref,changed_fields,new_values) VALUES ($1,'UPDATE',1,'protocol.a',ARRAY['status'],'{\"status\":\"active\"}'),($1,'UPDATE',2,'protocol.b',ARRAY['status'],'{\"status\":\"active\"}'),($1,'UPDATE',3,'protocol.c',ARRAY['status'],'{\"status\":\"inactive\"}')").bind(day).execute(&mut *conn).await?;
    let pending: Vec<(SummaryKind,DateTime<Utc>,i64)> = sqlx::query_as("SELECT kind,bucket,count(*) FROM native_summary_invalidation GROUP BY kind,bucket ORDER BY kind,bucket").fetch_all(&mut *conn).await?;
    sqlx::raw_sql(
        "SET log_min_duration_statement=-1; SET application_name='native_workload_idle';",
    )
    .execute(&mut *conn)
    .await?;
    drop(conn);
    ensure!(
        pending.len() == 5 && pending.iter().all(|r| r.2 == 1),
        "statement dedup mismatch: {pending:?}"
    );
    for kind in SummaryKind::ALL {
        for hour in 0..2 {
            if kind == SummaryKind::WorkerStatus && hour == 1 {
                continue;
            }
            SummaryRepository::refresh_bucket_at(
                pool,
                kind,
                day + Duration::hours(hour),
                &NativeMaintenanceConfig::default(),
                day + Duration::days(1),
            )
            .await?;
        }
    }
    let before_resume = SummaryRepository::status(pool).await?;
    let mut resume_targets = RetentionTargetsConfig::default();
    for target in [
        &mut resume_targets.events,
        &mut resume_targets.execution_history,
        &mut resume_targets.worker_history,
    ] {
        target.max_age_seconds = Some(7200);
    }
    let mut resume_config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?
    .database;
    resume_config.url = db.database_url().to_string();
    resume_config.schema = Some(db.schema().to_string());
    let resume_database = Database::new(&resume_config).await?;
    let resume_result = SummaryRepository::refresh_cycle(
        resume_database.pool(),
        &NativeMaintenanceConfig::default(),
        &resume_targets,
        day + Duration::hours(2),
        &CancellationToken::new(),
    )
    .await;
    resume_database.pool().close().await;
    let resume_result = resume_result?;
    let after_resume = SummaryRepository::status(pool).await?;
    evidence.record(json!({"type":"restart_catchup_proof","before":before_resume,"repository_result":resume_result,"after":after_resume,"new_builder_pool":true,"empty_worker_hour":day+Duration::hours(1)}))?;
    ensure!(
        resume_result.buckets_processed == 1
            && resume_result.failures.is_empty()
            && after_resume
                .iter()
                .all(|s| s.coverage_hours == 2 && s.dirty_notifications == 0),
        "restarted builder did not finish the persisted empty-hour gap"
    );
    let nullable = read::read(
        pool,
        SummaryKind::ExecutionStatus,
        day,
        day + Duration::hours(1),
        None,
        None,
        true,
    )
    .await?;
    ensure!(
        nullable.metadata.mode == ReadMode::SummaryOnly
            && nullable
                .data
                .iter()
                .any(|r| r.reference.is_none() && r.status.is_none() && r.count == 1)
            && nullable
                .data
                .iter()
                .any(|r| r.reference.as_deref() == Some("") && r.count == 1),
        "nullable dimensions collapsed"
    );
    evidence.record(json!({"type":"production_protocol","trigger_catalog":catalog,"invalidation_foreign_keys":fks,"pending_before_refresh":pending,"nullable_rows":row_values(&nullable.data)}))?;
    // A real uncommitted repository writer holds RowExclusiveLock on the parent.
    let mut writer = pool.begin().await?;
    EventRepository::create(
        &mut *writer,
        CreateEventInput {
            trigger: None,
            trigger_ref: "protocol.writer".to_string(),
            config: None,
            payload: Some(json!({"writer":true})),
            trace_tag: Some("protocol.writer".to_string()),
            source: None,
            source_ref: None,
            rule: None,
            rule_ref: None,
        },
    )
    .await?;
    let started = Instant::now();
    let expired = PartitionRepository::expire_before(
        pool,
        ManagedTable::Event,
        day + Duration::days(1),
        &NativeMaintenanceConfig::default(),
        false,
        || false,
    )
    .await;
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    writer.rollback().await?;
    let rejected = expired.expect_err("an uncommitted parent writer must block expiry");
    ensure!(
        rejected.partitions_dropped == 0,
        "writer-blocked expiry committed a drop"
    );
    evidence.record(json!({"type":"writer_expiry_lock_wait","configured_limit_ms":250,"repository_ms":elapsed,"confirmed_partitions":rejected.partitions_dropped,"error":rejected.to_string()}))?;
    Ok(())
}

async fn ingest(
    db: &TestDatabase,
    evidence: &mut Evidence,
    execution: bool,
    batch: usize,
    pair: usize,
    tracking: bool,
) -> Result<()> {
    let pool = db.pool();
    // Repository writes use the wall clock, unlike deterministic historical imports.
    ensure_days(pool, Utc::now(), Utc::now() + Duration::days(1), evidence).await?;
    if !tracking {
        for table in ["event", "execution_history", "worker_history"] {
            sqlx::query(&format!(
                "ALTER TABLE {table} DISABLE TRIGGER native_summary_insert"
            ))
            .execute(pool)
            .await?;
        }
    }
    let payloads: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('request_id',g,'host','node-'||(g%100)::text,'records',(SELECT string_agg(md5(g::text||':'||b::text),'') FROM generate_series(1,$2::integer) b),'labels',jsonb_build_object('region','test-region','attempt',g%3)) FROM generate_series(1,$1::bigint) g").bind((batch*TRANSACTIONS*4) as i64).bind(if execution {64i32} else {32i32}).fetch_all(pool).await?;
    let inputs = Arc::new(payloads);
    let mut tasks = Vec::new();
    let barrier = Arc::new(Barrier::new(5));
    let mut connections = Vec::new();
    for _ in 0..4 {
        connections.push(pool.acquire().await?);
    }
    for (writer, mut conn) in connections.into_iter().enumerate() {
        let barrier = barrier.clone();
        let inputs = inputs.clone();
        tasks.push(tokio::spawn(async move {
            let mut samples=Vec::new(); let mut errors=Vec::new(); barrier.wait().await;
            for txid in 0..TRANSACTIONS {
                let started=Instant::now();
                let outcome = async {
                let mut tx=conn.begin().await?;
                for row in 0..batch {
                    let i=(writer*TRANSACTIONS+txid)*batch+row;
                    let written = if execution {
                        ExecutionRepository::create(&mut *tx,CreateExecutionInput {action_ref:format!("evidence.action_{}",i%8),config:Some(json!({"host":format!("node-{}",i%100),"limit":100,"tags":["synthetic","test"]})),result:Some(inputs[i].clone()),status:ExecutionStatus::Completed,trace_tag:Some(format!("nw.write.{i}")),..Default::default()}).await.map(|_|())
                    } else {
                        EventRepository::create(&mut *tx,CreateEventInput {trigger:None,trigger_ref:format!("evidence.trigger_{}",i%8),config:None,payload:Some(inputs[i].clone()),trace_tag:Some(format!("nw.write.{i}")),source:None,source_ref:None,rule:None,rule_ref:None}).await.map(|_|())
                    };
                    if let Err(error) = written {tx.rollback().await?; return Err(anyhow::Error::from(error));}
                }
                tx.commit().await?;
                Ok::<_,anyhow::Error>(())
                }.await;
                samples.push(started.elapsed().as_secs_f64()*1000.0);
                if let Err(error) = outcome {errors.push(format!("writer {writer}, transaction {txid}: {error}")); break;}
            }
            (samples,errors)
        }));
    }
    let start = Instant::now();
    barrier.wait().await;
    let mut all = Vec::new();
    let mut errors = Vec::new();
    // Join every writer even if one fails. No background writer survives cleanup.
    for task in tasks {
        match task.await {
            Ok((samples, failed)) => {
                all.extend(samples);
                errors.extend(failed);
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    let source = if execution { "execution" } else { "event" };
    let actual: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {source}"))
        .fetch_one(pool)
        .await?;
    let notifications: Value=sqlx::query_scalar("SELECT coalesce(jsonb_object_agg(kind,n),'{}') FROM (SELECT kind,count(*) n FROM native_summary_invalidation GROUP BY kind) s").fetch_one(pool).await?;
    let sequence: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM native_summary_invalidation_id_seq")
            .fetch_one(pool)
            .await?;
    let expected_notifications = if !tracking {
        json!({})
    } else if execution {
        json!({"execution_creation": TRANSACTIONS*4})
    } else {
        json!({"event_volume": TRANSACTIONS*4})
    };
    let marker_checks = notifications == expected_notifications
        && if tracking {
            sequence == ((TRANSACTIONS * 4) as i64, true)
        } else {
            sequence == (1, false)
        };
    let mut ordered = all.clone();
    ordered.sort_by(f64::total_cmp);
    let p95 = ordered
        .get((ordered.len() * 95).div_ceil(100).saturating_sub(1))
        .copied();
    evidence.record(json!({"type":"ingest","source":source,"batch_rows":batch,"pair":pair,"tracking":tracking,"transactions":TRANSACTIONS*4,"expected_rows":batch*TRANSACTIONS*4,"actual_rows":actual,"wall_ms":elapsed,"rows_per_second":actual as f64*1000.0/elapsed,"transaction_p95_ms":p95,"transaction_samples_ms":all,"errors":errors,"notifications":notifications,"invalidation_sequence":sequence,"marker_checks":marker_checks}))?;
    ensure!(
        errors.is_empty() && actual == (batch * TRANSACTIONS * 4) as i64 && marker_checks,
        "ingest failed"
    );
    Ok(())
}

// Separate diagnostic scope. Its instrumented timings never enter acceptance pairs.
async fn ingest_diagnostic(
    db: &TestDatabase,
    evidence: &mut Evidence,
    execution: bool,
    tracking: bool,
) -> Result<()> {
    let pool = db.pool();
    ensure_days(pool, Utc::now(), Utc::now(), evidence).await?;
    if !tracking {
        for table in ["event", "execution_history"] {
            sqlx::query(&format!(
                "ALTER TABLE {table} DISABLE TRIGGER native_summary_insert"
            ))
            .execute(pool)
            .await?;
        }
    }
    let payloads: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('request_id',g,'host','node-'||(g%100)::text,'records',(SELECT string_agg(md5(g::text||':'||b::text),'') FROM generate_series(1,$1::integer) b),'labels',jsonb_build_object('region','test-region','attempt',g%3)) FROM generate_series(1,32) g").bind(if execution {64i32} else {32i32}).fetch_all(pool).await?;
    let mut conn = pool.acquire().await?;
    let source = if execution { "execution" } else { "event" };
    let tag = format!("nd_{source}_{tracking}");
    sqlx::query("SELECT set_config('application_name',$1,false)")
        .bind(&tag)
        .execute(&mut *conn)
        .await?;
    sqlx::raw_sql("SET track_functions='pl'; SET log_min_duration_statement=0")
        .execute(&mut *conn)
        .await?;
    let auto_explain = sqlx::raw_sql("LOAD 'auto_explain'; SET auto_explain.log_min_duration=0; SET auto_explain.log_nested_statements=on; SET auto_explain.log_analyze=on; SET auto_explain.log_timing=off; SET auto_explain.log_buffers=on; SET auto_explain.log_format='json'").execute(&mut *conn).await;
    let plan_logging = auto_explain.is_ok();
    if let Err(error) = auto_explain {
        evidence.record(
            json!({"type":"diagnostic_plan_logging_unavailable","error":error.to_string()}),
        )?;
    }
    let mut tx = conn.begin().await?;
    let mut statement_samples = Vec::new();
    for (i, payload) in payloads.into_iter().enumerate() {
        let started = Instant::now();
        let result = if execution {
            ExecutionRepository::create(&mut *tx,CreateExecutionInput {action_ref:format!("evidence.action_{}",i%8),config:Some(json!({"host":format!("node-{}",i%100),"limit":100,"tags":["synthetic","test"]})),result:Some(payload),status:ExecutionStatus::Completed,trace_tag:Some(format!("nd.write.{i}")),..Default::default()}).await.map(|_|())
        } else {
            EventRepository::create(
                &mut *tx,
                CreateEventInput {
                    trigger: None,
                    trigger_ref: format!("evidence.trigger_{}", i % 8),
                    config: None,
                    payload: Some(payload),
                    trace_tag: Some(format!("nd.write.{i}")),
                    source: None,
                    source_ref: None,
                    rule: None,
                    rule_ref: None,
                },
            )
            .await
            .map(|_| ())
        };
        statement_samples.push(started.elapsed().as_secs_f64() * 1000.0);
        if let Err(error) = result {
            tx.rollback().await?;
            return Err(error.into());
        }
    }
    let commit = Instant::now();
    tx.commit().await?;
    let commit_ms = commit.elapsed().as_secs_f64() * 1000.0;
    sqlx::query("SELECT pg_stat_force_next_flush()")
        .execute(&mut *conn)
        .await?;
    // Close this backend to publish cumulative function statistics before reading them.
    conn.close().await?;
    let stats: Value = sqlx::query_scalar("SELECT coalesce(jsonb_agg(jsonb_build_object('function',funcname,'calls',calls,'total_ms',total_time,'self_ms',self_time) ORDER BY funcname),'[]') FROM pg_stat_user_functions WHERE schemaname=current_schema()").fetch_one(pool).await?;
    let markers: i64 = sqlx::query_scalar("SELECT count(*) FROM native_summary_invalidation")
        .fetch_one(pool)
        .await?;
    let sequence: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM native_summary_invalidation_id_seq")
            .fetch_one(pool)
            .await?;
    let correct =
        markers == i64::from(tracking) && sequence == if tracking { (1, true) } else { (1, false) };
    evidence.record(json!({"type":"ingest_diagnostic","source":source,"tracking":tracking,"tag":tag,"rows":32,"statement_samples_ms":statement_samples,"commit_ms":commit_ms,"function_stats":stats,"markers":markers,"sequence":sequence,"correct":correct,"auto_explain":plan_logging,"acceptance_sample":false}))?;
    ensure!(correct, "diagnostic marker/sequence mismatch");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let dir = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("expected evidence directory"))?;
    let dir = Path::new(&dir);
    let scope = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "full".to_string());
    ensure!(
        [
            "full",
            "reads-only",
            "ingest-only",
            "ingest-diagnostic",
            "acceptance",
            "protocol-only"
        ]
        .contains(&scope.as_str()),
        "unknown scope {scope}"
    );
    let profiles = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "7,30".to_string())
        .split(',')
        .map(str::parse::<i64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let distinct = profiles.iter().collect::<std::collections::BTreeSet<_>>();
    ensure!(
        !profiles.is_empty()
            && profiles.iter().all(|d| [7, 30].contains(d))
            && distinct.len() == profiles.len(),
        "expected distinct profiles 7, 30, or both"
    );
    let mut evidence = Evidence(
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(dir.join("repository.jsonl"))?,
    );
    evidence.record(json!({"type":"contract","scope":scope,"profiles":profiles,"recent_tail":"replay last full source hour at normal density","warm_samples":20,"read_concurrency":4,"pairs":PAIRS,"writers":4,"transactions_per_writer":TRANSACTIONS,"batch_rows":[1,BATCH_ROWS],"aggregation":"nearest-rank p95; every paired wall-time and transaction-p95 ratio <=1.10; no performance sample retries or exclusions","setup_max_attempts_per_day":SETUP_ATTEMPTS_PER_DAY,"setup_overall_seconds":SETUP_DEADLINE_SECONDS,"materialization_overall_seconds":CATCHUP_DEADLINE_SECONDS,"max_catchup_cycles":MAX_CATCHUP_CYCLES,"fresh_builder_pool_each_cycle":true,"native_config":NativeMaintenanceConfig::default()}))?;
    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    let mut failures = Vec::new();
    let db = TestDatabase::create(&config.database).await?;
    let result = protocol(&db, &mut evidence).await;
    cleanup_owned(db, &config.database, &mut evidence).await?;
    if let Err(e) = result {
        evidence.record(json!({"type":"failure","phase":"protocol","error":e.to_string()}))?;
        failures.push(e.to_string());
    }
    if scope == "protocol-only" {
        ensure!(failures.is_empty(), "protocol failure: {failures:?}");
        evidence.record(json!({"type":"finished","failures":failures,"scope":"protocol_only"}))?;
        return Ok(());
    }
    if scope == "ingest-diagnostic" {
        for execution in [false, true] {
            for tracking in [false, true] {
                let db = TestDatabase::create(&config.database).await?;
                let result = ingest_diagnostic(&db, &mut evidence, execution, tracking).await;
                if let Err(error) = result {
                    evidence.record(json!({"type":"failure","phase":"ingest_diagnostic","error":error.to_string()}))?;
                    failures.push(error.to_string());
                }
                cleanup_owned(db, &config.database, &mut evidence).await?;
            }
        }
        evidence.record(json!({"type":"finished","scope":scope,"failures":failures}))?;
        ensure!(failures.is_empty(), "diagnostic failures: {failures:?}");
        return Ok(());
    }
    if scope != "ingest-only" {
        for days in profiles {
            let db = TestDatabase::create(&config.database).await?;
            let result = profile(&db, &mut evidence, dir, days).await;
            if let Err(e) = result {
                evidence.record(
                    json!({"type":"failure","phase":"profile","days":days,"error":e.to_string()}),
                )?;
                failures.push(e.to_string());
            }
            cleanup_owned(db, &config.database, &mut evidence).await?;
        }
    }
    if scope == "reads-only" {
        evidence.record(json!({"type":"finished","failures":failures,"scope":"reads_only"}))?;
        ensure!(failures.is_empty(), "read workload failure: {failures:?}");
        return Ok(());
    }
    if scope == "full" {
        for rows in [10_000, 1_000_000] {
            for partition in [true, false] {
                let db = TestDatabase::create(&config.database).await?;
                let result = expiry(&db, &mut evidence, rows, partition).await;
                cleanup_owned(db, &config.database, &mut evidence).await?;
                if let Err(e) = result {
                    failures.push(e.to_string());
                }
            }
        }
    }
    for execution in [false, true] {
        for batch in [1, BATCH_ROWS] {
            for pair in 0..PAIRS {
                for tracking in if pair % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let db = TestDatabase::create(&config.database).await?;
                    let result = ingest(&db, &mut evidence, execution, batch, pair, tracking).await;
                    cleanup_owned(db, &config.database, &mut evidence).await?;
                    if let Err(e) = result {
                        failures.push(e.to_string());
                    }
                }
            }
        }
    }
    evidence.record(json!({"type":"finished","failures":failures}))?;
    if !failures.is_empty() {
        bail!("workload correctness/setup failures: {failures:?}");
    }
    Ok(())
}
