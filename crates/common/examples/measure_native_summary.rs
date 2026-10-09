//! Actual repository materialization benchmark on a TestDatabase-owned clone.
//! Run with an owned PG16/18 ATTUNE__DATABASE__URL and unique ATTUNE_TEST_RUN_ID.
//! Arguments: rows per hour, completed hours. This does not measure the writer gate.

use std::time::Instant;

use attune_common::{
    config::{Config, NativeMaintenanceConfig, RetentionTargetsConfig},
    repositories::native_maintenance::{
        partitions::PartitionRepository, summaries::SummaryRepository, ManagedTable, SummaryKind,
    },
    test_database::TestDatabase,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use tokio_util::sync::CancellationToken;

type SummaryCountRow = (DateTime<Utc>, Option<String>, Option<String>, i64);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--dense-days") {
        return dense_profile(&args[1..]).await;
    }
    let rows: i64 = args.first().map(|s| s.parse()).transpose()?.unwrap_or(5000);
    let hours: i64 = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(24);
    if rows <= 0 || hours <= 0 || hours > 720 || rows.checked_mul(hours).is_none() {
        return Err("expected positive rows/hour and 1..720 completed hours".into());
    }
    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    let db = TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop();
    let pool = db.pool();
    let end = Utc.with_ymd_and_hms(2026, 1, 31, 0, 0, 0).unwrap();
    let start = end - Duration::hours(hours);
    // Identical half-open source bounds for the raw oracle and repository refresh.
    sqlx::query(
        "INSERT INTO event (created, trigger_ref, payload)
        SELECT $1::timestamptz + h * interval '1 hour' + interval '30 minutes',
            'fixture.' || (r % 8)::text, jsonb_build_object('record', r)
        FROM generate_series(0, $2::bigint - 1) h CROSS JOIN generate_series(1, $3::bigint) r",
    )
    .bind(start)
    .bind(hours)
    .bind(rows)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, new_values)
        SELECT $1::timestamptz + h * interval '1 hour' + interval '30 minutes',
            CASE WHEN r % 4 = 0 THEN 'INSERT' ELSE 'UPDATE' END, r,
            CASE WHEN r % 8 = 0 THEN NULL ELSE 'fixture.' || (r % 8)::text END,
            ARRAY['status'], jsonb_build_object('status', CASE WHEN r % 2 = 0 THEN 'completed' ELSE 'failed' END)
        FROM generate_series(0, $2::bigint - 1) h CROSS JOIN generate_series(1, $3::bigint) r")
        .bind(start).bind(hours).bind(rows).execute(pool).await?;
    let maintenance = NativeMaintenanceConfig {
        summary_bootstrap_hours: hours,
        ..Default::default()
    };
    let mut targets = RetentionTargetsConfig::default();
    for target in [
        &mut targets.events,
        &mut targets.execution_history,
        &mut targets.worker_history,
    ] {
        target.max_age_seconds = Some((hours * 3600) as u64);
    }
    let cancellation = CancellationToken::new();
    let started = Instant::now();
    let mut cycles = Vec::new();
    // Keep default bounded cycles. Do not disguise a costly fixture by raising limits.
    for _ in 0..(hours * 4 + 1) {
        let result =
            SummaryRepository::refresh_cycle(pool, &maintenance, &targets, end, &cancellation)
                .await?;
        if !result.failures.is_empty() {
            return Err(format!("refresh failures: {:?}", result.failures).into());
        }
        let complete = result.buckets_processed == 0;
        cycles.push(result);
        if complete {
            break;
        }
    }
    let elapsed = started.elapsed();
    let oracle: (i64, i64, i64) = sqlx::query_as("SELECT
        (SELECT count(*)::bigint FROM event WHERE created >= $1 AND created < $2),
        (SELECT count(*)::bigint FROM execution_history WHERE time >= $1 AND time < $2 AND 'status' = ANY(changed_fields)),
        (SELECT count(*)::bigint FROM execution_history WHERE time >= $1 AND time < $2 AND operation = 'INSERT')")
        .bind(start).bind(end).fetch_one(pool).await?;
    let materialized: (i64, i64, i64) = sqlx::query_as("SELECT
        (SELECT coalesce(sum(event_count), 0)::bigint FROM event_volume_hourly_summary WHERE bucket >= $1 AND bucket < $2),
        (SELECT coalesce(sum(transition_count), 0)::bigint FROM execution_status_hourly_summary WHERE bucket >= $1 AND bucket < $2),
        (SELECT coalesce(sum(execution_count), 0)::bigint FROM execution_creation_hourly_summary WHERE bucket >= $1 AND bucket < $2)")
        .bind(start).bind(end).fetch_one(pool).await?;
    if oracle != materialized {
        return Err(format!("raw {oracle:?} differs from summary {materialized:?}").into());
    }
    let status = SummaryRepository::status(pool).await?;
    if status
        .iter()
        .any(|s| s.coverage_hours != hours || s.dirty_notifications != 0)
    {
        return Err(format!("incomplete materialization: {status:?}").into());
    }
    if cycles
        .last()
        .is_none_or(|cycle| cycle.buckets_processed != 0)
    {
        return Err("did not reach a no-recomputation steady-state cycle".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "rows_per_hour": rows, "hours": hours, "raw_oracle": oracle,
            "materialized": materialized, "elapsed_milliseconds": elapsed.as_millis(),
            "cycles": cycles, "status": status, "writer_10_percent_gate": "not measured"
        }))?
    );
    db.cleanup().await?;
    Ok(())
}

