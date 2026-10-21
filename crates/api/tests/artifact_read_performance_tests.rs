//! Real-route authorization characterization and query-growth gates.
//! SQLx events are observed only while polling the request future; the threaded
//! audit writer is deliberately outside that subscriber. No SQL or binds are saved.

#[path = "artifact_workload/cpu.rs"]
mod cpu;
mod helpers;
#[path = "artifact_read_performance/managed_fixture.rs"]
mod managed_fixture;
#[path = "artifact_workload/reference.rs"]
mod reference;
#[path = "artifact_read_performance/source_origins.rs"]
mod source_origins;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use attune_api::authz::AuthorizationService;
use attune_common::{
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactReadContext, ArtifactRepository, ArtifactSearchFilters,
            ArtifactVersionRepository, CreateArtifactInput, CreateArtifactVersionInput,
        },
        execution::{CreateExecutionInput, ExecutionRepository},
        execution_secret_value::ExecutionSecretValueRepository,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        Create,
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{prepare_secret_values, SecretValueInput, ENTITY_EXECUTION_CONFIG},
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde::Serialize;
use serde_json::{json, Value};
use tracing::{
    field::{Field, Visit},
    instrument::WithSubscriber,
    Event, Subscriber,
};
use tracing_subscriber::{layer::Context, prelude::*, Layer};

#[derive(Clone, Default, Serialize)]
struct QueryMetrics {
    statements: usize,
    statement_categories: std::collections::BTreeMap<String, usize>,
    graph_record_reads: usize,
    returned_rows: u64,
    sqlx_elapsed_seconds: f64,
    payload_projection_reads: usize,
    ciphertext_reads: usize,
    source_scope_reads: usize,
    source_payload_reads: usize,
    pool_acquisitions: usize,
    pool_acquire_seconds: f64,
    execution_disclosure_poll_cpu_seconds: Option<f64>,
}

impl QueryMetrics {
    fn driver_metadata_reads(&self) -> usize {
        ["driver_type_discovery", "driver_enum_discovery"]
            .iter()
            .map(|category| {
                self.statement_categories
                    .get(*category)
                    .copied()
                    .unwrap_or(0)
            })
            .sum()
    }

    fn application_statements(&self) -> usize {
        self.statements - self.driver_metadata_reads()
    }
}

