//! Real-route authorization characterization and query-growth gates.
//! SQLx events are observed only while polling the request future; the threaded
//! audit writer is deliberately outside that subscriber. No SQL or binds are saved.
//! Budget fixtures bypass shared authorization metadata caches per app state:
//! every sample counts real RBAC reads, independent of TTL or neighboring tests.

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

// Normalized statements from the pinned SQLx PostgreSQL driver's describe.rs.
// Do not classify by table name: application catalog reads must still count.
const DRIVER_TYPE_BY_OID: &str = "select oid::regtype::text, typtype, typcategory, typrelid, typelem, typbasetype from pg_catalog.pg_type where oid = $1";
const DRIVER_ARRAY_TYPE: &str =
    "select oid, typarray from pg_catalog.pg_type where oid = $1::regtype::oid";
const DRIVER_TYPE_BY_NAME: &str = "select $1::regtype::oid";
const DRIVER_ENUM_BY_OID: &str =
    "select enumlabel from pg_catalog.pg_enum where enumtypid = $1 order by enumsortorder";
const POOL_SESSION_SETUP: &str =
    "select set_config('search_path', $1, false), set_config('application_name', $2, false)";

// These are selected-CONTENT ceilings, not relaxed metadata or driver exclusions.
// PG18's first selected-route run measured 30 application statements for both
// 1/64 execution origins, versus the inherited aggregate-metadata ceiling of 20.
// The fixed guarded route now includes three advisory locks, one BEGIN, three
// retained-current-IAM reads, two version revalidation/pinning reads, and one
// sensor-generation dispatch probe (10 statements). Keep all of them counted.
// The subsequent structural-owner survival check adds at most four statements
// for these action-owned fixtures: bound artifact, action, pack, and pack owner.
// A duplicate initial IAM snapshot/selection is still an optimization candidate,
// not permission to remove the held-current-IAM/source/target checks.
const SELECTED_CONTENT_FIXED_GUARD_QUERIES: usize = 10;
const SELECTED_CONTENT_OWNER_SURVIVAL_QUERIES: usize = 4;

fn selected_content_query_ceiling(disclosure_ceiling: usize) -> usize {
    disclosure_ceiling
        + SELECTED_CONTENT_FIXED_GUARD_QUERIES
        + SELECTED_CONTENT_OWNER_SURVIVAL_QUERIES
}

fn assert_selected_content_query_safety(queries: &QueryMetrics) {
    assert_eq!(
        queries.artifact_count_reads, 0,
        "content selected an exact artifact total"
    );
    assert_eq!(
        queries.artifact_linked_history_reads, 0,
        "content enumerated unrelated linked history"
    );
    assert_eq!(
        queries.ciphertext_reads, 0,
        "disclosure preparation selected ciphertext"
    );
    assert_eq!(
        queries.source_payload_reads, 0,
        "disclosure preparation selected source payloads"
    );
    // The log contains two IAM loads (preselection + held current IAM). Do not
    // let the explicit fixed-work allowance hide another full snapshot reload.
    for category in ["permission_set", "identity_role_assignment"] {
        assert!(
            queries
                .statement_categories
                .get(category)
                .copied()
                .unwrap_or(0)
                <= 2,
            "selected content repeated an IAM snapshot: {category}"
        );
    }
}

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
    artifact_count_reads: usize,
    artifact_linked_history_reads: usize,
    pool_acquisitions: usize,
    pool_acquire_seconds: f64,
    execution_disclosure_poll_cpu_seconds: Option<f64>,
}

