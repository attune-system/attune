//! Explicit stress profile, never selected by routine correctness runs.
use super::*;
use attune_common::repositories::{
    artifact_read_diagnostics::ArtifactReadDiagnostics,
    native_maintenance::{summaries::SummaryRepository, SummaryKind},
    retention::{RetentionRepository, RetentionTarget},
};

fn resident_memory() -> Value {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    json!(status
        .lines()
        .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
        .map(str::to_string)
        .collect::<Vec<_>>())
}

async fn ancestry(ctx: &TestContext, action_ref: &str, depth: usize) -> Result<i64> {
    let mut parent = None;
    for _ in 0..depth {
        let row = ExecutionRepository::create(
            &ctx.pool,
            CreateExecutionInput {
                action_ref: action_ref.into(),
                parent,
                ..Default::default()
            },
        )
        .await?;
        parent = Some(row.id);
    }
    parent.ok_or_else(|| "ancestry profile needs a positive depth".into())
}

#[tokio::test]
#[ignore = "explicit 2/20/100-artifact and 1/100/1000-history workload profile"]
async fn artifact_cardinality_and_history_workload_profile() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let stdout = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    let stderr = artifact(
        &ctx,
        "artifact_perf.run.stderr.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    let mut retained = Vec::new();
    let mut histories = Vec::new();
    for count in [1, 100, 1000] {
        while retained.len() < count {
            let id = execution(&ctx, &action_ref).await?;
            link(&ctx, stdout, id).await?;
            link(&ctx, stderr, id).await?;
            retained.push(id);
        }
        let latest = retained.last().ok_or("history fixture is empty")?;
        let path = format!("/api/v1/executions/{latest}/artifacts");
        AuthorizationService::invalidate_identity_authz_cache(
            ctx.user.as_ref().ok_or("identity missing")?.id,
        )
        .await;
        AuthorizationService::invalidate_permission_set_caches().await;
        let mut samples = Vec::new();
        for _ in 0..21 {
            let (sample, body) = measure(&ctx, &path).await?;
            assert_eq!(sample.status, 200, "{body}");
            assert_eq!(body["items"].as_array().ok_or("items missing")?.len(), 2);
            assert_eq!(body["has_more"], false);
            assert_eq!(sample.queries.artifact_count_reads, 0);
            assert_eq!(sample.queries.artifact_linked_history_reads, 0);
            assert_eq!(sample.queries.ciphertext_reads, 0);
            assert!(sample.response_bytes < 128 * 1024);
            samples.push(sample);
        }
        // Four simultaneous clients, five complete requests each. Do not call
        // these network/browser measurements, and never omit a failed response.
        let mut concurrent = Vec::new();
        for _ in 0..5 {
            let wave = futures::future::join_all((0..4).map(|_| measure(&ctx, &path))).await;
            for result in wave {
                let (sample, body) = result?;
                assert_eq!(sample.status, 200, "{body}");
                assert_eq!(body["items"].as_array().ok_or("items missing")?.len(), 2);
                assert_eq!(body["has_more"], false);
                concurrent.push(sample);
            }
        }
        histories.push(json!({"retained_executions":count,"runtime_log_artifacts":2,
            "first_after_authorization_cache_invalidation":samples.remove(0),"warm_samples":samples,
            "four_client_samples":concurrent,"capacity":ArtifactReadDiagnostics::snapshot(&ctx.pool).await?}));
    }
    let latest = *retained.last().ok_or("history fixture is empty")?;
    let mut artifact_count = 2;
    let mut cardinalities = Vec::new();
    for count in [2, 20, 100] {
        while artifact_count < count {
            let progress = artifact_count == 2;
            let reference = format!("artifact_perf.run.item{artifact_count}");
            let id = artifact(
                &ctx,
                &reference,
                ArtifactClassification::General,
                progress.then(|| json!({"body":"x".repeat(10 * 1024 * 1024)})),
            )
            .await?;
            link(&ctx, id, latest).await?;
            artifact_count += 1;
        }
        let mut samples = Vec::new();
        for _ in 0..20 {
            let (sample, body) = measure(
                &ctx,
                &format!("/api/v1/executions/{latest}/artifacts?per_page=20"),
            )
            .await?;
            assert_eq!(sample.status, 200, "{body}");
            assert_eq!(body["has_more"], count > 20);
            assert_eq!(body["next_cursor"].is_string(), count > 20);
            assert!(body.get("pagination").is_none());
            assert_eq!(sample.queries.artifact_count_reads, 0);
            assert_eq!(
                body["items"].as_array().ok_or("items missing")?.len(),
                count.min(20)
            );
            assert_eq!(sample.queries.payload_projection_reads, 0);
            assert_eq!(sample.queries.ciphertext_reads, 0);
            assert!(
                sample.response_bytes < 128 * 1024,
                "large progress data escaped the summary projection"
            );
            samples.push(sample);
        }
        cardinalities.push(json!({"artifacts":count,"retained_executions":1000,"per_page":20,
            "large_progress_bytes":if count >= 20 {10 * 1024 * 1024} else {0},"warm_samples":samples}));
    }
    save_report(
        "artifact-cardinality-profile",
        &json!({"history_cases":histories,"cardinality_cases":cardinalities,
        "limitations":["first after auth-cache invalidation is not cold PostgreSQL","metadata samples are not content-disclosure latency measurements",
            "four concurrent in-process Axum clients, not browser/network TTFB","no matched legacy p95 baseline"]}),
    )?;
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "explicit partition-count, maintenance-concurrency, and capacity stress profile"]
async fn artifact_partition_and_maintenance_workload_profile() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let stdout = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    let stderr = artifact(
        &ctx,
        "artifact_perf.run.stderr.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    let mut latest = 0;
    for _ in 0..100 {
        latest = execution(&ctx, &action_ref).await?;
        link(&ctx, stdout, latest).await?;
        link(&ctx, stderr, latest).await?;
    }
    grant(
        &ctx,
        "fixture.profile_events",
        json!([{"resource":"events","actions":["read","decrypt"]}]),
    )
    .await?;
    let trigger = trigger(&ctx).await?;
    let day =
        chrono::DateTime::parse_from_rfc3339("2020-01-02T00:00:00Z")?.with_timezone(&chrono::Utc);
    let mut sources = Vec::new();
    for _ in 0..63 {
        sources.push(event(&ctx, &trigger, Some(day + chrono::Duration::hours(3))).await?);
    }
    // One retained late source stays in DEFAULT throughout the partition sweep.
    sources.push(event(&ctx, &trigger, Some(day - chrono::Duration::days(1))).await?);
    secret_origins(
        &ctx,
        latest,
        sources
            .iter()
            .map(|id| entity_origin(EVENT_SOURCE_PAYLOAD, *id))
            .collect(),
    )
    .await?;
    // Keep measuring actual source authorization, not the now-safe metadata path.
    let path = format!("/api/v1/artifacts/{stdout}/versions/100");
    let native = fixture_maintenance_config();
    let mut prepared_days = 0;
    let mut profiles = Vec::new();
    for days in [1, 7, 30, 90] {
        while prepared_days < days {
            let at = day + chrono::Duration::days(prepared_days);
            let outcome =
                PartitionRepository::ensure_day(&ctx.pool, ManagedTable::Event, at, &native)
                    .await?;
            assert!(matches!(
                outcome,
                PartitionRepairOutcome::Applied { .. } | PartitionRepairOutcome::AlreadyPresent
            ));
            // Unrelated rows keep the fan-out test from measuring empty indexes.
            sqlx::query("INSERT INTO event (trigger_ref,created) SELECT 'profile.unrelated',$1 FROM generate_series(1,100)")
                .bind(at + chrono::Duration::hours(5)).execute(&ctx.pool).await?;
            prepared_days += 1;
        }
        ArtifactRepository::analyze_read_fixtures(&ctx.pool).await?;
        ArtifactReadDiagnostics::analyze_event_fixtures(&ctx.pool).await?;
        let mut samples = Vec::new();
        for _ in 0..21 {
            let (sample, body) = measure(&ctx, &path).await?;
            assert_eq!(sample.status, 200, "{body}");
            assert_eq!(body["data"]["version"], 100);
            assert_eq!(sample.queries.ciphertext_reads, 0);
            assert!(
                sample.queries.pool_acquisitions > 0,
                "pool acquisition observer is inactive"
            );
            if cfg!(target_os = "linux") {
                assert!(sample.handler_poll_cpu_seconds.is_some());
                assert!(sample
                    .queries
                    .execution_disclosure_poll_cpu_seconds
                    .is_some());
            }
            samples.push(sample);
        }
        // Keep a connection so SQLx's real prepared SELECT crosses the automatic
        // custom/generic threshold before explaining that same server statement.
        let mut connection = ctx.pool.acquire().await?;
        for _ in 0..8 {
            EventRepository::find_disclosure_rows(&mut *connection, &sources).await?;
        }
        let prepared_plan =
            EventRepository::explain_prepared_disclosure_rows(&mut connection, &sources).await?;
        drop(connection);
        profiles.push(json!({"daily_partitions":days, "default_source_count":1,
            "first_after_fixture_ddl":samples.remove(0), "warm_samples":samples,
            "prepared_event_plan":prepared_plan, "memory":resident_memory(),
            "capacity":ArtifactReadDiagnostics::snapshot(&ctx.pool).await?}));
    }

    // Same data and predicates, different SQLx parameter cardinalities. This is
    // a plan-sensitivity probe, not permission to change the runtime batch bound.
    let mut batch_profiles = Vec::new();
    let mut batch_ids = sources.clone();
    while batch_ids.len() < 256 {
        batch_ids.push(event(&ctx, &trigger, Some(day + chrono::Duration::hours(3))).await?);
    }
    for size in [1, 16, 64, 256] {
        let ids = &batch_ids[..size];
        let mut connection = ctx.pool.acquire().await?;
        for _ in 0..8 {
            assert_eq!(
                EventRepository::find_disclosure_rows(&mut *connection, ids)
                    .await?
                    .len(),
                size
            );
        }
        batch_profiles.push(json!({"ids":size, "prepared":EventRepository::explain_prepared_disclosure_rows(&mut connection, ids).await?}));
    }

    // An exclusively owned expired cohort is independent of the requested logs
    // and source events. Drain it completely rather than timing one easy batch.
    sqlx::query("INSERT INTO execution (action_ref,status,created,updated) SELECT 'profile.expired','completed','2019-01-01'::timestamptz,'2019-01-01'::timestamptz FROM generate_series(1,600)")
        .execute(&ctx.pool).await?;
    let repair_day = day - chrono::Duration::days(100);
    sqlx::query("INSERT INTO event (trigger_ref,created) SELECT 'profile.repair',$1 FROM generate_series(1,100)")
        .bind(repair_day).execute(&ctx.pool).await?;
    let before = ArtifactReadDiagnostics::snapshot(&ctx.pool).await?;
    let barrier = tokio::sync::Barrier::new(2);
    let reads = async {
        barrier.wait().await;
        let mut samples = Vec::new();
        let mut wait_samples = Vec::new();
        for _ in 0..40 {
            let (lock_waiters, sessions) = ArtifactReadDiagnostics::wait_sample(&ctx.pool).await?;
            wait_samples.push(json!({"lock_waiters":lock_waiters,"sessions":sessions,
                "pool_size":ctx.pool.size(),"pool_idle":ctx.pool.num_idle()}));
            let (sample, body) = measure(&ctx, &path).await?;
            assert_eq!(sample.status, 200, "{body}");
            assert_eq!(body["data"]["version"], 100);
            samples.push(sample);
        }
        Ok::<_, Box<dyn std::error::Error>>((samples, wait_samples))
    };
    let maintenance = async {
        use attune_common::{
            audit::{AuditCategory, AuditEventBuilder, AuditOutcome},
            config::RetentionConfig,
        };
        barrier.wait().await;
        let retention = RetentionConfig {
            batch_size: 50,
            max_batches_per_target: 4,
            dry_run: false,
            ..Default::default()
        };
        let mut deleted = 0;
        let mut calls = 0;
        loop {
            let result = RetentionRepository::run_target_bounded(
                &ctx.pool,
                RetentionTarget::Executions,
                3600,
                &retention,
                || false,
            )
            .await?;
            deleted += result.deleted;
            calls += 1;
            if result.deleted == 0 {
                break;
            }
            assert!(
                calls <= 4,
                "expired cohort did not drain within its predeclared budget"
            );
        }
        assert_eq!(deleted, 600);
        assert_eq!(
            PartitionRepository::ensure_day(&ctx.pool, ManagedTable::Event, repair_day, &native)
                .await?,
            PartitionRepairOutcome::Applied { rows_moved: 100 }
        );
        let dropped = PartitionRepository::expire_before(
            &ctx.pool,
            ManagedTable::Event,
            repair_day + chrono::Duration::days(1),
            &native,
            false,
            || false,
        )
        .await?;
        assert_eq!(dropped.partitions_dropped, 1);
        let audit_started = std::time::Instant::now();
        for _ in 0..100 {
            ctx.state.audit_emitter.emit(
                AuditEventBuilder::new(
                    AuditCategory::Api,
                    "artifact_profile.fixture",
                    AuditOutcome::Success,
                )
                .resource("artifact_profile")
                .build(),
            );
        }
        ctx.flush_audit().await?;
        let audit_emit_and_flush_seconds = audit_started.elapsed().as_secs_f64();
        let summary = SummaryRepository::refresh_bucket(
            &ctx.pool,
            SummaryKind::EventVolume,
            day + chrono::Duration::hours(3),
            &native,
        )
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(
            json!({"expired_execution_rows_deleted":deleted, "retention_calls":calls,
            "explicit_audit_records_emitted":100,"audit_emit_and_flush_seconds":audit_emit_and_flush_seconds,
            "event_partitions_dropped":dropped.partitions_dropped, "summary":summary}),
        )
    };
    let ((concurrent_samples, wait_samples), maintenance) = futures::try_join!(reads, maintenance)?;
    let after = ArtifactReadDiagnostics::snapshot(&ctx.pool).await?;
    let final_plan = EventRepository::explain_disclosure_rows(&ctx.pool, &sources).await?;
    let mut graph_profiles = Vec::new();
    for shared in [true, false] {
        let shared_parent = if shared {
            Some(ancestry(&ctx, &action_ref, 8).await?)
        } else {
            None
        };
        let mut leaves = Vec::new();
        for _ in 0..64 {
            let parent = match shared_parent {
                Some(parent) => parent,
                None => ancestry(&ctx, &action_ref, 8).await?,
            };
            let leaf = ExecutionRepository::create(
                &ctx.pool,
                CreateExecutionInput {
                    action_ref: action_ref.clone(),
                    parent: Some(parent),
                    ..Default::default()
                },
            )
            .await?;
            source_secret(&ctx, ENTITY_EXECUTION_CONFIG, leaf.id).await?;
            leaves.push(leaf.id);
        }
        secret_origins(
            &ctx,
            latest,
            leaves.iter().map(|id| execution_origin(*id)).collect(),
        )
        .await?;
        ArtifactRepository::analyze_read_fixtures(&ctx.pool).await?;
        let mut samples = Vec::new();
        for _ in 0..20 {
            let (sample, body) = measure(&ctx, &path).await?;
            assert_eq!(sample.status, 200, "{body}");
            assert_eq!(body["data"]["version"], 100);
            assert_eq!(sample.queries.ciphertext_reads, 0);
            samples.push(sample);
        }
        let mut connection = ctx.pool.acquire().await?;
        for _ in 0..8 {
            ExecutionRepository::find_authorization_lineages(&mut *connection, &leaves).await?;
        }
        let plan =
            ExecutionRepository::explain_authorization_lineages(&mut *connection, &leaves).await?;
        drop(connection);
        graph_profiles.push(json!({"shared_ancestry":shared,"ancestor_depth":8,"source_executions":64,
            "samples":samples,"lineage_plan":plan,"capacity":ArtifactReadDiagnostics::snapshot(&ctx.pool).await?,
            "explain_scope":"same repository SELECT and binds, separate EXPLAIN statement"}));
    }
    save_report(
        "artifact-workload-profile",
        &json!({
            "fixture":"two-logs-100-executions-64-event-origins", "selected_content_endpoint":path,
            "permissions":"fixture.artifact_reader-and-profile_events",
            "maintenance_test_operation_timeout_milliseconds":native.operation_timeout_milliseconds,
            "cache_state":"first-after-ddl-and-warm-reported-separately", "concurrency":{"readers":1,"maintenance_tasks":1},
            "partition_profiles":profiles, "batch_profiles":batch_profiles,
            "maintenance":maintenance, "concurrent_samples":concurrent_samples,
            "sampled_database_waits_between_requests":wait_samples,
            "capacity_before_churn":before, "capacity_after_churn":after,
            "post_churn_event_plan_without_forced_analyze":final_plan,
            "post_churn_memory":resident_memory(),
            "graph_profiles":graph_profiles,
            "limitations":["in-process Axum requests, not network/browser TTFB", "no fixed-source legacy latency baseline",
                "catalog tuple counters are estimates, not measured bloat", "fixture sizes do not certify deployment retention capacity"]
        }),
    )?;
    ctx.cleanup().await?;
    Ok(())
}