impl Visit for QueryMetrics {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() != "db.statement" {
            return;
        }
        // SQLx wraps long statements in newlines. Normalize whitespace so the
        // ciphertext gate observes those events, not just short literal SQL.
        let sql = value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        let primary_table = sql
            .split_once(" from ")
            .and_then(|(_, tail)| tail.split_whitespace().next());
        // Keep diagnostics independent of values and SQL text. In particular,
        // distinguish cache misses from repeated graph/disclosure reads.
        let category = match primary_table {
            Some("pg_catalog.pg_type") => "driver_type_discovery",
            Some("pg_catalog.pg_enum") => "driver_enum_discovery",
            Some(
                table @ ("execution"
                | "execution_secret_value"
                | "runtime_schema_binding"
                | "execution_key_read_origin"
                | "identity"
                | "permission_set"
                | "role"
                | "permission_assignment"
                | "identity_role_assignment"
                | "role_permission_assignment"
                | "artifact"
                | "artifact_version"),
            ) => table,
            _ if sql.starts_with("with recursive lineage") => "lineage",
            _ if sql.starts_with("begin") => "begin",
            _ if sql.starts_with("commit") => "commit",
            _ if sql.starts_with("rollback") => "rollback",
            _ if sql.starts_with("insert") => "other_insert",
            _ if sql.starts_with("update") => "other_update",
            _ if sql.starts_with("delete") => "other_delete",
            _ if sql.contains("pg_advisory") => "advisory_lock",
            _ if sql.starts_with("select") => "other_select",
            _ if sql.is_empty() => "empty_statement",
            _ => "other",
        };
        *self
            .statement_categories
            .entry(category.into())
            .or_default() += 1;
        if sql.starts_with("with recursive lineage")
            || matches!(
                primary_table,
                Some("execution" | "execution_secret_value" | "runtime_schema_binding")
            )
        {
            self.graph_record_reads += 1;
        }
        if sql.starts_with("select ")
            && (sql.contains(" from execution_secret_value")
                || sql.contains(" from event_origin_snapshot"))
            && sql.split(" from ").next().is_some_and(|projection| {
                projection.contains("encrypted_value")
                    || projection.contains("encryption_key_hash")
                    || projection
                        .split(',')
                        .any(|column| column.trim() == "snapshot")
            })
        {
            self.ciphertext_reads += 1;
        }
        if sql.contains(" from component_configuration_revision")
            || sql.contains(" from sensor_generation_schema_binding")
            || sql.contains(" join sensor_rule_structural_origin")
            || matches!(
                primary_table,
                Some(
                    "event"
                        | "event_origin_snapshot"
                        | "event_schema_binding"
                        | "inquiry"
                        | "inquiry_schema_binding"
                        | "sensor_generation_schema_binding"
                        | "sensor_rule_config_delivery"
                        | "sensor_rule_structural_origin"
                        | "rule"
                        | "action"
                )
            )
        {
            self.source_scope_reads += 1;
            if sql.split(" from ").next().is_some_and(|projection| {
                projection.split(',').any(|column| {
                    matches!(
                        column.trim().rsplit('.').next(),
                        Some("payload" | "response" | "response_options" | "config")
                    )
                })
            }) {
                self.source_payload_reads += 1;
            }
        }
        let Some((projection, _)) = sql.split_once(" from artifact") else {
            return;
        };
        if projection.split(',').any(|column| {
            matches!(
                column.trim().rsplit('.').next(),
                Some("data" | "content_json" | "meta")
            )
        }) {
            self.payload_projection_reads += 1;
        }
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "rows_returned" {
            self.returned_rows += value;
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "elapsed_secs" {
            self.sqlx_elapsed_seconds += value;
        }
        if field.name() == "aquired_after_secs" {
            self.pool_acquisitions += 1;
            self.pool_acquire_seconds += value;
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

#[derive(Clone, Default)]
struct RequestQueries(Arc<Mutex<QueryMetrics>>);

#[derive(Default)]
struct DisclosureCpuStart {
    threads: std::collections::HashMap<std::thread::ThreadId, (usize, Option<u64>)>,
}

impl<S> Layer<S> for RequestQueries
where
    S: Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_enter(&self, id: &tracing::span::Id, context: Context<'_, S>) {
        if let Some(span) = context
            .span(id)
            .filter(|span| span.metadata().name() == "artifact_execution_disclosure")
        {
            let mut extensions = span.extensions_mut();
            if extensions.get_mut::<DisclosureCpuStart>().is_none() {
                extensions.insert(DisclosureCpuStart::default());
            }
            let state = extensions.get_mut::<DisclosureCpuStart>().unwrap();
            let entry = state
                .threads
                .entry(std::thread::current().id())
                .or_insert_with(|| (0, cpu::thread_cpu_nanoseconds()));
            entry.0 += 1;
        }
    }

    fn on_exit(&self, id: &tracing::span::Id, context: Context<'_, S>) {
        if let Some(span) = context
            .span(id)
            .filter(|span| span.metadata().name() == "artifact_execution_disclosure")
        {
            // SQLx reenters the current span. Count its outermost entry once
            // per thread, including when futures migrate between threads.
            let started = {
                let mut extensions = span.extensions_mut();
                let Some(state) = extensions.get_mut::<DisclosureCpuStart>() else {
                    return;
                };
                let thread = std::thread::current().id();
                let Some((depth, _)) = state.threads.get_mut(&thread) else {
                    return;
                };
                *depth -= 1;
                if *depth == 0 {
                    state.threads.remove(&thread).and_then(|(_, time)| time)
                } else {
                    None
                }
            };
            if let Some(nanoseconds) = started
                .zip(cpu::thread_cpu_nanoseconds())
                .and_then(|(before, after)| after.checked_sub(before))
            {
                *self
                    .0
                    .lock()
                    .unwrap()
                    .execution_disclosure_poll_cpu_seconds
                    .get_or_insert(0.0) += nanoseconds as f64 / 1_000_000_000.0;
            }
        }
    }

    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            let mut metrics = self.0.lock().unwrap();
            metrics.statements += 1;
            event.record(&mut *metrics);
        } else if event.metadata().target() == "sqlx::pool::acquire" {
            event.record(&mut *self.0.lock().unwrap());
        }
    }
}

#[derive(Serialize)]
struct Measurement {
    status: u16,
    response_bytes: usize,
    handler_seconds: f64,
    completion_seconds: f64,
    handler_poll_cpu_seconds: Option<f64>,
    queries: QueryMetrics,
}

#[test]
fn query_observer_detects_payload_columns_without_recording_sql() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG, db.statement = "SELECT id, data, created FROM artifact", rows_returned = 1u64);
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG, db.statement = "SELECT id, version, meta, created FROM artifact_version", rows_returned = 2u64);
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG, db.statement = "SELECT id, name, created FROM artifact", rows_returned = 1u64);
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG, db.statement = "\n\nSELECT json_path, encrypted_value, encryption_key_hash\nFROM execution_secret_value\n", rows_returned = 1u64);
    });
    let metrics = observer.0.lock().unwrap();
    assert_eq!(metrics.statements, 4);
    assert_eq!(metrics.returned_rows, 5);
    assert_eq!(metrics.payload_projection_reads, 2);
    assert_eq!(metrics.ciphertext_reads, 1);
    assert_eq!(metrics.graph_record_reads, 1);
}

#[test]
fn query_budget_separates_connection_local_type_discovery_but_reports_every_statement() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            db.statement = "SELECT oid::regtype::text, typtype FROM pg_catalog.pg_type WHERE oid = $1");
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            db.statement = "SELECT enumlabel FROM pg_catalog.pg_enum WHERE enumtypid = $1 ORDER BY enumsortorder");
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            db.statement = "SELECT id FROM execution WHERE id = $1");
    });
    let metrics = observer.0.lock().unwrap();
    assert_eq!(metrics.statements, 3);
    assert_eq!(metrics.driver_metadata_reads(), 2);
    assert_eq!(metrics.application_statements(), 1);
    assert_eq!(metrics.graph_record_reads, 1);
}