impl QueryMetrics {
    fn driver_metadata_reads(&self) -> usize {
        [
            "driver_type_discovery",
            "driver_enum_discovery",
            "driver_type_name_discovery",
        ]
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
        self.statements
            - self.driver_metadata_reads()
            - self
                .statement_categories
                .get("pool_session_setup")
                .copied()
                .unwrap_or(0)
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
        if sql.contains("count(") && matches!(primary_table, Some("artifact" | "artifact_version"))
        {
            self.artifact_count_reads += 1;
        }
        if sql.starts_with("select distinct execution from artifact_version")
            || sql.starts_with("select distinct artifact, execution from artifact_version")
        {
            self.artifact_linked_history_reads += 1;
        }
        // Keep diagnostics independent of values and SQL text. In particular,
        // distinguish cache misses from repeated graph/disclosure reads.
        let category = match primary_table {
            _ if matches!(sql.as_str(), DRIVER_TYPE_BY_OID | DRIVER_ARRAY_TYPE) => {
                "driver_type_discovery"
            }
            _ if sql == DRIVER_TYPE_BY_NAME => "driver_type_name_discovery",
            _ if sql == DRIVER_ENUM_BY_OID => "driver_enum_discovery",
            _ if sql == POOL_SESSION_SETUP => "pool_session_setup",
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

/// SQLx puts complete short queries in `summary` and leaves `db.statement`
/// empty. Resolve that one known discovery shape after all event fields arrive.
/// No query text is retained, and unknown summaries are never excluded.
struct QueryEvent<'a> {
    metrics: &'a mut QueryMetrics,
    empty_statement: bool,
    named_type_summary: bool,
}

impl QueryEvent<'_> {
    fn finish(self) {
        if self.empty_statement {
            let category = if self.named_type_summary {
                "driver_type_name_discovery"
            } else {
                "empty_statement"
            };
            *self
                .metrics
                .statement_categories
                .entry(category.into())
                .or_default() += 1;
        }
    }
}

impl Visit for QueryEvent<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "summary" => {
                self.named_type_summary = value
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .eq_ignore_ascii_case(DRIVER_TYPE_BY_NAME);
            }
            "db.statement" => {
                self.empty_statement = value.trim().is_empty();
                if !self.empty_statement {
                    self.metrics.record_str(field, value);
                }
            }
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.metrics.record_u64(field, value);
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.metrics.record_f64(field, value);
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
            let mut query = QueryEvent {
                metrics: &mut metrics,
                empty_statement: false,
                named_type_summary: false,
            };
            event.record(&mut query);
            query.finish();
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
            db.statement = DRIVER_TYPE_BY_OID);
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            db.statement = DRIVER_ENUM_BY_OID);
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
fn query_budget_excludes_only_exact_pool_setup_and_preserves_application_limit() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..25 {
            tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
                db.statement = "SELECT id FROM execution WHERE id = $1");
        }
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            db.statement = DRIVER_TYPE_BY_OID);
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            db.statement = DRIVER_ENUM_BY_OID);
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            db.statement = POOL_SESSION_SETUP);
        let metrics = observer.0.lock().unwrap().clone();
        assert_eq!(metrics.statements, 28);
        assert_eq!(metrics.application_statements(), 25);
        // Other session changes are still application work, not pool setup.
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            db.statement = "SELECT set_config('statement_timeout', $1, true)");
        let metrics = observer.0.lock().unwrap();
        assert_eq!(metrics.statements, 29);
        assert_eq!(metrics.application_statements(), 26);
        assert!(metrics.application_statements() > 25);
    });
}

#[test]
fn query_observer_counts_exact_artifact_totals_and_both_linked_history_shapes() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        for sql in [
            "SELECT COUNT(*) FROM artifact WHERE visibility = $1",
            "SELECT COUNT(*) FROM artifact_version WHERE artifact = $1",
            "SELECT DISTINCT execution FROM artifact_version WHERE artifact = $1",
            "SELECT DISTINCT artifact, execution FROM artifact_version WHERE artifact = ANY($1)",
            "SELECT id, execution FROM artifact_version WHERE artifact = $1 AND version = $2",
        ] {
            tracing::event!(target: "sqlx::query", tracing::Level::DEBUG, db.statement = sql);
        }
    });
    let metrics = observer.0.lock().unwrap();
    assert_eq!(metrics.statements, 5);
    assert_eq!(metrics.artifact_count_reads, 2);
    assert_eq!(metrics.artifact_linked_history_reads, 2);
}