/// Diagnostic source-density profile. Optional fixture path imports the unchanged
/// full acceptance fixture instead of the explicitly labeled thin source fixture.
async fn dense_profile(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let days: i64 = args.first().ok_or("--dense-days needs days")?.parse()?;
    if !(1..=30).contains(&days) {
        return Err("dense days must be 1..30".into());
    }
    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    let db = TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop();
    let result = dense_profile_in(&db, days, args.get(1)).await;
    db.cleanup().await?;
    result
}

async fn dense_profile_in(
    db: &TestDatabase,
    days: i64,
    fixture: Option<&String>,
) -> Result<(), Box<dyn std::error::Error>> {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr;
    let end = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let start = end - Duration::days(days);
    let config = NativeMaintenanceConfig::default();
    for table in ManagedTable::ALL {
        let horizon = if table == ManagedTable::AuditEvent && days == 30 {
            90
        } else {
            days
        };
        for day in 0..=horizon {
            let at = (end - Duration::days(horizon) + Duration::days(day))
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc();
            PartitionRepository::ensure_day(db.pool(), table, at, &config).await?;
        }
    }
    println!(
        "{}",
        serde_json::json!({"type":"source_import", "days":days,"fixture":fixture,"diagnostic_thin_fixture":fixture.is_none(),"operation_timeout_milliseconds":config.operation_timeout_milliseconds})
    );
    sqlx::raw_sql("ALTER TABLE event DISABLE TRIGGER event_created_notify; ALTER TABLE enforcement DISABLE TRIGGER enforcement_created_notify;")
        .execute(db.pool()).await?;
    if let Some(path) = fixture {
        let sql = std::fs::read_to_string(path)?;
        sqlx::raw_sql(&sql).execute(db.pool()).await?;
    } else {
        // Same daily source density and source horizons. Payloads are thin and live
        // execution/enforcement rows are absent, so this is not an acceptance run.
        sqlx::query("INSERT INTO event (created,trigger_ref,payload) SELECT $1::timestamptz + ((g - 1) * $2::bigint * 86400 / ($2::bigint * 40000)) * interval '1 second', 'evidence.trigger_' || g % 8, jsonb_build_object('id',g) FROM generate_series(1,$2::bigint * 40000) g")
            .bind(start).bind(days).execute(db.pool()).await?;
        println!(
            "{}",
            serde_json::json!({"type":"stage","stage":"event_imported"})
        );
        sqlx::query("INSERT INTO execution_history(time,operation,entity_id,entity_ref,changed_fields,new_values)
            SELECT $1::timestamptz + ((g - 1) * $2::bigint * 86400 / ($2::bigint * 64000)) * interval '1 second',
            CASE WHEN g % 16 < 5 THEN 'INSERT' ELSE 'UPDATE' END,g,'evidence.action_' || g % 8,
            CASE WHEN g % 16 < 5 THEN ARRAY[]::text[] ELSE ARRAY['status'] END,
            jsonb_build_object('status', CASE WHEN g % 3 = 0 THEN 'completed' ELSE 'running' END)
            FROM generate_series(1,$2::bigint * 64000) g")
            .bind(start).bind(days).execute(db.pool()).await?;
        println!(
            "{}",
            serde_json::json!({"type":"stage","stage":"history_imported"})
        );
        sqlx::query("INSERT INTO worker_history(time,operation,entity_id,entity_ref,changed_fields,new_values)
            SELECT $1::timestamptz + h * interval '6 hours','UPDATE',w,'evidence.worker_' || w,ARRAY['status'],'{\"status\":\"active\"}'::jsonb FROM generate_series(0,$2::bigint * 4 - 1) h CROSS JOIN generate_series(1,100) w")
            .bind(start).bind(days).execute(db.pool()).await?;
        let audit_days = if days == 30 { 90 } else { days };
        // Audit notification fanout is outside this thin source diagnostic. The
        // unchanged full fixture path above keeps the acceptance import behavior.
        sqlx::raw_sql("ALTER TABLE audit_event DISABLE TRIGGER USER;")
            .execute(db.pool())
            .await?;
        sqlx::query("INSERT INTO audit_event(category,event_type,outcome,resource_ref,created,details)
            SELECT 'execution','execution.completed','success','evidence.action_' || g % 8,
            $1::timestamptz + ((g - 1) * $2::bigint * 86400 / ($2::bigint * 20000)) * interval '1 second',jsonb_build_object('id',g) FROM generate_series(1,$2::bigint * 20000) g")
            .bind(end - Duration::days(audit_days)).bind(audit_days).execute(db.pool()).await?;
        sqlx::raw_sql("ALTER TABLE audit_event ENABLE TRIGGER USER;")
            .execute(db.pool())
            .await?;
        println!(
            "{}",
            serde_json::json!({"type":"stage","stage":"audit_imported"})
        );
    }
    sqlx::raw_sql("ALTER TABLE event ENABLE TRIGGER event_created_notify; ALTER TABLE enforcement ENABLE TRIGGER enforcement_created_notify; ANALYZE;")
        .execute(db.pool()).await?;
    // Log all actual protocol SQL, including SET and COMMIT, for server/wall diagnosis.
    let options = PgConnectOptions::from_str(db.database_url())?
        .application_name("summary_budget_probe")
        .options([
            ("search_path", "attune,public"),
            ("log_min_duration_statement", "0"),
        ]);
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await?;
    let run = async {
        for kind in SummaryKind::ALL {
            let probe = start + Duration::hours((days * 24 - 1).min(36));
            let (table, timestamp, dimension, status, predicate) = match kind {
                SummaryKind::EventVolume => ("event","created","trigger_ref","NULL::text","TRUE"),
                SummaryKind::ExecutionStatus => ("execution_history","time","entity_ref","new_values->>'status'","'status'=ANY(changed_fields)"),
                SummaryKind::ExecutionCreation => ("execution_history","time","entity_ref","NULL::text","operation='INSERT'"),
                SummaryKind::WorkerStatus => ("worker_history","time","entity_ref","new_values->>'status'","'status'=ANY(changed_fields)"),
            };
            let plan: serde_json::Value = sqlx::query_scalar(&format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) SELECT {dimension},{status},count(*)::bigint FROM {table} WHERE {timestamp} >= $1 AND {timestamp} < $2 AND {predicate} GROUP BY 1,2"))
                .bind(probe).bind(probe+Duration::hours(1)).fetch_one(&pool).await?;
            println!("{}", serde_json::json!({"type":"source_plan","kind":kind,"plan":plan}));
        }
        let started = Instant::now();
        for hour in 0..=days * 24 {
            let bucket = start + Duration::hours(hour);
            for kind in SummaryKind::ALL {
                let began = Instant::now();
                match SummaryRepository::refresh_bucket_at(&pool,kind,bucket,&config,end+Duration::hours(1)).await {
                    Ok(result) => println!("{}",serde_json::json!({"type":"refresh","result":result})),
                    Err(error) => {
                        let committed = match &error {
                            attune_common::Error::Other(other) => other.downcast_ref::<attune_common::repositories::native_maintenance::summaries::SummaryCommitDeadlineExceeded>().map(|overrun| &overrun.committed),
                            _ => None,
                        };
                        println!("{}",serde_json::json!({"type":"refresh_error","kind":kind,"bucket":bucket,"elapsed_ms":began.elapsed().as_secs_f64()*1000.0,"error":error.to_string(),"committed":committed}));
                        return Err(error.into());
                    }
                }
            }
        }
        let status = SummaryRepository::status(&pool).await?;
        if status.iter().any(|s| s.coverage_hours != days * 24 + 1 || s.dirty_notifications != 0) { return Err(format!("incomplete clean coverage {status:?}").into()); }
        for kind in SummaryKind::ALL {
            let (dimension,status_column,count_column,predicate) = match kind {
                SummaryKind::EventVolume => ("trigger_ref","NULL::text","event_count","TRUE"),
                SummaryKind::ExecutionStatus | SummaryKind::WorkerStatus => ("entity_ref","new_values->>'status'","transition_count","'status'=ANY(changed_fields)"),
                SummaryKind::ExecutionCreation => ("entity_ref","NULL::text","execution_count","operation='INSERT'"),
            };
            let ref_column = match kind { SummaryKind::EventVolume => "trigger_ref", SummaryKind::WorkerStatus => "worker_name", _ => "action_ref" };
            let summary_status = if matches!(kind,SummaryKind::ExecutionStatus|SummaryKind::WorkerStatus) {"new_status"} else {"NULL::text"};
            let raw: Vec<SummaryCountRow> = sqlx::query_as(&format!("SELECT date_trunc('hour',{},'UTC'),{dimension},{status_column},count(*)::bigint FROM {} WHERE {} >= $1 AND {} < $2 AND {predicate} GROUP BY 1,2,3 ORDER BY 1,2 NULLS LAST,3 NULLS LAST",kind.time_column(),kind.source_table(),kind.time_column(),kind.time_column()))
                .bind(start).bind(end+Duration::hours(1)).fetch_all(&pool).await?;
            let stored: Vec<SummaryCountRow> = sqlx::query_as(&format!("SELECT bucket,{ref_column},{summary_status},{count_column} FROM {} WHERE bucket >= $1 AND bucket < $2 ORDER BY 1,2 NULLS LAST,3 NULLS LAST",kind.summary_table()))
                .bind(start).bind(end+Duration::hours(1)).fetch_all(&pool).await?;
            if raw != stored { return Err(format!("grouped oracle mismatch {kind:?}").into()); }
            println!("{}",serde_json::json!({"type":"grouped_oracle","kind":kind,"groups":raw.len(),"count":raw.iter().map(|r|r.3).sum::<i64>(),"correct":true}));
        }
        println!("{}",serde_json::json!({"type":"materialization","elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"status":status,"diagnostic_thin_fixture":fixture.is_none(),"config":config}));
        Ok::<_,Box<dyn std::error::Error>>(())
    }.await;
    pool.close().await;
    run
}