#[test]
fn query_observer_counts_zipped_foreign_proof_join_as_source_metadata() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        tracing::event!(target:"sqlx::query",tracing::Level::DEBUG,db.statement="SELECT p.source_entity,p.proof FROM unnest($1::TEXT[],$2::BIGINT[],$3::BIGINT[]) requested JOIN sensor_rule_structural_origin p ON (p.source_entity,p.source_id,p.rule_id)=(requested.entity,requested.id,requested.rule)");
    });
    let metrics = observer.0.lock().unwrap();
    assert_eq!(metrics.application_statements(), 1);
    assert_eq!(metrics.source_scope_reads, 1);
    assert_eq!(metrics.ciphertext_reads, 0);
    assert_eq!(metrics.source_payload_reads, 0);
}

#[test]
fn query_observer_attributes_owner_subqueries_and_pool_acquisition() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG, db.statement = "SELECT r.id, CASE WHEN EXISTS (SELECT 1 FROM pack p) THEN TRUE END FROM component_configuration_revision r");
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG, db.statement = "SELECT id, payload FROM event WHERE id = ANY($1)");
        tracing::event!(target: "sqlx::pool::acquire", tracing::Level::DEBUG, aquired_after_secs = 0.125f64);
    });
    let metrics = observer.0.lock().unwrap();
    assert_eq!(metrics.statements, 2);
    assert_eq!(metrics.source_scope_reads, 2);
    assert_eq!(metrics.source_payload_reads, 1);
    assert_eq!(metrics.pool_acquisitions, 1);
    assert_eq!(metrics.pool_acquire_seconds, 0.125);
}

#[test]
fn disclosure_cpu_observer_accepts_reentrant_current_span_instrumentation() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::debug_span!("artifact_execution_disclosure");
        let _outer = span.enter();
        for _ in 0..100 {
            let _nested = span.enter();
            std::hint::black_box(42);
        }
    });
    if cfg!(target_os = "linux") {
        assert!(observer
            .0
            .lock()
            .unwrap()
            .execution_disclosure_poll_cpu_seconds
            .is_some_and(|time| time > 0.0));
    }
}

async fn measure(ctx: &TestContext, path: &str) -> Result<(Measurement, Value)> {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    let start = Instant::now();
    let (response, handler_poll_cpu_seconds) =
        cpu::with_poll_cpu(ctx.get(path, ctx.token()).with_subscriber(subscriber)).await;
    let response = response?;
    let handler_seconds = start.elapsed().as_secs_f64();
    let status = response.status().as_u16();
    let bytes = response.bytes().await?;
    let measurement = Measurement {
        status,
        response_bytes: bytes.len(),
        handler_seconds,
        completion_seconds: start.elapsed().as_secs_f64(),
        handler_poll_cpu_seconds,
        queries: observer.0.lock().unwrap().clone(),
    };
    assert!(
        measurement.queries.statements > 0,
        "SQLx query observer did not observe the handler"
    );
    Ok((measurement, serde_json::from_slice(&bytes)?))
}

async fn grant(ctx: &TestContext, reference: &str, grants: Value) -> Result<()> {
    let identity = ctx
        .user
        .as_ref()
        .ok_or("authenticated fixture identity missing")?
        .id;
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: reference.into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    AuthorizationService::invalidate_identity_authz_cache(identity).await;
    AuthorizationService::invalidate_permission_set_caches().await;
    Ok(())
}

async fn action(ctx: &TestContext) -> Result<String> {
    grant(
        ctx,
        "fixture.artifact_reader",
        json!([
            {"resource":"actions","actions":["read","execute"]},
            {"resource":"executions","actions":["read","decrypt"]}
        ]),
    )
    .await?;
    managed_fixture::action_pack(ctx).await
}

async fn execution(ctx: &TestContext, action_ref: &str) -> Result<i64> {
    let response = ctx
        .post(
            "/api/v1/executions/execute",
            json!({
                "action_ref": action_ref, "parameters": {}, "permission_set_refs": []
            }),
            ctx.token(),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"]["id"]
        .as_i64()
        .ok_or_else(|| "execution fixture did not return an ID".into())
}

async fn artifact(
    ctx: &TestContext,
    reference: &str,
    classification: ArtifactClassification,
    data: Option<Value>,
) -> Result<i64> {
    Ok(ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: reference.into(),
            scope: OwnerType::Action,
            owner: "artifact_perf.run".into(),
            r#type: if data.is_some() {
                ArtifactType::Progress
            } else {
                ArtifactType::FileText
            },
            visibility: ArtifactVisibility::Private,
            classification,
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 2000,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data,
        },
    )
    .await?
    .id)
}

async fn link(ctx: &TestContext, artifact: i64, execution: i64) -> Result<()> {
    ArtifactVersionRepository::create(
        &ctx.pool,
        CreateArtifactVersionInput {
            artifact,
            execution: Some(execution),
            content_type: Some("text/plain".into()),
            content: None,
            content_json: None,
            file_path: None,
            meta: None,
            created_by: None,
        },
    )
    .await?;
    Ok(())
}