#[test]
fn application_catalog_queries_are_not_driver_discovery() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        for sql in [
            "SELECT oid FROM pg_catalog.pg_type WHERE oid = $1",
            "SELECT enumlabel FROM pg_catalog.pg_enum WHERE enumtypid = $1",
            "SELECT enumlabel FROM pg_catalog.pg_enum WHERE enumtypid = $2 ORDER BY enumsortorder",
            "SELECT (SELECT oid FROM pg_catalog.pg_type WHERE oid = $1) FROM execution",
            "SELECT count(*) FROM pg_catalog.pg_enum",
        ] {
            tracing::event!(target: "sqlx::query", tracing::Level::DEBUG, db.statement = sql);
        }
    });
    let metrics = observer.0.lock().unwrap();
    assert_eq!(metrics.statements, 5);
    assert_eq!(metrics.driver_metadata_reads(), 0);
    assert_eq!(metrics.application_statements(), 5);
}

#[test]
fn pinned_driver_query_shapes_survive_whitespace_and_case_normalization() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        for sql in [
            DRIVER_TYPE_BY_OID,
            DRIVER_ARRAY_TYPE,
            DRIVER_ENUM_BY_OID,
            DRIVER_TYPE_BY_NAME,
        ] {
            let sql = sql.to_uppercase().replace(" FROM ", "\nFROM\n");
            tracing::event!(target: "sqlx::query", tracing::Level::DEBUG, db.statement = sql.as_str());
        }
    });
    let metrics = observer.0.lock().unwrap();
    assert_eq!(metrics.statements, 4);
    assert_eq!(metrics.driver_metadata_reads(), 4);
    assert_eq!(metrics.application_statements(), 0);
}

#[test]
fn short_driver_summary_is_classified_once_in_either_field_order() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        let summary = String::from("SELECT $1::regtype::oid");
        let sql = String::new();
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            summary, db.statement = sql, rows_returned = 1u64);
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            db.statement = "", summary = "select $1::regtype::oid", rows_returned = 1u64);
    });
    let metrics = observer.0.lock().unwrap();
    assert_eq!(metrics.statements, 2);
    assert_eq!(metrics.returned_rows, 2);
    assert_eq!(metrics.driver_metadata_reads(), 2);
    assert_eq!(metrics.application_statements(), 0);
    assert_eq!(metrics.statement_categories.len(), 1);
}

#[test]
fn short_summary_fallback_does_not_hide_application_queries() {
    let observer = RequestQueries::default();
    let subscriber = tracing_subscriber::registry().with(observer.clone());
    tracing::subscriber::with_default(subscriber, || {
        for summary in [
            "",
            "SELECT 1",
            "SELECT $1::regtype::oid ...",
            "SELECT $2::regtype::oid",
        ] {
            tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
                summary = summary, db.statement = "");
        }
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            summary = DRIVER_TYPE_BY_NAME, db.statement = "SELECT id FROM execution WHERE id = $1");
        // A summary alone is not evidence that the statement field was empty.
        tracing::event!(target: "sqlx::query", tracing::Level::DEBUG,
            summary = DRIVER_TYPE_BY_NAME);
    });
    let metrics = observer.0.lock().unwrap();
    assert_eq!(metrics.statements, 6);
    assert_eq!(metrics.driver_metadata_reads(), 0);
    assert_eq!(metrics.application_statements(), 6);
    assert_eq!(metrics.graph_record_reads, 1);
    assert_eq!(
        metrics.statement_categories.get("empty_statement"),
        Some(&4)
    );
}