async fn secret_origins(ctx: &TestContext, id: i64, origins: Vec<SecretOrigin>) -> Result<()> {
    let provenance = SecretProvenance {
        origins,
        templates: vec![],
    };
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/private".into(),
            value: json!("fixture material"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&provenance)?),
        }],
        ctx.state
            .config
            .security
            .encryption_key
            .as_ref()
            .ok_or("fixture key missing")?,
    )?;
    ExecutionSecretValueRepository::upsert_many(&ctx.pool, ENTITY_EXECUTION_CONFIG, id, &prepared)
        .await?;
    Ok(())
}

fn execution_origin(id: i64) -> SecretOrigin {
    SecretOrigin::Entity {
        entity_type: ENTITY_EXECUTION_CONFIG.into(),
        entity_id: id,
        path: "/private".into(),
    }
}

fn local_origin() -> SecretOrigin {
    SecretOrigin::Local {
        source_kind: "parameter_schema".into(),
        source_ref: None,
    }
}

#[tokio::test]
async fn provenance_frontiers_match_exact_kind_id_and_path_tuples() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let first = execution(&ctx, &action_ref).await?;
    let second = execution(&ctx, &action_ref).await?;
    secret_origins(&ctx, first, vec![local_origin()]).await?;
    secret_origins(&ctx, second, vec![local_origin()]).await?;
    let mut conn = ctx.pool.acquire().await?;
    let sites = vec![
        ("execution_result".into(), first, "/private".into()),
        (ENTITY_EXECUTION_CONFIG.into(), second, "/private".into()),
    ];
    let rows =
        ExecutionSecretValueRepository::find_bounded_provenance_by_sites(&mut conn, &sites).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entity_type, ENTITY_EXECUTION_CONFIG);
    assert_eq!(rows[0].entity_id, second);
    let absent = vec![(ENTITY_EXECUTION_CONFIG.into(), second, "/missing".into())];
    assert!(
        ExecutionSecretValueRepository::find_bounded_provenance_by_sites(&mut conn, &absent)
            .await?
            .is_empty()
    );
    drop(conn);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn distinct_execution_origins_and_workflow_children_use_frontier_batches() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let root = execution(&ctx, &action_ref).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    let mut origins = Vec::new();
    let mut measurements = Vec::new();
    for count in [1, 64] {
        while origins.len() < count {
            let source = execution(&ctx, &action_ref).await?;
            sqlx::query("UPDATE execution SET parent = $1 WHERE id = $2")
                .bind(root)
                .bind(source)
                .execute(&ctx.pool)
                .await?;
            secret_origins(&ctx, source, vec![local_origin()]).await?;
            origins.push(execution_origin(source));
        }
        secret_origins(&ctx, destination, origins.clone()).await?;
        let path = format!("/api/v1/artifacts/{log}");
        let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
        let (sample, body) = measure(&ctx, &path).await?;
        assert_eq!(sample.status, 200, "{body}");
        assert_eq!(
            sample.queries.ciphertext_reads, 0,
            "authorization selected ciphertext"
        );
        measurements.push((count, sample));
    }
    let report = serde_json::to_string_pretty(&measurements)?;
    println!("execution origin measurements: {report}");
    if let Some(output) = std::env::var_os("ATTUNE_ARTIFACT_BENCHMARK_OUTPUT") {
        use std::io::Write;
        let path = std::path::PathBuf::from(output).join("execution-origin-frontier.json");
        writeln!(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?,
            "{report}"
        )?;
    }
    let small = measurements[0].1.queries.graph_record_reads;
    let large = measurements[1].1.queries.graph_record_reads;
    let total = measurements[1].1.queries.statements;
    ctx.cleanup().await?;
    assert!(
        large == small && total <= 20,
        "graph-record reads grew from {small} to {large}, or total {total} exceeded 20; {report}"
    );
    Ok(())
}

#[tokio::test]
async fn shared_origin_depth_budget_and_cycles_are_checked_on_every_traversal() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let leaf = execution(&ctx, &action_ref).await?;
    secret_origins(&ctx, leaf, vec![local_origin()]).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    let path = format!("/api/v1/artifacts/{log}");
    let mut previous = leaf;
    let mut chain = Vec::new();
    for _ in 0..31 {
        let id = execution(&ctx, &action_ref).await?;
        secret_origins(&ctx, id, vec![execution_origin(previous)]).await?;
        chain.push(id);
        previous = id;
    }
    // Leaf at depth 31 allows. At depth 32 it denies even if the same record
    // was previously allowed through the direct edge, in either origin order.
    for (head, expected) in [
        (chain[29], StatusCode::OK),
        (chain[30], StatusCode::NOT_FOUND),
    ] {
        for origins in [
            vec![execution_origin(leaf), execution_origin(head)],
            vec![execution_origin(head), execution_origin(leaf)],
        ] {
            secret_origins(&ctx, destination, origins).await?;
            assert_eq!(ctx.get(&path, ctx.token()).await?.status(), expected);
        }
    }
    for origins in [
        vec![execution_origin(destination)],
        vec![execution_origin(leaf), execution_origin(destination)],
    ] {
        secret_origins(&ctx, destination, origins).await?;
        assert_eq!(
            ctx.get(&path, ctx.token()).await?.status(),
            StatusCode::NOT_FOUND
        );
    }
    secret_origins(&ctx, destination, vec![execution_origin(leaf)]).await?;
    secret_origins(&ctx, leaf, vec![execution_origin(destination)]).await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn batched_execution_sources_fail_closed_for_missing_paths_and_expiry() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let destination = execution(&ctx, &action_ref).await?;
    let source = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    let path = format!("/api/v1/artifacts/{log}");
    secret_origins(&ctx, destination, vec![execution_origin(source)]).await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    secret_origins(&ctx, source, vec![local_origin()]).await?;
    assert_eq!(ctx.get(&path, ctx.token()).await?.status(), StatusCode::OK);
    secret_origins(
        &ctx,
        destination,
        vec![SecretOrigin::Entity {
            entity_type: ENTITY_EXECUTION_CONFIG.into(),
            entity_id: source,
            path: "/missing".into(),
        }],
    )
    .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    secret_origins(&ctx, destination, vec![execution_origin(source)]).await?;
    sqlx::query("DELETE FROM execution WHERE id = $1")
        .bind(source)
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    secret_origins(
        &ctx,
        destination,
        vec![SecretOrigin::Entity {
            entity_type: "unknown_source".into(),
            entity_id: source,
            path: "/private".into(),
        }],
    )
    .await?;
    assert_eq!(
        ctx.get(&path, ctx.token()).await?.status(),
        StatusCode::NOT_FOUND
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn batched_lineages_preserve_dangling_parents_and_64_ancestor_owner_limit() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let identity = ctx.user.as_ref().ok_or("fixture identity missing")?.id;
    let root = ExecutionRepository::create(
        &ctx.pool,
        CreateExecutionInput {
            action_ref: "private.root".into(),
            executor: Some(identity),
            ..Default::default()
        },
    )
    .await?;
    let mut parent = root.id;
    let mut chain = Vec::new();
    for _ in 0..65 {
        let row = ExecutionRepository::create(
            &ctx.pool,
            CreateExecutionInput {
                action_ref: "private.child".into(),
                parent: Some(parent),
                ..Default::default()
            },
        )
        .await?;
        chain.push(row.id);
        parent = row.id;
    }
    let dangling = ExecutionRepository::create(
        &ctx.pool,
        CreateExecutionInput {
            action_ref: "private.dangling".into(),
            parent: Some(i64::MAX),
            ..Default::default()
        },
    )
    .await?;
    let ids = [chain[63], chain[64], dangling.id, i64::MAX];
    let rows = ExecutionRepository::find_authorization_lineages(&ctx.pool, &ids).await?;
    assert_eq!(
        rows.len(),
        3,
        "missing roots must not become fabricated anchors"
    );
    let near = rows
        .iter()
        .find(|row| row.execution_id == chain[63])
        .ok_or("lineage missing")?;
    assert_eq!(near.anchor_id, root.id);
    assert_eq!(near.ancestor_identity_ids, vec![identity]);
    let far = rows
        .iter()
        .find(|row| row.execution_id == chain[64])
        .ok_or("lineage missing")?;
    assert_eq!(
        far.anchor_id, root.id,
        "visibility roots have no 64-parent cutoff"
    );
    assert!(far.ancestor_identity_ids.is_empty());
    assert_eq!(
        ExecutionRepository::find_ancestor_identity_ids(&ctx.pool, chain[62]).await?,
        vec![identity]
    );
    assert!(
        ExecutionRepository::find_ancestor_identity_ids(&ctx.pool, chain[63])
            .await?
            .is_empty()
    );
    assert!(
        ExecutionRepository::find_ancestor_identity_ids(&ctx.pool, i64::MAX)
            .await?
            .is_empty()
    );
    let row = rows
        .iter()
        .find(|row| row.execution_id == dangling.id)
        .ok_or("dangling lineage missing")?;
    assert_eq!(row.anchor_id, dangling.id);
    assert!(!row.cyclic);
    sqlx::query("UPDATE execution SET parent = id WHERE id = $1")
        .bind(dangling.id)
        .execute(&ctx.pool)
        .await?;
    let rows = ExecutionRepository::find_authorization_lineages(&ctx.pool, &[dangling.id]).await?;
    assert!(
        rows[0].cyclic,
        "malformed ancestry must terminate and fail closed"
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn workflow_source_visibility_uses_root_but_self_scope_uses_child_owner() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let root = execution(&ctx, &action_ref).await?;
    let source = execution(&ctx, &action_ref).await?;
    let destination = execution(&ctx, &action_ref).await?;
    sqlx::query("UPDATE execution SET parent = $1, executor = NULL, action_ref = 'private.child' WHERE id = $2")
        .bind(root).bind(source).execute(&ctx.pool).await?;
    secret_origins(&ctx, source, vec![local_origin()]).await?;
    secret_origins(&ctx, destination, vec![execution_origin(source)]).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, destination).await?;
    for (scope, expected) in [
        ("self", StatusCode::NOT_FOUND),
        ("descendants", StatusCode::OK),
    ] {
        let grants = json!([
            {"resource":"actions","actions":["read","execute"]},
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"refs":[action_ref],"execution_scope":scope}}
        ]);
        sqlx::query("UPDATE permission_set SET grants = $1 WHERE ref = 'fixture.artifact_reader'")
            .bind(grants)
            .execute(&ctx.pool)
            .await?;
        AuthorizationService::invalidate_identity_authz_cache(
            ctx.user.as_ref().ok_or("fixture identity missing")?.id,
        )
        .await;
        AuthorizationService::invalidate_permission_set_caches().await;
        assert_eq!(
            ctx.get(&format!("/api/v1/artifacts/{log}"), ctx.token())
                .await?
                .status(),
            expected
        );
    }
    // Removing the parent loses private workflow-root visibility on the next
    // request. No allowed lineage or source record survives across requests.
    sqlx::query("UPDATE execution SET parent = NULL WHERE id = $1")
        .bind(source)
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}"), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn runtime_log_history_queries_grow_by_batches_not_executions() -> Result<()> {
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
    let mut samples = Vec::new();
    let mut already_created = 0;
    for retained in [1, 64, 100] {
        let mut latest = 0;
        for _ in already_created..retained {
            latest = execution(&ctx, &action_ref).await?;
            link(&ctx, stdout, latest).await?;
            link(&ctx, stderr, latest).await?;
        }
        already_created = retained;
        let path = format!("/api/v1/executions/{latest}/artifacts");
        ArtifactRepository::analyze_read_fixtures(&ctx.pool).await?;
        let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
        // Driver enum metadata is connection-local. Exercise pool churn so the
        // application-query gate cannot accidentally depend on warmed sockets.
        if retained == 100 {
            // Finish the warm-up request's background audit work before closing
            // sockets, so it cannot warm replacement connections outside the
            // measured request subscriber.
            ctx.flush_audit().await?;
            let mut parked = Vec::new();
            for _ in 0..ctx.pool.size() {
                parked.push(ctx.pool.acquire().await?);
            }
            for connection in parked {
                connection.close().await?;
            }
        }
        let (sample, body) = measure(&ctx, &path).await?;
        if retained == 100 {
            assert!(
                sample.queries.driver_metadata_reads() > 0,
                "Fresh-connection regression did not exercise SQLx type discovery"
            );
        }
        assert_eq!(sample.status, 200, "{body}");
        assert_eq!(
            body["items"]
                .as_array()
                .ok_or("artifact array missing")?
                .len(),
            2
        );
        samples.push((retained, sample));
        if retained == 100 {
            let user = attune_api::auth::middleware::AuthenticatedUser {
                claims: attune_api::auth::jwt::validate_token(
                    ctx.token().ok_or("fixture token missing")?,
                    &ctx.state.jwt_config,
                )?,
            };
            let identity = ctx.user.as_ref().ok_or("fixture identity missing")?;
            let read_ctx = ArtifactReadContext {
                identity_id: identity.id,
                identity_attributes: identity
                    .attributes
                    .as_object()
                    .into_iter()
                    .flat_map(|attributes| {
                        attributes
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                    })
                    .collect(),
                grants: ctx
                    .state
                    .authorization_service()
                    .effective_grants(&user)
                    .await?,
            };
            let plan = ArtifactRepository::explain_summary_scan(
                &ctx.pool,
                &ArtifactSearchFilters {
                    execution: Some(latest),
                    ..Default::default()
                },
                Some(&read_ctx),
            )
            .await?;
            if let Some(output) = std::env::var_os("ATTUNE_ARTIFACT_BENCHMARK_OUTPUT") {
                use std::io::Write;
                let path =
                    std::path::PathBuf::from(output).join("artifact-execution-summary-plan.json");
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
                writeln!(file, "{}", serde_json::to_string_pretty(&plan)?)?;
            }
        }
    }
    let report = serde_json::to_string_pretty(&samples)?;
    println!("artifact history measurements: {report}");
    if let Some(output) = std::env::var_os("ATTUNE_ARTIFACT_BENCHMARK_OUTPUT") {
        use std::io::Write;
        let path = std::path::PathBuf::from(output).join("runtime-log-history.json");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        writeln!(file, "{report}")?;
    }
    // Full statement counts and elapsed time above still include driver costs.
    // Only the application scaling budget excludes catalog type discovery.
    let small = samples[0].1.queries.application_statements();
    let within_batch = samples[1].1.queries.application_statements();
    let large = samples[2].1.queries.application_statements();
    ctx.cleanup().await?;
    assert!(
        within_batch <= small + 4 && large <= within_batch + 7 && large <= 20,
        "history authorization grew from {small} to {large} statements beyond the batch query budget; {report}"
    );
    Ok(())
}