#[tokio::test]
async fn uncached_authorization_counts_reads_after_warmup_and_global_invalidation() -> Result<()> {
    use attune_api::auth::{jwt::validate_token, middleware::AuthenticatedUser};
    use attune_common::repositories::{FindByRef, Update};
    let ctx = TestContext::new().await?.with_auth().await?;
    let _ = action(&ctx).await?;
    let user = AuthenticatedUser {
        claims: validate_token(
            ctx.token().ok_or("fixture token missing")?,
            &ctx.state.jwt_config,
        )?,
    };
    let shared = ctx.state.authorization_service();
    let expected = shared
        .load_snapshot(&user)
        .await?
        .ok_or("snapshot missing")?;
    let uncached = shared.clone().without_metadata_cache();
    let mut counts = Vec::new();
    for invalidate in [false, true] {
        if invalidate {
            AuthorizationService::invalidate_identity_authz_cache(expected.identity_id).await;
            AuthorizationService::invalidate_permission_set_caches().await;
        }
        let observer = RequestQueries::default();
        let subscriber = tracing_subscriber::registry().with(observer.clone());
        let snapshot = uncached
            .load_snapshot(&user)
            .with_subscriber(subscriber)
            .await?
            .ok_or("snapshot missing")?;
        assert_eq!(snapshot.grants, expected.grants);
        assert_eq!(snapshot.identity_attributes, expected.identity_attributes);
        let metrics = observer.0.lock().unwrap().clone();
        assert!(
            metrics
                .statement_categories
                .get("permission_set")
                .copied()
                .unwrap_or(0)
                > 0
        );
        assert!(
            metrics
                .statement_categories
                .get("identity_role_assignment")
                .copied()
                .unwrap_or(0)
                > 0
        );
        counts.push(metrics.application_statements());
    }
    assert_eq!(counts[0], counts[1]);
    let requested = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.requested".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"actions","actions":["read"]}]),
        },
    )
    .await?;
    let refs = vec![requested.r#ref.clone()];
    assert!(shared.can_delegate_permission_sets(&user, &refs).await?);
    assert!(uncached.can_delegate_permission_sets(&user, &refs).await?);
    PermissionSetRepository::update(
        &ctx.pool,
        requested.id,
        attune_common::repositories::identity::UpdatePermissionSetInput {
            grants: Some(json!([{"resource":"permissions","actions":["manage"]}])),
            ..Default::default()
        },
    )
    .await?;
    assert!(
        !uncached.can_delegate_permission_sets(&user, &refs).await?,
        "by-reference delegation must observe changed grants without invalidation"
    );
    // Exercise the shared-cache path without assuming a TTL or simultaneous hit.
    let _ = shared.load_snapshot(&user).await?;
    let set = PermissionSetRepository::find_by_ref(&ctx.pool, "fixture.artifact_reader")
        .await?
        .ok_or("permission fixture missing")?;
    PermissionSetRepository::update(
        &ctx.pool,
        set.id,
        attune_common::repositories::identity::UpdatePermissionSetInput {
            grants: Some(json!([])),
            ..Default::default()
        },
    )
    .await?;
    assert!(uncached
        .load_snapshot(&user)
        .await?
        .ok_or("snapshot missing")?
        .grants
        .is_empty());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn cold_filtered_artifact_requests_account_for_enum_name_discovery() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    let reference = action(&ctx).await?;
    let execution = execution(&ctx, &reference).await?;
    let log = artifact(
        &ctx,
        "artifact_perf.filtered.stdout.log",
        ArtifactClassification::RuntimeLog,
        None,
    )
    .await?;
    link(&ctx, log, execution).await?;
    ctx.flush_audit().await?;
    let mut parked = Vec::new();
    for _ in 0..ctx.pool.size() {
        parked.push(ctx.pool.acquire().await?);
    }
    for connection in parked {
        connection.close().await?;
    }
    let (sample, body) = measure(
        &ctx,
        &format!("/api/v1/artifacts?execution={execution}&classification=runtime_log&type=file_text&visibility=private"),
    ).await?;
    assert_eq!(sample.status, 200, "{body}");
    assert_eq!(
        body["items"]
            .as_array()
            .ok_or("artifact array missing")?
            .len(),
        1
    );
    assert!(
        sample
            .queries
            .statement_categories
            .get("driver_type_name_discovery")
            .copied()
            .unwrap_or(0)
            > 0,
        "filtered route must exercise enum-name lookup: {}",
        serde_json::to_string(&sample)?
    );
    assert!(
        sample.queries.application_statements() <= 25,
        "{}",
        serde_json::to_string(&sample)?
    );
    ctx.cleanup().await?;
    Ok(())
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

async fn assert_safe_runtime_log_header(ctx: &TestContext, artifact: i64) -> Result<()> {
    let response = ctx
        .get(&format!("/api/v1/artifacts/{artifact}"), ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    let header = body["data"].as_object().ok_or("header missing")?;
    assert_eq!(header.get("id"), Some(&json!(artifact)));
    assert_eq!(header.get("classification"), Some(&json!("runtime_log")));
    for key in header.keys() {
        assert!(
            [
                "id",
                "ref",
                "scope",
                "owner",
                "type",
                "visibility",
                "classification",
                "name",
                "created",
                "updated",
                "retention_policy",
                "retention_limit"
            ]
            .contains(&key.as_str()),
            "runtime-log header exposed non-allowlisted field {key}"
        );
    }
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
        let path = format!("/api/v1/artifacts/{log}/versions/1");
        assert_safe_runtime_log_header(&ctx, log).await?;
        let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
        if count == 64 {
            // Exercise connection-local type discovery without background audit
            // work warming replacement connections outside the measurement.
            ctx.flush_audit().await?;
            let mut parked = Vec::new();
            for _ in 0..ctx.pool.size() {
                parked.push(ctx.pool.acquire().await?);
            }
            for connection in parked {
                connection.close().await?;
            }
            // Complete session setup before measurement, but leave SQLx's
            // connection-local enum metadata cold on every replacement.
            let mut replacements = Vec::new();
            for _ in 0..ctx.pool.options().get_max_connections() {
                replacements.push(ctx.pool.acquire().await?);
            }
            drop(replacements);
        }
        let (sample, body) = measure(&ctx, &path).await?;
        if count == 64 {
            assert!(
                sample.queries.driver_metadata_reads() > 0,
                "Fresh-connection regression did not exercise SQLx type discovery"
            );
        }
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
    let total = measurements[1].1.queries.application_statements();
    assert_eq!(
        measurements[0].1.queries.application_statements(),
        total,
        "selected execution-origin work must stay constant within one frontier batch"
    );
    for (_, sample) in &measurements {
        assert_selected_content_query_safety(&sample.queries);
    }
    ctx.cleanup().await?;
    assert!(
        large == small && total <= selected_content_query_ceiling(20),
        "graph-record reads grew from {small} to {large}, or selected-content application total {total} exceeded its disclosure + fixed-guard ceiling; {report}"
    );
    Ok(())
}

#[tokio::test]
async fn shared_origin_depth_budget_and_cycles_are_checked_on_every_traversal() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let path = format!("/api/v1/artifacts/{log}/versions/1");
    assert_safe_runtime_log_header(&ctx, log).await?;
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let path = format!("/api/v1/artifacts/{log}/versions/1");
    assert_safe_runtime_log_header(&ctx, log).await?;
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
            ctx.get(&format!("/api/v1/artifacts/{log}/versions/1"), ctx.token())
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
        ctx.get(&format!("/api/v1/artifacts/{log}/versions/1"), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn runtime_log_metadata_and_selected_body_never_load_unrelated_history() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let mut selected_samples = Vec::new();
    let mut already_created = 0;
    let mut selected_execution = None;
    for retained in [1, 256, 257, 1000] {
        let mut latest = 0;
        for _ in already_created..retained {
            latest = execution(&ctx, &action_ref).await?;
            selected_execution.get_or_insert(latest);
            link(&ctx, stdout, latest).await?;
            link(&ctx, stderr, latest).await?;
        }
        already_created = retained;
        let selected_execution = selected_execution.ok_or("selected execution missing")?;
        let path = format!("/api/v1/executions/{selected_execution}/artifacts");
        ArtifactRepository::analyze_read_fixtures(&ctx.pool).await?;
        let _ = ctx.get(&path, ctx.token()).await?.bytes().await?;
        // Driver enum metadata is connection-local. Exercise pool churn so the
        // application-query gate cannot accidentally depend on warmed sockets.
        if retained == 1000 {
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
        if retained == 1000 {
            assert!(
                sample.queries.driver_metadata_reads() > 0,
                "Fresh-connection regression did not exercise SQLx type discovery"
            );
        }
        assert_eq!(sample.status, 200, "{body}");
        assert_eq!(sample.queries.artifact_count_reads, 0);
        assert_eq!(sample.queries.artifact_linked_history_reads, 0);
        assert_eq!(sample.queries.source_scope_reads, 0);
        for category in [
            "execution_secret_value",
            "runtime_schema_binding",
            "execution_key_read_origin",
        ] {
            assert_eq!(
                sample
                    .queries
                    .statement_categories
                    .get(category)
                    .copied()
                    .unwrap_or(0),
                0,
                "metadata loaded disclosure evidence: {category}"
            );
        }
        assert_eq!(
            body["items"]
                .as_array()
                .ok_or("artifact array missing")?
                .len(),
            2
        );
        for item in body["items"].as_array().ok_or("items missing")? {
            assert_eq!(item["selected_version"]["execution"], selected_execution);
            assert_eq!(item["selected_version"]["version"], 1);
        }
        let (selected, content) =
            measure(&ctx, &format!("/api/v1/artifacts/{stdout}/versions/1")).await?;
        assert_eq!(selected.status, 200, "{content}");
        assert_eq!(content["data"]["version"], 1);
        assert_eq!(selected.queries.artifact_linked_history_reads, 0);
        assert_eq!(selected.queries.ciphertext_reads, 0);
        selected_samples.push((retained, selected));
        for metadata_path in [
            format!("/api/v1/artifacts/{stdout}"),
            format!("/api/v1/artifacts/{stdout}/versions?per_page=20"),
        ] {
            let (metadata, body) = measure(&ctx, &metadata_path).await?;
            assert_eq!(metadata.status, 200, "{body}");
            assert_eq!(metadata.queries.artifact_count_reads, 0);
            assert_eq!(metadata.queries.artifact_linked_history_reads, 0);
            assert_eq!(metadata.queries.source_scope_reads, 0);
            assert_eq!(
                metadata
                    .queries
                    .statement_categories
                    .get("execution_secret_value")
                    .copied()
                    .unwrap_or(0),
                0
            );
            assert_eq!(
                metadata
                    .queries
                    .statement_categories
                    .get("runtime_schema_binding")
                    .copied()
                    .unwrap_or(0),
                0
            );
            assert_eq!(metadata.queries.payload_projection_reads, 0);
        }
        samples.push((retained, sample));
        if retained == 1000 {
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
    // Only the application scaling budget excludes catalog type discovery and
    // the exact per-connection setup statement; RBAC cache misses still count.
    let small = samples[0].1.queries.application_statements();
    let large = samples
        .last()
        .ok_or("samples missing")?
        .1
        .queries
        .application_statements();
    let selected_small = selected_samples[0].1.queries.graph_record_reads;
    source_origins::save_report("runtime-log-selected-body", &selected_samples)?;
    for (_, sample) in &selected_samples {
        assert_eq!(
            sample.queries.graph_record_reads, selected_small,
            "fixed selected body loaded more execution roots as sibling history grew"
        );
        assert_selected_content_query_safety(&sample.queries);
        assert_eq!(
            sample.queries.application_statements(),
            selected_samples[0].1.queries.application_statements(),
            "fixed selected body application queries grew with unrelated siblings"
        );
        // The no-origin body measured 25 at 1/256/257/1000 siblings on PG18.
        // Only the four owner-survival statements are additional here. Metadata
        // below retains its original <=20 ceiling and zero-disclosure contract.
        assert!(sample.queries.application_statements() <= 25 + SELECTED_CONTENT_OWNER_SURVIVAL_QUERIES,
            "fixed selected body exceeded its measured guarded baseline plus owner-survival work: {}",
            serde_json::to_string(sample)?);
    }
    ctx.cleanup().await?;
    assert!(
        large == small && large <= 20,
        "metadata queries grew from {small} to {large} with unrelated history; {report}"
    );
    Ok(())
}

#[tokio::test]
async fn runtime_log_metadata_visibility_does_not_bypass_selected_content_disclosure() -> Result<()>
{
    use attune_common::repositories::{key::KeyRepository, Delete};
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let key = attune_common::repositories::key::KeyRepository::create_encrypted_in_pool(
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
            value: json!("fixture material"),
        },
        ctx.state
            .config
            .security
            .encryption_key
            .as_deref()
            .ok_or("fixture key missing")?,
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
    assert_safe_runtime_log_header(&ctx, log).await?;
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}/versions/1"), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let list: Value = ctx
        .get(&format!("/api/v1/executions/{id}/artifacts"), ctx.token())
        .await?
        .json()
        .await?;
    assert_eq!(list["items"][0]["id"], log);
    assert_eq!(list["items"][0]["selected_version"]["execution"], id);
    grant(&ctx, "fixture.origin_read", json!([
        {"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":["system.fixture_private"],"owner_types":["system"]}}
    ])).await?;
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}/versions/1"), ctx.token())
            .await?
            .status(),
        StatusCode::OK
    );
    assert!(KeyRepository::delete(&ctx.pool, key.id).await?);
    assert_safe_runtime_log_header(&ctx, log).await?;
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}/versions/1"), ctx.token())
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
async fn missing_sibling_execution_denies_its_content_not_readable_selected_content() -> Result<()>
{
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    assert_safe_runtime_log_header(&ctx, log).await?;
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}/versions/2"), ctx.token())
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}/versions/1"), ctx.token())
            .await?
            .status(),
        StatusCode::OK
    );
    let list: Value = ctx
        .get(
            &format!("/api/v1/executions/{execution}/artifacts"),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert_eq!(list["items"][0]["id"], log);
    assert_eq!(list["items"][0]["selected_version"]["version"], 1);
    assert_eq!(list["items"][0]["selected_version"]["execution"], execution);
    // A metadata-visible later run with denied provenance is different from the
    // missing/invisible version above: latest must select it and deny its body,
    // never scan older history for a body that happens to be readable.
    let denied = self::execution(&ctx, &action_ref).await?;
    secret_origins(&ctx, denied, vec![execution_origin(i64::MAX)]).await?;
    link(&ctx, log, denied).await?;
    assert_safe_runtime_log_header(&ctx, log).await?;
    let history: Value = ctx
        .get(&format!("/api/v1/artifacts/{log}/versions"), ctx.token())
        .await?
        .json()
        .await?;
    assert_eq!(
        history["items"][0]["version"], 3,
        "version metadata visibility is not a body-disclosure promise"
    );
    for path in [
        format!("/api/v1/artifacts/{log}/versions/latest"),
        format!("/api/v1/artifacts/{log}/download"),
        format!("/api/v1/artifacts/{log}/versions/3/download"),
    ] {
        assert_eq!(
            ctx.get(&path, ctx.token()).await?.status(),
            StatusCode::NOT_FOUND,
            "latest must not fall back to a readable sibling: {path}"
        );
    }
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}/versions/1"), ctx.token())
            .await?
            .status(),
        StatusCode::OK
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn selected_disclosure_repository_failures_are_not_masked_and_metadata_still_works(
) -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    assert_eq!(
        ctx.get(&format!("/api/v1/artifacts/{log}/versions/1"), ctx.token())
            .await?
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_safe_runtime_log_header(&ctx, log).await?;
    let response = ctx
        .get(
            &format!("/api/v1/executions/{execution}/artifacts"),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let list: Value = response.json().await?;
    assert_eq!(list["items"][0]["id"], log);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn execution_cursor_pages_include_safe_metadata_without_scanning_denied_bodies() -> Result<()>
{
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    // SQL-visible log metadata remains pageable even when sibling bodies deny.
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
        expected.push(id);
    }
    expected.reverse();
    let mut cursor = None::<String>;
    for (page, expected_items) in expected.chunks(100).enumerate() {
        let mut path = format!("/api/v1/executions/{execution}/artifacts?per_page=100");
        if let Some(cursor) = &cursor {
            path.push('&');
            path.push_str(
                &url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("cursor", cursor)
                    .finish(),
            );
        }
        let response = ctx.get(&path, ctx.token()).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        let items = body["items"].as_array().ok_or("page items missing")?;
        assert_eq!(items.len(), expected_items.len());
        assert_eq!(
            items
                .iter()
                .map(|item| item["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            expected_items
        );
        assert!(body.get("pagination").is_none());
        assert!(body.get("total_items").is_none());
        let has_more = (page + 1) * 100 < expected.len();
        assert_eq!(body["has_more"], has_more);
        cursor = body["next_cursor"].as_str().map(str::to_string);
        assert_eq!(cursor.is_some(), has_more);
        for item in items {
            assert_eq!(item["selected_version"]["execution"], execution);
        }
    }
    assert!(cursor.is_none());
    let body: Value = ctx
        .get(
            &format!("/api/v1/executions/{execution}/artifacts?type=url&per_page=1"),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert!(body["items"]
        .as_array()
        .ok_or("page items missing")?
        .is_empty());
    assert_eq!(body["has_more"], false);
    assert!(body["next_cursor"].is_null());
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
    assert_eq!(body["has_more"], false);
    let body: Value = ctx
        .get(
            &format!("/api/v1/artifacts?execution={execution}&per_page=1"),
            ctx.token(),
        )
        .await?
        .json()
        .await?;
    assert_eq!(body["items"].as_array().ok_or("items missing")?.len(), 1);
    assert_eq!(body["has_more"], true);
    assert!(body.get("pagination").is_none());
    assert!(body.get("total_items").is_none());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn history_cursor_pages_exclude_hidden_newer_versions_without_exact_totals() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
    let mut cursor = None::<String>;
    for version in [3, 1] {
        let mut path = format!("/api/v1/artifacts/{id}/versions?per_page=1");
        if let Some(cursor) = &cursor {
            path.push('&');
            path.push_str(
                &url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("cursor", cursor)
                    .finish(),
            );
        }
        let response = ctx.get(&path, ctx.token()).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        assert_eq!(body["items"][0]["version"], version);
        assert!(body.get("pagination").is_none());
        assert!(body.get("total_items").is_none());
        assert_eq!(body["has_more"], version == 3);
        cursor = body["next_cursor"].as_str().map(str::to_string);
        assert_eq!(cursor.is_some(), version == 3);
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
async fn artifact_cursor_pages_reject_invalid_sizes_and_malformed_cursors() -> Result<()> {
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
    for base in [
        "/api/v1/artifacts",
        "/api/v1/executions/1/artifacts",
        "/api/v1/artifacts/1/versions",
    ] {
        for query in [
            "per_page=0",
            "per_page=101",
            "per_page=4294967295",
            "cursor=not-a-valid-cursor",
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
    let ctx = TestContext::new_without_authorization_metadata_cache()
        .await?
        .with_auth()
        .await?;
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