#[tokio::test]
async fn global_summary_visibility_does_not_bypass_detail_disclosure() -> Result<()> {
    use attune_common::repositories::{key::KeyRepository, Delete};
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let id = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, id).await?;
    // Disclosure retains the actual Key identity. A fabricated ID and owner
    // tuple cannot stand in for a source whose grants are being tested.
    let key = attune_common::repositories::key::KeyRepository::create(
        &ctx.pool,
        attune_common::repositories::key::CreateKeyInput {
            local_ref: "fixture_private".into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: "Owned disclosure source".into(),
            encrypted: true,
            encryption_key_hash: None,
            value: attune_common::crypto::encrypt_json(
                &json!("fixture material"),
                ctx.state
                    .config
                    .security
                    .encryption_key
                    .as_deref()
                    .ok_or("fixture key missing")?,
            )?,
        },
    )
    .await?;
    let origin = SecretProvenance::key(attune_common::key_access::key_origin(&key));
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/private".into(),
            value: json!("fixture material"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&origin)?),
        }],
        ctx.state
            .config
            .security
            .encryption_key
            .as_ref()
            .ok_or("fixture key missing")?,
    )?;
    ExecutionSecretValueRepository::upsert_many(&ctx.pool, ENTITY_EXECUTION_CONFIG, id, &prepared)
        .await?;
    let global = ctx
        .get(&format!("/api/v1/artifacts?execution={id}"), ctx.token())
        .await?;
    assert_eq!(global.status(), StatusCode::OK);
    let body: Value = global.json().await?;
    assert_eq!(body["items"][0]["id"], log);
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}"), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let list: Value = ctx
        .get(&format!("/api/v1/executions/{id}/artifacts"), ctx.token())
        .await?
        .json()
        .await?;
    assert!(list["items"]
        .as_array()
        .ok_or("artifact array missing")?
        .is_empty());
    grant(&ctx, "fixture.origin_read", json!([
        {"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":["system.fixture_private"],"owner_types":["system"]}}
    ])).await?;
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}"), ctx.token())
            .await?
            .status(),
        StatusCode::OK
    );
    assert!(KeyRepository::delete(&ctx.pool, key.id).await?);
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}"), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND,
        "A grant must not replace missing source identity evidence",
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn summaries_exclude_large_artifact_and_version_payloads() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let execution = execution(&ctx, &action_ref).await?;
    let payload = json!({"padding": "x".repeat(10 * 1024 * 1024)});
    let id = artifact(
        &ctx,
        "artifact_perf.progress",
        ArtifactClassification::General,
        Some(payload.clone()),
    )
    .await?;
    ArtifactVersionRepository::create(
        &ctx.pool,
        CreateArtifactVersionInput {
            artifact: id,
            execution: Some(execution),
            content_type: Some("application/json".into()),
            content: None,
            content_json: Some(payload.clone()),
            file_path: None,
            meta: Some(payload),
            created_by: None,
        },
    )
    .await?;
    for path in [
        format!("/api/v1/artifacts?execution={execution}"),
        format!("/api/v1/executions/{execution}/artifacts"),
        format!("/api/v1/artifacts/{id}/versions"),
    ] {
        let (sample, body) = measure(&ctx, &path).await?;
        assert_eq!(sample.status, 200, "{body}");
        assert_eq!(
            sample.queries.payload_projection_reads, 0,
            "payload selected by summary route {path}"
        );
        assert!(
            sample.response_bytes < 4096,
            "summary contains fixture content"
        );
    }
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn missing_linked_executions_deny_logs_instead_of_disappearing_from_a_batch() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let execution = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, execution).await?;
    link(&ctx, log, i64::MAX).await?;
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}"), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let list: Value = ctx
        .get(
            &format!("/api/v1/executions/{execution}/artifacts"),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert!(list["items"]
        .as_array()
        .ok_or("artifact array missing")?
        .is_empty());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn disclosure_repository_failures_are_not_successful_empty_lists_or_detail_404s() -> Result<()>
{
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let execution = execution(&ctx, &action_ref).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.run.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, execution).await?;
    // This physical clone is exclusively owned by the test. Renaming its table
    // injects a real repository failure after visibility succeeds, without mocks.
    sqlx::query("ALTER TABLE execution_secret_value RENAME TO fixture_unavailable_secret_values")
        .execute(&ctx.pool)
        .await?;
    for path in [
        format!("/api/v1/artifacts/{log}"),
        format!("/api/v1/executions/{execution}/artifacts"),
    ] {
        assert_eq!(
            ctx.get(&path, ctx.token()).await?.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn execution_pages_count_disclosure_eligible_rows_after_a_denied_candidate_batch(
) -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let execution = execution(&ctx, &action_ref).await?;
    let progress = artifact(
        &ctx,
        "artifact_perf.progress",
        ArtifactClassification::General,
        Some(json!([])),
    )
    .await?;
    link(&ctx, progress, execution).await?;
    let mut expected = vec![progress];
    for n in 0..2 {
        let id = artifact(
            &ctx,
            &format!("artifact_perf.file{n}"),
            ArtifactClassification::General,
            None,
        )
        .await?;
        link(&ctx, id, execution).await?;
        expected.push(id);
    }
    // A whole first scan batch can be SQL-visible but fail runtime disclosure.
    // No offset/limit may be applied before that gate or reused as an exact total.
    for n in 0..260 {
        let id = artifact(
            &ctx,
            &format!("artifact_perf.denied{n}.stdout.log"),
            ArtifactClassification::RuntimeLog,
            None,
        )
        .await?;
        link(&ctx, id, execution).await?;
        link(&ctx, id, i64::MAX).await?;
    }
    expected.reverse();
    for (offset, id) in expected.iter().enumerate() {
        let response = ctx
            .get(
                &format!(
                    "/api/v1/executions/{execution}/artifacts?page={}&per_page=1",
                    offset + 1
                ),
                ctx.token(),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        assert_eq!(
            body["items"].as_array().ok_or("page items missing")?.len(),
            1
        );
        assert_eq!(body["items"][0]["id"], *id);
        assert_eq!(body["pagination"]["total_items"], 3);
        assert_eq!(body["pagination"]["total_pages"], 3);
        assert_eq!(body["pagination"]["has_next"], offset < 2);
    }
    let body: Value = ctx
        .get(
            &format!("/api/v1/executions/{execution}/artifacts?page=4&per_page=1"),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert!(body["items"]
        .as_array()
        .ok_or("page items missing")?
        .is_empty());
    assert_eq!(body["pagination"]["total_items"], 3);
    let body: Value = ctx
        .get(
            &format!("/api/v1/executions/{execution}/artifacts?type=progress&per_page=1"),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert_eq!(
        body["items"][0]["id"], progress,
        "filtered progress discovery must not depend on page one of all artifacts"
    );
    assert_eq!(body["pagination"]["total_items"], 1);
    let body: Value = ctx
        .get(
            &format!("/api/v1/artifacts?execution={execution}&per_page=1"),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert_eq!(
        body["pagination"]["total_items"], 263,
        "global-list totals remain visibility-only"
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn history_pages_count_visible_versions_and_exclude_hidden_newer_versions() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let action_ref = action(&ctx).await?;
    let visible = execution(&ctx, &action_ref).await?;
    let hidden = execution(&ctx, &action_ref).await?;
    let id = artifact(
        &ctx,
        "artifact_perf.history",
        ArtifactClassification::General,
        None,
    )
    .await?;
    for execution in [visible, hidden, visible, hidden] {
        link(&ctx, id, execution).await?;
    }
    sqlx::query("UPDATE permission_set SET grants = $1 WHERE ref = 'fixture.artifact_reader'")
        .bind(json!([
            {"resource":"actions","actions":["read","execute"]},
            {"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[visible]}}
        ]))
        .execute(&ctx.pool)
        .await?;
    AuthorizationService::invalidate_identity_authz_cache(
        ctx.user.as_ref().ok_or("fixture identity missing")?.id,
    )
    .await;
    AuthorizationService::invalidate_permission_set_caches().await;
    for (page, version) in [(1, 3), (2, 1)] {
        let response = ctx
            .get(
                &format!("/api/v1/artifacts/{id}/versions?page={page}&per_page=1"),
                ctx.token(),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        assert_eq!(body["items"][0]["version"], version);
        assert_eq!(body["pagination"]["total_items"], 2);
        assert_eq!(body["pagination"]["has_next"], page == 1);
    }
    let latest: Value = ctx
        .get(
            &format!("/api/v1/artifacts/{id}/versions/latest"),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert_eq!(
        latest["data"]["version"], 3,
        "latest-readable selection is unchanged"
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn artifact_page_boundaries_reject_invalid_sizes_and_overflow() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    for base in [
        "/api/v1/artifacts",
        "/api/v1/executions/1/artifacts",
        "/api/v1/artifacts/1/versions",
    ] {
        for query in [
            "page=0",
            "per_page=0",
            "per_page=101",
            "page=4294967295&per_page=100",
        ] {
            assert_eq!(
                ctx.get(&format!("{base}?{query}"), ctx.token())
                    .await?
                    .status(),
                StatusCode::BAD_REQUEST,
                "invalid artifact page boundary: {base}?{query}"
            );
        }
    }
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn cancellation_keeps_bounded_owner_checks_without_a_read_anchor() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let identity = ctx.user.as_ref().ok_or("fixture identity missing")?.id;
    grant(
        &ctx,
        "fixture.cancel_self",
        json!([
            {"resource":"executions","actions":["cancel"],"constraints":{"execution_scope":"self"}}
        ]),
    )
    .await?;
    let row = ExecutionRepository::create(
        &ctx.pool,
        CreateExecutionInput {
            action_ref: "private.cancel_target".into(),
            executor: Some(identity),
            ..Default::default()
        },
    )
    .await?;
    // Read/decrypt must reject this malformed anchor, but cancellation has
    // always used its own target plus a bounded 64-parent owner check.
    sqlx::query("UPDATE execution SET parent = id WHERE id = $1")
        .bind(row.id)
        .execute(&ctx.pool)
        .await?;
    let response = ctx
        .post(
            &format!("/api/v1/executions/{}/cancel", row.id),
            json!({}),
            ctx.token(),
        )
        .await?;
    let status = response.status();
    let body: Value = response.json().await?;
    ctx.cleanup().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "cancelled");
    Ok(())
}
