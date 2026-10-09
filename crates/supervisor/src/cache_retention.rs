//! Cache subsystem retention, freshness, and lifecycle maintenance.
//!
//! Runs as a bounded, distinct step inside the existing supervisor retention
//! cycle (see `main.rs`'s `run_retention_cycle`), reusing its advisory lock
//! and cadence rather than electing a second leader, per the preferred
//! integration shape recorded in `docs/KEY_CACHE.md` ("Gap 1: Supervisor
//! maintenance integration is under-specified"). All cache data access goes
//! through `CacheNamespaceRepository`, `CacheGenerationRepository`, and
//! `CacheEntryRepository` -- this module never issues ad hoc SQL against the
//! cache tables.
//!
//! Lifecycle handled here:
//! - Abandoned unpublished (`staging` or `ready`) generations older than
//!   `staging_expiry_seconds` are marked `failed` so the normal cleanup path
//!   reclaims them.
//! - A tombstoned namespace already moves its in-flight `staging`/`ready`
//!   generations to `failed` and retires its active generation immediately
//!   (see `CacheNamespaceRepository::tombstone`); this module drains those
//!   generations by atomically dropping their entry partitions and metadata,
//!   and once a tombstoned namespace has no generations left,
//!   deletes the namespace row itself. Owner rows stay protected by the
//!   `ON DELETE RESTRICT` foreign keys on `cache_namespace` until that drain
//!   completes.
//! - Active generations and retired-but-still-readable generations are never
//!   selected for cleanup (`CacheGenerationRepository::select_cleanup_candidates`
//!   only returns `failed` rows or `retired` rows whose `readable_until` has
//!   passed); this module additionally re-checks `min_traversal_window_seconds`
//!   defensively before treating a retired generation as eligible.
//! - Freshness and repeated-staging-failure alerts are emitted through the
//!   shared `core.alert` mechanism with bounded, low-cardinality fields only
//!   (numeric IDs, owner type, and counts -- never namespace names, owner
//!   refs, external IDs, or cached values).

use std::{sync::Arc, time::Instant};

use attune_common::{
    config::CacheRetentionConfig,
    models::{CacheGeneration, CacheGenerationState, CacheNamespace, Id, OwnerType},
    mq::Publisher,
    repositories::{
        cache::{
            CacheGenerationCleanupOutcome, CacheStatisticsRefreshOutcome, CacheStorageRepository,
            MAX_CLEANUP_SELECTION,
        },
        CacheGenerationRepository, CacheNamespaceRepository, FindById, MaintenanceRepository,
    },
    system_alert::{emit_core_alert, SystemAlert},
    Error, Result,
};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Generations inspected per namespace for each bounded generation query.
/// Repeated cycles continue draining additional expired unpublished rows.
const GENERATIONS_PER_NAMESPACE_SCAN: i64 = 100;

/// Everything the cache retention step needs to reach the database and emit
/// alerts. Borrowed from the supervisor's long-lived service state each
/// cycle.
pub struct CacheRetentionContext<'a> {
    pub pool: &'a PgPool,
    pub publisher: Option<&'a Publisher>,
    pub service_name: &'a str,
    pub environment: &'a str,
    pub state: Arc<CacheRetentionState>,
}

/// Process-lifetime traversal watermarks for bounded cache maintenance.
///
/// Namespace IDs are monotonically increasing, so an ID keyset can safely
/// survive deletions and tombstones. The scanner wraps to the beginning after
/// reaching the tail, ensuring fixed low-ID prefixes cannot monopolize every
/// cycle.
#[derive(Debug, Default)]
pub struct CacheRetentionState {
    namespace_after_id: Mutex<Option<Id>>,
    cleanup_after_id: Mutex<Option<Id>>,
}

impl CacheRetentionState {
    async fn namespace_after_id(&self) -> Option<Id> {
        *self.namespace_after_id.lock().await
    }

    async fn set_namespace_after_id(&self, after_id: Option<Id>) {
        *self.namespace_after_id.lock().await = after_id;
    }
}

/// Aggregate, bounded-cardinality outcome of one cache retention step.
/// Intentionally carries only counts and a dry-run flag -- never namespace
/// names, owner refs, or external IDs -- so it is always safe to log or audit
/// in full.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheRetentionCycleSummary {
    pub dry_run: bool,
    pub namespaces_scanned: usize,
    pub staging_expired: usize,
    pub cleanup_candidates: usize,
    pub entries_deleted: u64,
    pub bytes_reclaimed: u64,
    pub lock_deferrals: u64,
    pub deadline_deferrals: u64,
    pub cleanup_failures: u64,
    pub maintenance_failures: u64,
    pub registered_partitions: i64,
    pub partitions_created_total: i64,
    pub partitions_dropped_total: i64,
    pub cleanup_backlog_total: i64,
    pub oldest_cleanup_age_seconds: i64,
    pub reclamation_duration_ms: u64,
    pub statistics_duration_ms: u64,
    pub statistics_pending: bool,
    pub storage_observed: bool,
    pub statistics_age_seconds: Option<u64>,
    pub statistics_refreshed: bool,
    pub statistics_lock_deferrals: u64,
    pub statistics_deadline_deferrals: u64,
    pub statistics_failures: u64,
    pub cleanup_budget_exhausted: bool,
    pub generations_deleted: usize,
    pub namespaces_deleted: usize,
    pub freshness_alerts: usize,
    pub staging_failure_alerts: usize,
    pub fresh_namespaces: u64,
    pub stale_namespaces: u64,
    pub namespaces_without_active_generation: u64,
    pub namespace_age_max_seconds: u64,
    pub active_generation_age_max_seconds: u64,
    pub staging_generation_age_max_seconds: u64,
    pub refresh_failures_observed: u64,
    pub records_observed: u64,
    pub storage_bytes_observed: u64,
    pub failed_cleanup_candidates: u64,
    pub expired_snapshot_cleanup_candidates: u64,
    pub cleanup_backlog_saturated: bool,
    pub failed_generations_deleted: usize,
    pub expired_snapshots_deleted: usize,
    pub maintenance_duration_ms: u64,
    scope_metrics: [CacheScopeMetrics; 5],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CacheScopeMetrics {
    namespaces: u64,
    records: u64,
    storage_bytes: u64,
    refresh_failures: u64,
}

impl CacheRetentionCycleSummary {
    /// Whether this cycle changed or would have changed (`dry_run`) any
    /// cache retention state, i.e. whether it is worth an audit entry.
    pub fn had_effect(&self) -> bool {
        self.staging_expired > 0
            || self.entries_deleted > 0
            || self.generations_deleted > 0
            || self.namespaces_deleted > 0
            || self.statistics_refreshed
            || self.statistics_failures > 0
            || self.cleanup_failures > 0
            || self.maintenance_failures > 0
            || self.lock_deferrals > 0
            || self.deadline_deferrals > 0
    }
}

/// Runs one bounded cache retention/freshness step. Intended to be called
/// once per supervisor retention cycle, inside the same advisory lock as
/// runtime row retention.
pub async fn run_cache_retention_cycle(
    ctx: &CacheRetentionContext<'_>,
    config: &CacheRetentionConfig,
) -> Result<CacheRetentionCycleSummary> {
    config
        .validate_storage_maintenance()
        .map_err(Error::validation)?;
    if !config.enabled {
        info!("Cache retention is disabled in configuration; skipping cache cleanup step");
        return Ok(CacheRetentionCycleSummary::default());
    }

    let started = Instant::now();
    let mut summary = CacheRetentionCycleSummary {
        dry_run: config.dry_run,
        ..Default::default()
    };
    let result = async {
        scan_namespaces(ctx, config, &mut summary).await?;
    drain_cleanup_candidates(ctx, config, &mut summary).await?;
    delete_empty_tombstoned_namespaces(ctx, config, &mut summary).await?;

    let statistics_started = Instant::now();
    match CacheStorageRepository::refresh_statistics(ctx.pool, config).await {
        Ok(CacheStatisticsRefreshOutcome::Applied) => summary.statistics_refreshed = true,
        Ok(CacheStatisticsRefreshOutcome::DeferredBusy) => summary.statistics_lock_deferrals += 1,
        Ok(CacheStatisticsRefreshOutcome::DeferredDeadline) => summary.statistics_deadline_deferrals += 1,
        Ok(CacheStatisticsRefreshOutcome::NotDue) => {},
        Err(error) => {
            summary.statistics_failures += 1;
            warn!(error = %error, "Cache statistics maintenance failed; committed cleanup remains recorded");
        }
    }
    summary.statistics_duration_ms = u64::try_from(statistics_started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match CacheStorageRepository::observe(ctx.pool, config).await {
        Ok(observation) => {
            summary.storage_observed = true;
            summary.registered_partitions = observation.registered_partitions;
            summary.partitions_created_total = observation.partitions_created;
            summary.partitions_dropped_total = observation.partitions_dropped;
            summary.cleanup_backlog_total = observation.cleanup_backlog;
            summary.oldest_cleanup_age_seconds = observation.oldest_cleanup_age_seconds;
            summary.statistics_pending = observation.statistics_pending;
            summary.statistics_age_seconds = observation.last_analyzed_at.map(|time| age_seconds(Utc::now(), time));
        }
        Err(error) => {
            summary.maintenance_failures += 1;
            warn!(error = %error, "Cache storage observation failed; committed cleanup remains recorded");
        }
    }
        Ok::<_, attune_common::Error>(())
    }
    .await;

    match result {
        Ok(()) => {}
        Err(err) => {
            warn!(
                component = "cache_maintenance",
                metric_set = "cache_maintenance_cycle",
                status = "failed",
                maintenance_cycle_count = 1u64,
                maintenance_failure_count = 1u64,
                maintenance_duration_ms =
                    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                error = %err,
                "Cache retention step failed"
            );
            // Earlier expiry/drop transactions may already have committed.
            // Return their counters for auditing instead of losing progress.
            summary.maintenance_failures += 1;
        }
    }
    summary.maintenance_duration_ms =
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    emit_operational_metrics(&summary);
    Ok(summary)
}

/// Expires abandoned unpublished generations and emits freshness/failure
/// alerts for each live (non-tombstoned) namespace. Tombstoned namespaces are
/// excluded by `CacheNamespaceRepository::list_metadata` itself, since they
/// have no meaningful freshness and their staging/ready generations are
/// already failed by `tombstone()`.
async fn scan_namespaces(
    ctx: &CacheRetentionContext<'_>,
    config: &CacheRetentionConfig,
    summary: &mut CacheRetentionCycleSummary,
) -> Result<()> {
    let namespace_limit = config
        .max_namespaces_per_cycle
        .clamp(1, MAX_CLEANUP_SELECTION);
    let start_after_id = ctx.state.namespace_after_id().await;
    let (namespaces, next_after_id) =
        load_rotating_namespace_batch(ctx.pool, start_after_id, namespace_limit).await?;
    summary.namespaces_scanned = namespaces.len();

    let staging_cutoff = Utc::now() - bounded_duration(config.staging_expiry_seconds);
    let alert_limit = config.alert_limit_per_cycle.max(0);
    let mut freshness_alerts_emitted = 0i64;
    let mut staging_failure_alerts_emitted = 0i64;

    for loaded_namespace in &namespaces {
        let mut namespace = loaded_namespace.clone();
        let generations = match CacheGenerationRepository::list_for_namespace(
            ctx.pool,
            namespace.id,
            GENERATIONS_PER_NAMESPACE_SCAN,
        )
        .await
        {
            Ok(generations) => generations,
            Err(err) => {
                warn!(
                    namespace_id = namespace.id,
                    error = %err,
                    "Failed to inspect cache namespace generations"
                );
                continue;
            }
        };
        let expired_unpublished = match CacheGenerationRepository::select_expired_unpublished(
            ctx.pool,
            namespace.id,
            staging_cutoff,
            GENERATIONS_PER_NAMESPACE_SCAN,
        )
        .await
        {
            Ok(generations) => generations,
            Err(err) => {
                warn!(
                    namespace_id = namespace.id,
                    error = %err,
                    "Failed to select expired unpublished cache generations"
                );
                continue;
            }
        };
        let mut namespace_changed = false;
        for generation in &expired_unpublished {
            summary.staging_expired += 1;
            if config.dry_run {
                continue;
            }
            if let Err(err) = CacheGenerationRepository::fail(
                ctx.pool,
                generation.id,
                "abandoned unpublished generation exceeded staging_expiry_seconds",
            )
            .await
            {
                warn!(
                    generation_id = generation.id,
                    error = %err,
                    "Failed to expire abandoned staging cache generation"
                );
            } else {
                namespace_changed = true;
            }
        }

        if namespace_changed {
            namespace = CacheNamespaceRepository::find_by_id(ctx.pool, namespace.id)
                .await?
                .ok_or_else(|| {
                    Error::not_found("cache_namespace", "id", namespace.id.to_string())
                })?;
        }

        if let Err(err) =
            observe_namespace_metrics(ctx.pool, &namespace, &generations, summary).await
        {
            warn!(
                namespace_id = namespace.id,
                error = %err,
                "Failed to collect cache namespace operational metrics"
            );
        }

        if !config.freshness_alerts_enabled {
            continue;
        }

        if freshness_alerts_emitted < alert_limit {
            match maybe_emit_freshness_alert(ctx, config, &namespace).await {
                Ok(true) => {
                    summary.freshness_alerts += 1;
                    freshness_alerts_emitted += 1;
                }
                Ok(false) => {}
                Err(err) => warn!(
                    namespace_id = namespace.id,
                    error = %err,
                    "Failed to evaluate cache namespace freshness"
                ),
            }
        }

        if staging_failure_alerts_emitted < alert_limit {
            match maybe_emit_staging_failure_alert(ctx, config, &namespace).await {
                Ok(true) => {
                    summary.staging_failure_alerts += 1;
                    staging_failure_alerts_emitted += 1;
                }
                Ok(false) => {}
                Err(err) => warn!(
                    namespace_id = namespace.id,
                    error = %err,
                    "Failed to evaluate cache namespace staging failure streak"
                ),
            }
        }
    }

    ctx.state.set_namespace_after_id(next_after_id).await;
    Ok(())
}

/// Loads one bounded namespace batch after the current watermark and fills any
/// remaining capacity from the beginning. Wrapped rows are restricted to IDs
/// at or below the starting watermark, preventing duplicates if new rows are
/// inserted between the tail and head queries.
async fn load_rotating_namespace_batch(
    pool: &PgPool,
    start_after_id: Option<Id>,
    limit: i64,
) -> Result<(Vec<CacheNamespace>, Option<Id>)> {
    let first =
        CacheNamespaceRepository::list_metadata_page(pool, None, start_after_id, limit).await?;
    let mut namespaces = first.items;

    if let Some(watermark) = start_after_id {
        let remaining = limit.saturating_sub(namespaces.len() as i64);
        if remaining > 0 {
            let wrapped =
                CacheNamespaceRepository::list_metadata_page(pool, None, None, remaining).await?;
            namespaces.extend(
                wrapped
                    .items
                    .into_iter()
                    .filter(|namespace| namespace.id <= watermark),
            );
        }
    }

    let next_after_id = namespaces.last().map(|namespace| namespace.id);
    Ok((namespaces, next_after_id))
}

async fn observe_namespace_metrics(
    pool: &PgPool,
    namespace: &CacheNamespace,
    generations: &[CacheGeneration],
    summary: &mut CacheRetentionCycleSummary,
) -> Result<()> {
    let now = Utc::now();
    summary.namespace_age_max_seconds = summary
        .namespace_age_max_seconds
        .max(age_seconds(now, namespace.created));
    let scope = &mut summary.scope_metrics[owner_type_index(namespace.owner_type)];
    scope.namespaces = scope.namespaces.saturating_add(1);
    let refresh_failures = u64::try_from(namespace.consecutive_refresh_failures).unwrap_or(0);
    summary.refresh_failures_observed = summary
        .refresh_failures_observed
        .saturating_add(refresh_failures);
    scope.refresh_failures = scope.refresh_failures.saturating_add(refresh_failures);

    for generation in generations {
        let records = u64::try_from(generation.record_count.max(0)).unwrap_or(u64::MAX);
        let bytes = u64::try_from(generation.size_bytes.max(0)).unwrap_or(u64::MAX);
        summary.records_observed = summary.records_observed.saturating_add(records);
        summary.storage_bytes_observed = summary.storage_bytes_observed.saturating_add(bytes);
        scope.records = scope.records.saturating_add(records);
        scope.storage_bytes = scope.storage_bytes.saturating_add(bytes);

        if generation.state == CacheGenerationState::Staging {
            summary.staging_generation_age_max_seconds = summary
                .staging_generation_age_max_seconds
                .max(age_seconds(now, generation.created));
        }
    }

    let Some(active_id) = namespace.active_generation else {
        summary.namespaces_without_active_generation = summary
            .namespaces_without_active_generation
            .saturating_add(1);
        return Ok(());
    };
    let Some(active) = CacheGenerationRepository::find_by_id(pool, active_id).await? else {
        summary.namespaces_without_active_generation = summary
            .namespaces_without_active_generation
            .saturating_add(1);
        return Ok(());
    };
    let Some(activated) = active.activated else {
        summary.namespaces_without_active_generation = summary
            .namespaces_without_active_generation
            .saturating_add(1);
        return Ok(());
    };

    let active_age = age_seconds(now, activated);
    summary.active_generation_age_max_seconds =
        summary.active_generation_age_max_seconds.max(active_age);
    let freshness_target = u64::try_from(namespace.freshness_target_seconds).unwrap_or(0);
    if freshness_target > 0 && active_age > freshness_target {
        summary.stale_namespaces = summary.stale_namespaces.saturating_add(1);
    } else {
        summary.fresh_namespaces = summary.fresh_namespaces.saturating_add(1);
    }
    Ok(())
}

/// Emits a bounded, redacted alert when a namespace's active generation is
/// older than its freshness target plus the configured grace period. Returns
/// `true` only when an alert was actually emitted (not suppressed by
/// cooldown).
async fn maybe_emit_freshness_alert(
    ctx: &CacheRetentionContext<'_>,
    config: &CacheRetentionConfig,
    namespace: &CacheNamespace,
) -> Result<bool> {
    if namespace.freshness_target_seconds == 0 {
        return Ok(false);
    }
    let Some(active_id) = namespace.active_generation else {
        return Ok(false);
    };
    let Some(active) = CacheGenerationRepository::find_by_id(ctx.pool, active_id).await? else {
        return Ok(false);
    };
    let Some(activated) = active.activated else {
        return Ok(false);
    };

    let age_seconds = Utc::now()
        .signed_duration_since(activated)
        .num_seconds()
        .max(0) as u64;
    let freshness_target_seconds = u64::try_from(namespace.freshness_target_seconds).unwrap_or(0);
    let threshold = freshness_target_seconds + config.freshness_alert_grace_seconds;
    if age_seconds <= threshold {
        return Ok(false);
    }

    let correlation_id = format!("supervisor:cache:freshness:{}", namespace.id);
    if MaintenanceRepository::alert_recently_emitted(
        ctx.pool,
        &correlation_id,
        config.alert_cooldown_seconds,
    )
    .await?
    {
        return Ok(false);
    }

    let alert = SystemAlert {
        severity: "warning".to_string(),
        category: "cache".to_string(),
        failure_type: "cache_namespace_stale".to_string(),
        component_type: "cache_namespace".to_string(),
        component_id: Some(namespace.id),
        component_ref: Some(owner_type_label(namespace.owner_type).to_string()),
        worker_role: None,
        observed_at: Utc::now(),
        summary: format!(
            "Cache namespace {} active generation is {}s old, exceeding its {}s freshness target",
            namespace.id, age_seconds, freshness_target_seconds
        ),
        details: json!({
            "namespace_id": namespace.id,
            "owner_type": owner_type_label(namespace.owner_type),
            "active_generation_id": active_id,
            "age_seconds": age_seconds,
            "freshness_target_seconds": freshness_target_seconds,
            "freshness_alert_grace_seconds": config.freshness_alert_grace_seconds,
            "service_name": ctx.service_name,
            "environment": ctx.environment,
        }),
        correlation_id: Some(correlation_id),
    };
    emit_core_alert(ctx.pool, ctx.publisher, alert).await?;
    Ok(true)
}

/// Emits a bounded, redacted alert when a namespace's persisted refresh
/// failure streak reaches `staging_failure_alert_threshold`. Returns `true`
/// only when an alert was actually emitted (not suppressed by cooldown).
async fn maybe_emit_staging_failure_alert(
    ctx: &CacheRetentionContext<'_>,
    config: &CacheRetentionConfig,
    namespace: &CacheNamespace,
) -> Result<bool> {
    let consecutive_failures =
        u32::try_from(namespace.consecutive_refresh_failures).unwrap_or(u32::MAX);
    if consecutive_failures < config.staging_failure_alert_threshold {
        return Ok(false);
    }

    let correlation_id = format!("supervisor:cache:staging-failures:{}", namespace.id);
    if MaintenanceRepository::alert_recently_emitted(
        ctx.pool,
        &correlation_id,
        config.alert_cooldown_seconds,
    )
    .await?
    {
        return Ok(false);
    }

    let alert = SystemAlert {
        severity: "warning".to_string(),
        category: "cache".to_string(),
        failure_type: "cache_staging_repeated_failure".to_string(),
        component_type: "cache_namespace".to_string(),
        component_id: Some(namespace.id),
        component_ref: Some(owner_type_label(namespace.owner_type).to_string()),
        worker_role: None,
        observed_at: Utc::now(),
        summary: format!(
            "Cache namespace {} has {} consecutive failed refresh generations",
            namespace.id, consecutive_failures
        ),
        details: json!({
            "namespace_id": namespace.id,
            "owner_type": owner_type_label(namespace.owner_type),
            "consecutive_failures": consecutive_failures,
            "staging_failure_alert_threshold": config.staging_failure_alert_threshold,
            "service_name": ctx.service_name,
            "environment": ctx.environment,
        }),
        correlation_id: Some(correlation_id),
    };
    emit_core_alert(ctx.pool, ctx.publisher, alert).await?;
    Ok(true)
}

/// Reclaims bounded candidates atomically, without deleting entry rows.
async fn drain_cleanup_candidates(
    ctx: &CacheRetentionContext<'_>,
    config: &CacheRetentionConfig,
    summary: &mut CacheRetentionCycleSummary,
) -> Result<()> {
    let generation_limit = config
        .max_generations_per_cycle
        .clamp(1, MAX_CLEANUP_SELECTION);
    let watermark = *ctx.state.cleanup_after_id.lock().await;
    let mut candidates = CacheGenerationRepository::select_cleanup_candidates_after(
        ctx.pool,
        watermark,
        generation_limit,
    )
    .await?;
    if let Some(watermark) = watermark {
        let remaining = generation_limit - candidates.len() as i64;
        if remaining > 0 {
            candidates.extend(
                CacheGenerationRepository::select_cleanup_candidates_after(
                    ctx.pool, None, remaining,
                )
                .await?
                .into_iter()
                .filter(|generation| generation.id <= watermark),
            );
        }
    }
    summary.cleanup_candidates = candidates.len();
    summary.cleanup_backlog_saturated = candidates.len() >= generation_limit as usize;
    for candidate in &candidates {
        match candidate.state {
            CacheGenerationState::Failed => {
                summary.failed_cleanup_candidates =
                    summary.failed_cleanup_candidates.saturating_add(1);
            }
            CacheGenerationState::Retired => {
                summary.expired_snapshot_cleanup_candidates = summary
                    .expired_snapshot_cleanup_candidates
                    .saturating_add(1);
            }
            _ => {}
        }
    }

    if config.dry_run {
        return Ok(());
    }

    let started = Instant::now();
    for candidate in candidates {
        let remaining = config
            .max_cleanup_cycle_milliseconds
            .saturating_sub(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
        if remaining == 0 {
            summary.cleanup_budget_exhausted = true;
            break;
        }
        *ctx.state.cleanup_after_id.lock().await = Some(candidate.id);
        let mut bounded = config.clone();
        bounded.max_cleanup_cycle_milliseconds = remaining;
        bounded.ddl_statement_timeout_milliseconds =
            bounded.ddl_statement_timeout_milliseconds.min(remaining);
        bounded.ddl_lock_timeout_milliseconds =
            bounded.ddl_lock_timeout_milliseconds.min(remaining);
        let ddl_started = Instant::now();
        let outcome =
            CacheGenerationRepository::drop_if_cleanup_eligible(ctx.pool, candidate.id, &bounded)
                .await;
        summary.reclamation_duration_ms = summary
            .reclamation_duration_ms
            .saturating_add(u64::try_from(ddl_started.elapsed().as_millis()).unwrap_or(u64::MAX));
        match outcome {
            Ok(CacheGenerationCleanupOutcome::Dropped { records, bytes }) => {
                summary.entries_deleted += records;
                summary.bytes_reclaimed += bytes;
                summary.generations_deleted += 1;
                match candidate.state {
                    CacheGenerationState::Failed => {
                        summary.failed_generations_deleted += 1;
                    }
                    CacheGenerationState::Retired => {
                        summary.expired_snapshots_deleted += 1;
                    }
                    _ => {}
                }
            }
            Ok(CacheGenerationCleanupOutcome::DeferredBusy) => summary.lock_deferrals += 1,
            Ok(CacheGenerationCleanupOutcome::DeferredDeadline) => summary.deadline_deferrals += 1,
            Ok(
                CacheGenerationCleanupOutcome::Absent | CacheGenerationCleanupOutcome::Ineligible,
            ) => {}
            Err(err) => {
                summary.cleanup_failures += 1;
                warn!(generation_id = candidate.id, error = %err, "Cache partition reclamation failed");
            }
        }
        if u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
            >= config.max_cleanup_cycle_milliseconds
        {
            summary.cleanup_budget_exhausted = true;
            break;
        }
    }

    Ok(())
}

async fn delete_empty_tombstoned_namespaces(
    ctx: &CacheRetentionContext<'_>,
    config: &CacheRetentionConfig,
    summary: &mut CacheRetentionCycleSummary,
) -> Result<()> {
    if config.dry_run {
        return Ok(());
    }
    let limit = config
        .max_namespaces_per_cycle
        .clamp(1, MAX_CLEANUP_SELECTION);
    summary.namespaces_deleted +=
        CacheNamespaceRepository::delete_empty_tombstoned_batch(ctx.pool, limit).await? as usize;
    Ok(())
}

fn bounded_duration(seconds: u64) -> ChronoDuration {
    ChronoDuration::seconds(i64::try_from(seconds).unwrap_or(i64::MAX))
}

fn age_seconds(now: chrono::DateTime<Utc>, timestamp: chrono::DateTime<Utc>) -> u64 {
    u64::try_from(now.signed_duration_since(timestamp).num_seconds().max(0)).unwrap_or(u64::MAX)
}

fn owner_type_index(owner_type: OwnerType) -> usize {
    match owner_type {
        OwnerType::System => 0,
        OwnerType::Identity => 1,
        OwnerType::Pack => 2,
        OwnerType::Action => 3,
        OwnerType::Sensor => 4,
    }
}

fn owner_type_label(owner_type: OwnerType) -> &'static str {
    match owner_type {
        OwnerType::System => "system",
        OwnerType::Identity => "identity",
        OwnerType::Pack => "pack",
        OwnerType::Action => "action",
        OwnerType::Sensor => "sensor",
    }
}

fn emit_operational_metrics(summary: &CacheRetentionCycleSummary) {
    info!(
        component = "cache_maintenance",
        metric_set = "cache_maintenance_cycle",
        status = if summary.maintenance_failures
            + summary.cleanup_failures
            + summary.statistics_failures
            > 0
        {
            "partial_failure"
        } else {
            "success"
        },
        dry_run = summary.dry_run,
        maintenance_cycle_count = 1u64,
        maintenance_failure_count = summary.maintenance_failures,
        maintenance_duration_ms = summary.maintenance_duration_ms,
        partitions_dropped = summary.generations_deleted,
        bytes_reclaimed = summary.bytes_reclaimed,
        lock_deferrals = summary.lock_deferrals,
        deadline_deferrals = summary.deadline_deferrals,
        cleanup_failures = summary.cleanup_failures,
        registered_partitions = summary.registered_partitions,
        partitions_created_total = summary.partitions_created_total,
        partitions_dropped_total = summary.partitions_dropped_total,
        cleanup_backlog_total = summary.cleanup_backlog_total,
        oldest_cleanup_age_seconds = summary.oldest_cleanup_age_seconds,
        reclamation_duration_ms = summary.reclamation_duration_ms,
        statistics_duration_ms = summary.statistics_duration_ms,
        statistics_pending = summary.statistics_pending,
        storage_observed = summary.storage_observed,
        statistics_age_seconds = ?summary.statistics_age_seconds,
        statistics_refreshed = summary.statistics_refreshed,
        statistics_lock_deferrals = summary.statistics_lock_deferrals,
        statistics_deadline_deferrals = summary.statistics_deadline_deferrals,
        statistics_failures = summary.statistics_failures,
        cleanup_budget_exhausted = summary.cleanup_budget_exhausted,
        namespaces_scanned = summary.namespaces_scanned,
        fresh_namespaces = summary.fresh_namespaces,
        stale_namespaces = summary.stale_namespaces,
        namespaces_without_active_generation = summary.namespaces_without_active_generation,
        namespace_age_max_seconds = summary.namespace_age_max_seconds,
        active_generation_age_max_seconds = summary.active_generation_age_max_seconds,
        last_successful_refresh_age_max_seconds = summary.active_generation_age_max_seconds,
        staging_generation_age_max_seconds = summary.staging_generation_age_max_seconds,
        refresh_failures_observed = summary.refresh_failures_observed,
        records_observed = summary.records_observed,
        storage_bytes_observed = summary.storage_bytes_observed,
        cleanup_backlog_generations = summary.cleanup_candidates,
        cleanup_backlog_saturated = summary.cleanup_backlog_saturated,
        failed_cleanup_candidates = summary.failed_cleanup_candidates,
        expired_snapshot_cleanup_candidates = summary.expired_snapshot_cleanup_candidates,
        expired_staging_generations = summary.staging_expired,
        entries_deleted = summary.entries_deleted,
        failed_generations_deleted = summary.failed_generations_deleted,
        expired_snapshots_deleted = summary.expired_snapshots_deleted,
        namespaces_deleted = summary.namespaces_deleted,
        freshness_alerts = summary.freshness_alerts,
        staging_failure_alerts = summary.staging_failure_alerts,
        "Cache maintenance operational metrics"
    );

    for (index, owner_type) in [
        OwnerType::System,
        OwnerType::Identity,
        OwnerType::Pack,
        OwnerType::Action,
        OwnerType::Sensor,
    ]
    .into_iter()
    .enumerate()
    {
        let scope = summary.scope_metrics[index];
        if scope.namespaces == 0 {
            continue;
        }
        info!(
            component = "cache_maintenance",
            metric_set = "cache_scope_storage",
            owner_type = owner_type_label(owner_type),
            namespace_count = scope.namespaces,
            record_count = scope.records,
            storage_bytes = scope.storage_bytes,
            refresh_failure_count = scope.refresh_failures,
            "Cache scope operational metrics"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{
        config::Config,
        repositories::{
            cache::{
                CacheEntryInput, CacheNamespacePolicy, CacheOwnerScope, CreateCacheGenerationInput,
                CreateCacheGenerationResult, CreateCacheNamespaceInput, InsertCacheChunkResult,
            },
            CacheEntryRepository, CacheIngestRepository, Create,
        },
        test_database::TestDatabase,
    };
    use chrono::Duration;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_test_id() -> String {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros()
            % 1_000_000;
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        format!("{timestamp}{counter}")
    }

    /// Create a fully migrated schema-isolated database for one test.
    async fn test_pool() -> TestDatabase {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        let config_path = format!("{manifest_dir}/../../config.test.yaml");
        let mut config = Config::load_from_file(&config_path).expect("load test config");
        if let Ok(url) = std::env::var("CACHE_RETENTION_TEST_DATABASE_URL") {
            config.database.url = url;
        }
        TestDatabase::create(&config.database)
            .await
            .expect("create isolated supervisor test database")
            .with_cleanup_on_drop()
    }

    /// Ensures the shared `core.alert` trigger exists (idempotent). Real
    /// deployments register this via a bootstrap/core pack; tests must seed
    /// it themselves since a fresh per-test schema starts empty.
    async fn ensure_core_alert_trigger(pool: &PgPool) {
        sqlx::query(
            "INSERT INTO trigger (ref, label) VALUES ('core.alert', 'System Alert') \
             ON CONFLICT (ref) DO NOTHING",
        )
        .execute(pool)
        .await
        .expect("seed core.alert trigger fixture");
    }

    fn ctx<'a>(pool: &'a PgPool) -> CacheRetentionContext<'a> {
        ctx_with_state(pool, Arc::new(CacheRetentionState::default()))
    }

    fn ctx_with_state<'a>(
        pool: &'a PgPool,
        state: Arc<CacheRetentionState>,
    ) -> CacheRetentionContext<'a> {
        CacheRetentionContext {
            pool,
            publisher: None,
            service_name: "attune-supervisor-test",
            environment: "test",
            state,
        }
    }

    fn test_config() -> CacheRetentionConfig {
        CacheRetentionConfig {
            enabled: true,
            max_cleanup_cycle_milliseconds: 30_000,
            ddl_lock_timeout_milliseconds: 250,
            ddl_creation_statement_timeout_milliseconds: 5_000,
            ddl_statement_timeout_milliseconds: 1_000,
            statistics_interval_seconds: 300,
            statistics_statement_timeout_milliseconds: 5_000,
            max_generations_per_cycle: 50,
            max_namespaces_per_cycle: 50,
            min_traversal_window_seconds: 0,
            staging_expiry_seconds: 0,
            dry_run: false,
            freshness_alerts_enabled: true,
            freshness_alert_grace_seconds: 0,
            staging_failure_alert_threshold: 3,
            alert_cooldown_seconds: 3600,
            alert_limit_per_cycle: 25,
        }
    }

    async fn create_namespace(pool: &PgPool, policy: CacheNamespacePolicy) -> CacheNamespace {
        CacheNamespaceRepository::create(
            pool,
            CreateCacheNamespaceInput {
                owner: CacheOwnerScope::system(),
                namespace: format!("ns_{}", unique_test_id()),
                policy,
            },
        )
        .await
        .expect("create test cache namespace")
    }

    async fn create_generation(pool: &PgPool, namespace_id: Id) -> CacheGeneration {
        let expected_active_generation = CacheNamespaceRepository::find_by_id(pool, namespace_id)
            .await
            .expect("load cache namespace")
            .expect("cache namespace exists")
            .active_generation;
        match CacheGenerationRepository::create_or_get(
            pool,
            &CreateCacheGenerationInput {
                namespace: namespace_id,
                client_refresh_id: format!("refresh_{}", unique_test_id()),
                expected_active_generation,
                expected_chunk_count: 1,
                expected_count: None,
                expected_bytes: None,
                checksum_algorithm: None,
                checksum: None,
                source_revision: None,
                created_by: None,
                created_by_execution: None,
            },
        )
        .await
        .expect("create test cache generation")
        {
            CreateCacheGenerationResult::Created(generation)
            | CreateCacheGenerationResult::Existing(generation) => generation,
        }
    }

    async fn age_generation(pool: &PgPool, generation_id: Id) {
        sqlx::query(
            "UPDATE cache_generation SET created = NOW() - INTERVAL '1 hour' WHERE id = $1",
        )
        .bind(generation_id)
        .execute(pool)
        .await
        .expect("age cache generation");
    }

    /// Seeds one or more entries into a generation as a single ingest chunk
    /// (chunk index 0). Multiple entries must be seeded together this way
    /// rather than via repeated single-entry calls: `insert_chunk` is
    /// idempotent per `(generation, chunk_index)`, so re-using chunk index 0
    /// across separate calls would just replay the first call.
    async fn seed_entries(pool: &PgPool, generation_id: Id, external_ids: &[&str]) {
        let entries: Vec<CacheEntryInput> = external_ids
            .iter()
            .map(|external_id| CacheEntryInput {
                external_id: (*external_id).to_string(),
                value: json!({"id": external_id}),
                source_updated_at: None,
                source_checksum: None,
            })
            .collect();
        match CacheIngestRepository::insert_chunk(pool, generation_id, 0, "chk-v1", &entries)
            .await
            .expect("insert test cache entries")
        {
            InsertCacheChunkResult::Inserted(_) | InsertCacheChunkResult::Replayed(_) => {}
        }
    }

    async fn seed_entry(pool: &PgPool, generation_id: Id, external_id: &str) {
        seed_entries(pool, generation_id, &[external_id]).await;
    }

    async fn seal_and_promote(
        pool: &PgPool,
        namespace_id: Id,
        generation_id: Id,
        expected_active: Option<Id>,
        prior_readable_until: chrono::DateTime<Utc>,
    ) -> CacheGeneration {
        CacheGenerationRepository::seal(pool, generation_id)
            .await
            .expect("seal test cache generation");
        CacheGenerationRepository::promote(
            pool,
            namespace_id,
            generation_id,
            expected_active,
            prior_readable_until,
        )
        .await
        .expect("promote test cache generation")
        .activated_generation
    }

    #[tokio::test]
    async fn disabled_config_skips_cleanup_entirely() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, generation.id, "abc").await;

        let mut config = test_config();
        config.enabled = false;
        config.staging_expiry_seconds = 0; // would otherwise expire immediately

        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary, CacheRetentionCycleSummary::default());

        let still_staging = CacheGenerationRepository::find_by_id(&pool, generation.id)
            .await
            .expect("find generation")
            .expect("generation still present");
        assert_eq!(still_staging.state, CacheGenerationState::Staging);
    }

    #[tokio::test]
    async fn enabled_invocation_expires_abandoned_staging_generation() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;
        age_generation(&pool, generation.id).await;

        let config = test_config(); // staging_expiry_seconds: 0

        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.staging_expired, 1);
        // The abandoned generation has no entries, so the same cycle also
        // drains (trivially) and deletes it once `fail()` makes it a cleanup
        // candidate -- this is the intended end-to-end invocation behavior.
        assert_eq!(summary.failed_cleanup_candidates, 1);
        assert_eq!(summary.failed_generations_deleted, 1);
        assert_eq!(summary.generations_deleted, 1);

        let gone = CacheGenerationRepository::find_by_id(&pool, generation.id)
            .await
            .expect("find generation");
        assert!(
            gone.is_none(),
            "abandoned staging generation with no entries is reclaimed in one cycle"
        );
    }

    #[tokio::test]
    async fn enabled_invocation_expires_abandoned_ready_generation() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;
        age_generation(&pool, generation.id).await;
        seed_entry(&pool, generation.id, "ready-but-unpublished").await;
        CacheGenerationRepository::seal(&pool, generation.id)
            .await
            .expect("seal ready generation");

        let summary = run_cache_retention_cycle(&ctx(&pool), &test_config())
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.staging_expired, 1);
        assert_eq!(summary.entries_deleted, 1);
        assert_eq!(summary.generations_deleted, 1);
        assert!(CacheGenerationRepository::find_by_id(&pool, generation.id)
            .await
            .expect("find generation")
            .is_none());
    }

    #[tokio::test]
    async fn newer_failed_generations_do_not_hide_expired_unpublished_generation() {
        let pool = test_pool().await;
        let namespace = create_namespace(
            &pool,
            CacheNamespacePolicy {
                max_staging_generations: 150,
                ..CacheNamespacePolicy::default()
            },
        )
        .await;
        let abandoned = create_generation(&pool, namespace.id).await;
        sqlx::query(
            "UPDATE cache_generation SET created = NOW() - INTERVAL '2 hours' WHERE id = $1",
        )
        .bind(abandoned.id)
        .execute(&pool)
        .await
        .expect("age abandoned generation fixture");

        for _ in 0..101 {
            let generation = create_generation(&pool, namespace.id).await;
            CacheGenerationRepository::fail(&pool, generation.id, "test: newer failed generation")
                .await
                .expect("fail newer generation");
        }

        let mut config = test_config();
        config.staging_expiry_seconds = 3600;
        config.max_generations_per_cycle = 1;
        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.staging_expired, 1);
        assert_eq!(summary.cleanup_candidates, 1);
        assert_eq!(summary.generations_deleted, 1);
        assert_eq!(summary.failed_generations_deleted, 1);
        assert_eq!(summary.cleanup_failures, 0);
        let abandoned_after_cleanup = CacheGenerationRepository::find_by_id(&pool, abandoned.id)
            .await
            .expect("find abandoned generation");
        let newer = CacheGenerationRepository::list_for_namespace(&pool, namespace.id, 150)
            .await
            .expect("find remaining newer failures");
        pool.cleanup().await.unwrap();
        assert!(
            abandoned_after_cleanup.is_none(),
            "ascending-ID cleanup must reclaim the expired oldest generation first"
        );
        assert_eq!(
            newer.len(),
            101,
            "one-generation cleanup must leave all newer candidates"
        );
        assert!(newer
            .iter()
            .all(|generation| generation.state == CacheGenerationState::Failed));
    }

    #[tokio::test]
    async fn dry_run_reports_without_mutating() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;
        age_generation(&pool, generation.id).await;

        let mut config = test_config();
        config.dry_run = true;

        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.staging_expired, 1);
        assert_eq!(summary.entries_deleted, 0);
        assert_eq!(summary.generations_deleted, 0);

        let still_staging = CacheGenerationRepository::find_by_id(&pool, generation.id)
            .await
            .expect("find generation")
            .expect("generation still present");
        assert_eq!(still_staging.state, CacheGenerationState::Staging);
    }

    #[tokio::test]
    async fn active_generation_entries_are_preserved() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, generation.id, "keep-me").await;
        seal_and_promote(
            &pool,
            namespace.id,
            generation.id,
            None,
            Utc::now() + Duration::hours(1),
        )
        .await;

        let config = test_config();
        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.cleanup_candidates, 0);
        assert_eq!(summary.entries_deleted, 0);

        let active = CacheGenerationRepository::find_by_id(&pool, generation.id)
            .await
            .expect("find generation")
            .expect("active generation still present");
        assert_eq!(active.state, CacheGenerationState::Active);
        let entry = CacheEntryRepository::find_active(&pool, namespace.id, "keep-me")
            .await
            .expect("lookup active entry");
        assert!(entry.is_some());
    }

    #[tokio::test]
    async fn pinned_retired_generation_within_window_is_preserved() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;

        let first = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, first.id, "still-readable").await;
        seal_and_promote(
            &pool,
            namespace.id,
            first.id,
            None,
            Utc::now() + Duration::hours(1),
        )
        .await;

        let second = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, second.id, "new-active").await;
        // Retiring `first` with a *future* readable_until keeps it pinned.
        seal_and_promote(
            &pool,
            namespace.id,
            second.id,
            Some(first.id),
            Utc::now() + Duration::hours(1),
        )
        .await;

        let config = test_config();
        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.cleanup_candidates, 0);
        assert_eq!(summary.generations_deleted, 0);

        let retired = CacheGenerationRepository::find_by_id(&pool, first.id)
            .await
            .expect("find retired generation")
            .expect("retired generation still present");
        assert_eq!(retired.state, CacheGenerationState::Retired);
        let readable =
            CacheGenerationRepository::find_readable_pinned(&pool, namespace.id, first.id)
                .await
                .expect("readable pinned lookup");
        assert!(
            readable.is_some(),
            "retired generation within its window must stay readable"
        );
    }

    #[tokio::test]
    async fn expired_retired_generation_is_drained_and_deleted() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;

        let first = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, first.id, "expire-me").await;
        // readable_until already in the past: activates then instantly expires
        // once superseded below.
        seal_and_promote(
            &pool,
            namespace.id,
            first.id,
            None,
            Utc::now() - Duration::seconds(1),
        )
        .await;

        let second = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, second.id, "current").await;
        seal_and_promote(
            &pool,
            namespace.id,
            second.id,
            Some(first.id),
            Utc::now() - Duration::seconds(1),
        )
        .await;

        let config = test_config();
        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.cleanup_candidates, 1);
        assert_eq!(summary.expired_snapshot_cleanup_candidates, 1);
        assert_eq!(summary.entries_deleted, 1);
        assert_eq!(summary.generations_deleted, 1);
        assert_eq!(summary.expired_snapshots_deleted, 1);

        let gone = CacheGenerationRepository::find_by_id(&pool, first.id)
            .await
            .expect("find generation");
        assert!(
            gone.is_none(),
            "expired retired generation must be deleted once drained"
        );
    }

    #[tokio::test]
    async fn tombstoned_namespace_drains_and_deletes_once_empty() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;
        seed_entries(&pool, generation.id, &["orphan-1", "orphan-2"]).await;

        let tombstoned = CacheNamespaceRepository::tombstone(&pool, namespace.id)
            .await
            .expect("tombstone namespace");
        assert!(tombstoned);

        // tombstone() already marks the in-flight staging generation failed.
        let after_tombstone = CacheGenerationRepository::find_by_id(&pool, generation.id)
            .await
            .expect("find generation")
            .expect("generation still present before drain");
        assert_eq!(after_tombstone.state, CacheGenerationState::Failed);

        let config = test_config();
        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.cleanup_candidates, 1);
        assert_eq!(summary.entries_deleted, 2);
        assert_eq!(summary.generations_deleted, 1);
        assert_eq!(summary.namespaces_deleted, 1);

        let namespace_gone = CacheNamespaceRepository::find_by_id(&pool, namespace.id)
            .await
            .expect("find namespace");
        assert!(
            namespace_gone.is_none(),
            "emptied tombstoned namespace must be deleted"
        );
    }

    #[tokio::test]
    async fn tombstoned_namespace_without_generations_is_deleted_independently() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        CacheNamespaceRepository::tombstone(&pool, namespace.id)
            .await
            .expect("tombstone empty namespace");

        let summary = run_cache_retention_cycle(&ctx(&pool), &test_config())
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.cleanup_candidates, 0);
        assert_eq!(summary.namespaces_deleted, 1);
        assert!(CacheNamespaceRepository::find_by_id(&pool, namespace.id)
            .await
            .expect("find namespace")
            .is_none());
    }

    #[tokio::test]
    async fn atomic_reclamation_removes_all_entries_and_never_double_releases_usage() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;
        seed_entries(&pool, generation.id, &["a", "b", "c", "d", "e"]).await;
        CacheGenerationRepository::fail(&pool, generation.id, "test: force cleanup eligible")
            .await
            .expect("fail generation for cleanup eligibility");

        let config = test_config();

        let first_cycle = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("first cache retention cycle");
        assert_eq!(
            first_cycle.entries_deleted, 5,
            "one partition drop reclaims the entire selected generation"
        );
        assert_eq!(
            first_cycle.generations_deleted, 1,
            "generation metadata is removed in the drop transaction"
        );

        let second_cycle = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("second cache retention cycle");
        assert_eq!(second_cycle.entries_deleted, 0);
        assert_eq!(second_cycle.generations_deleted, 0);
        assert_eq!(second_cycle.bytes_reclaimed, 0);
        assert!(first_cycle.bytes_reclaimed > 0);

        let third_cycle = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("third cache retention cycle");
        assert_eq!(third_cycle.entries_deleted, 0);
        assert_eq!(
            third_cycle.generations_deleted, 0,
            "a completed cleanup is an idempotent no-op"
        );
    }

    #[tokio::test]
    async fn blocked_oldest_generation_does_not_starve_later_cycles() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let first = create_generation(&pool, namespace.id).await;
        CacheGenerationRepository::fail(&pool, first.id, "fixture")
            .await
            .unwrap();
        let second = create_generation(&pool, namespace.id).await;
        CacheGenerationRepository::fail(&pool, second.id, "fixture")
            .await
            .unwrap();
        let mut held = pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM cache_generation WHERE id=$1 FOR UPDATE")
            .bind(first.id)
            .execute(&mut *held)
            .await
            .unwrap();
        let state = Arc::new(CacheRetentionState::default());
        let context = ctx_with_state(&pool, state);
        let mut config = test_config();
        config.max_generations_per_cycle = 1;
        config.ddl_lock_timeout_milliseconds = 50;
        config.freshness_alerts_enabled = false;
        let blocked = run_cache_retention_cycle(&context, &config).await.unwrap();
        assert_eq!(blocked.lock_deferrals, 1);
        assert_eq!(blocked.generations_deleted, 0);
        let next = run_cache_retention_cycle(&context, &config).await.unwrap();
        assert_eq!(next.generations_deleted, 1);
        assert!(CacheGenerationRepository::find_by_id(&pool, second.id)
            .await
            .unwrap()
            .is_none());
        assert!(CacheGenerationRepository::find_by_id(&pool, first.id)
            .await
            .unwrap()
            .is_some());
        held.rollback().await.unwrap();
        let wrapped = run_cache_retention_cycle(&context, &config).await.unwrap();
        assert_eq!(wrapped.generations_deleted, 1);
        pool.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn cleanup_admission_wait_uses_remaining_cycle_and_preserves_rotation() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let first = create_generation(&pool, namespace.id).await;
        let second = create_generation(&pool, namespace.id).await;
        for generation in [first.id, second.id] {
            CacheGenerationRepository::fail(&pool, generation, "owned fixture")
                .await
                .unwrap();
        }
        let mut holder = pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(7821101,0)")
            .execute(&mut *holder)
            .await
            .unwrap();
        let state = Arc::new(CacheRetentionState::default());
        let context = ctx_with_state(&pool, state.clone());
        let mut config = test_config();
        config.max_cleanup_cycle_milliseconds = 300;
        let mut summary = CacheRetentionCycleSummary::default();
        let started = Instant::now();
        drain_cleanup_candidates(&context, &config, &mut summary)
            .await
            .unwrap();
        let elapsed = started.elapsed();
        let cursor = *state.cleanup_after_id.lock().await;
        holder.rollback().await.unwrap();
        let mut next = CacheRetentionCycleSummary::default();
        config.max_cleanup_cycle_milliseconds = 2_000;
        drain_cleanup_candidates(&context, &config, &mut next)
            .await
            .unwrap();
        pool.cleanup().await.unwrap();
        assert!(elapsed < std::time::Duration::from_secs(1));
        assert!(summary.cleanup_budget_exhausted);
        assert_eq!(summary.generations_deleted, 0);
        assert_eq!(summary.entries_deleted, 0);
        assert_eq!(summary.bytes_reclaimed, 0);
        assert_eq!(summary.deadline_deferrals, 1);
        assert_eq!(summary.lock_deferrals, 0);
        assert_eq!(cursor, Some(first.id));
        assert_eq!(next.generations_deleted, 2);
    }

    #[tokio::test]
    async fn cleanup_later_deferral_keeps_confirmed_reclamation_counts() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let first = create_generation(&pool, namespace.id).await;
        seed_entries(&pool, first.id, &["owned-first"]).await;
        let second = create_generation(&pool, namespace.id).await;
        seed_entries(&pool, second.id, &["owned-second"]).await;
        for generation in [first.id, second.id] {
            CacheGenerationRepository::fail(&pool, generation, "owned fixture")
                .await
                .unwrap();
        }
        let mut held = pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM cache_generation WHERE id=$1 FOR UPDATE")
            .bind(second.id)
            .execute(&mut *held)
            .await
            .unwrap();
        let mut config = test_config();
        config.max_cleanup_cycle_milliseconds = 500;
        let mut summary = CacheRetentionCycleSummary::default();
        drain_cleanup_candidates(&ctx(&pool), &config, &mut summary)
            .await
            .unwrap();
        let first_remaining = CacheGenerationRepository::find_by_id(&pool, first.id)
            .await
            .unwrap();
        let second_remaining = CacheGenerationRepository::find_by_id(&pool, second.id)
            .await
            .unwrap();
        held.rollback().await.unwrap();
        pool.cleanup().await.unwrap();
        assert_eq!(summary.generations_deleted, 1);
        assert_eq!(summary.entries_deleted, 1);
        assert!(summary.bytes_reclaimed > 0);
        assert_eq!(summary.lock_deferrals + summary.deadline_deferrals, 1);
        assert!(first_remaining.is_none());
        assert!(second_remaining.is_some());
    }

    #[tokio::test]
    async fn bounded_generations_per_cycle_limits_candidates_processed() {
        let pool = test_pool().await;
        let namespace = create_namespace(
            &pool,
            CacheNamespacePolicy {
                max_staging_generations: 3,
                ..CacheNamespacePolicy::default()
            },
        )
        .await;

        let mut generation_ids = Vec::new();
        for _ in 0..3 {
            let generation = create_generation(&pool, namespace.id).await;
            CacheGenerationRepository::fail(&pool, generation.id, "test: force cleanup eligible")
                .await
                .expect("fail generation for cleanup eligibility");
            generation_ids.push(generation.id);
        }

        let mut config = test_config();
        config.max_generations_per_cycle = 1;

        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");
        assert_eq!(
            summary.cleanup_candidates, 1,
            "only one candidate is selected per cycle"
        );
        assert_eq!(summary.generations_deleted, 1);

        let mut remaining = 0;
        for id in &generation_ids {
            if CacheGenerationRepository::find_by_id(&pool, *id)
                .await
                .expect("find generation")
                .is_some()
            {
                remaining += 1;
            }
        }
        assert_eq!(
            remaining, 2,
            "unprocessed candidates remain for the next cycle"
        );
    }

    #[tokio::test]
    async fn namespace_watermark_traverses_fairly_and_wraps() {
        let pool = test_pool().await;
        let mut namespaces = Vec::new();
        for _ in 0..5 {
            namespaces.push(create_namespace(&pool, CacheNamespacePolicy::default()).await);
        }

        let state = Arc::new(CacheRetentionState::default());
        let mut config = test_config();
        config.max_namespaces_per_cycle = 2;
        config.staging_expiry_seconds = 3600;
        config.freshness_alerts_enabled = false;

        run_cache_retention_cycle(&ctx_with_state(&pool, state.clone()), &config)
            .await
            .expect("first cache retention cycle");
        assert_eq!(
            state.namespace_after_id().await,
            Some(namespaces[1].id),
            "first cycle advances beyond the fixed prefix"
        );

        run_cache_retention_cycle(&ctx_with_state(&pool, state.clone()), &config)
            .await
            .expect("second cache retention cycle");
        assert_eq!(state.namespace_after_id().await, Some(namespaces[3].id));

        run_cache_retention_cycle(&ctx_with_state(&pool, state.clone()), &config)
            .await
            .expect("wrapping cache retention cycle");
        assert_eq!(
            state.namespace_after_id().await,
            Some(namespaces[0].id),
            "tail capacity is filled from the head without waiting an empty cycle"
        );
    }

    #[tokio::test]
    async fn namespace_watermark_survives_tombstones_and_wraparound() {
        let pool = test_pool().await;
        let mut namespaces = Vec::new();
        for _ in 0..4 {
            namespaces.push(create_namespace(&pool, CacheNamespacePolicy::default()).await);
        }

        let state = Arc::new(CacheRetentionState::default());
        let mut config = test_config();
        config.max_namespaces_per_cycle = 2;
        config.staging_expiry_seconds = 3600;
        config.freshness_alerts_enabled = false;

        run_cache_retention_cycle(&ctx_with_state(&pool, state.clone()), &config)
            .await
            .expect("first cache retention cycle");
        CacheNamespaceRepository::tombstone(&pool, namespaces[2].id)
            .await
            .expect("tombstone namespace");

        let wrapped = run_cache_retention_cycle(&ctx_with_state(&pool, state.clone()), &config)
            .await
            .expect("cycle across tombstoned watermark gap");
        assert_eq!(wrapped.namespaces_scanned, 2);
        assert_eq!(
            state.namespace_after_id().await,
            Some(namespaces[0].id),
            "scanner processes the live tail then wraps around the tombstoned row"
        );

        run_cache_retention_cycle(&ctx_with_state(&pool, state.clone()), &config)
            .await
            .expect("post-wrap cache retention cycle");
        assert_eq!(
            state.namespace_after_id().await,
            Some(namespaces[3].id),
            "remaining live namespaces continue to receive maintenance"
        );
    }

    #[tokio::test]
    async fn operational_metrics_cover_freshness_failures_storage_and_cleanup() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;

        let active = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, active.id, "active-record").await;
        seal_and_promote(
            &pool,
            namespace.id,
            active.id,
            None,
            Utc::now() + Duration::hours(1),
        )
        .await;

        let failed = create_generation(&pool, namespace.id).await;
        CacheGenerationRepository::fail(&pool, failed.id, "test refresh failure")
            .await
            .expect("fail refresh generation");

        let mut config = test_config();
        config.staging_expiry_seconds = 3600;
        config.freshness_alerts_enabled = false;
        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.fresh_namespaces, 1);
        assert_eq!(summary.stale_namespaces, 0);
        assert_eq!(summary.refresh_failures_observed, 1);
        assert_eq!(summary.records_observed, 1);
        assert!(summary.storage_bytes_observed > 0);
        assert_eq!(summary.failed_cleanup_candidates, 1);
        assert_eq!(summary.failed_generations_deleted, 1);
        assert_eq!(summary.expired_snapshots_deleted, 0);
        assert_eq!(summary.scope_metrics[0].namespaces, 1);
        assert_eq!(summary.scope_metrics[0].refresh_failures, 1);
    }

    #[tokio::test]
    async fn freshness_alert_is_emitted_and_redacted() {
        let pool = test_pool().await;
        ensure_core_alert_trigger(&pool).await;

        let namespace = create_namespace(
            &pool,
            CacheNamespacePolicy {
                freshness_target_seconds: 1,
                ..CacheNamespacePolicy::default()
            },
        )
        .await;
        let generation = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, generation.id, "sensitive-external-id-12345").await;
        seal_and_promote(
            &pool,
            namespace.id,
            generation.id,
            None,
            Utc::now() + Duration::hours(1),
        )
        .await;

        // `age_seconds` truncates to whole seconds; make sure at least one
        // full second separates `activated` from the freshness check below
        // instead of racing sub-second clock precision.
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;

        let config = test_config(); // freshness_alert_grace_seconds: 0
        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.freshness_alerts, 1);

        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM event WHERE trigger_ref = 'core.alert' ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .expect("fetch emitted alert payload");

        let payload_text = payload.to_string();
        assert!(
            !payload_text.contains(&namespace.namespace),
            "alert payload must not include the namespace name"
        );
        assert!(
            !payload_text.contains("sensitive-external-id-12345"),
            "alert payload must never include external IDs or cached values"
        );
        assert_eq!(
            payload["details"]["namespace_id"].as_i64(),
            Some(namespace.id),
            "alert must still carry the bounded numeric namespace id"
        );
        assert_eq!(payload["failure_type"], "cache_namespace_stale");
    }

    #[tokio::test]
    async fn zero_freshness_target_disables_staleness_metrics_and_alerts() {
        let pool = test_pool().await;
        ensure_core_alert_trigger(&pool).await;
        let namespace = create_namespace(
            &pool,
            CacheNamespacePolicy {
                freshness_target_seconds: 0,
                ..CacheNamespacePolicy::default()
            },
        )
        .await;
        let generation = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, generation.id, "freshness-disabled").await;
        seal_and_promote(
            &pool,
            namespace.id,
            generation.id,
            None,
            Utc::now() + Duration::hours(1),
        )
        .await;
        sqlx::query(
            "UPDATE cache_generation SET activated = NOW() - INTERVAL '1 day' WHERE id = $1",
        )
        .bind(generation.id)
        .execute(&pool)
        .await
        .expect("age active generation");

        let summary = run_cache_retention_cycle(&ctx(&pool), &test_config())
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.fresh_namespaces, 1);
        assert_eq!(summary.stale_namespaces, 0);
        assert_eq!(summary.freshness_alerts, 0);
    }

    #[tokio::test]
    async fn repeated_staging_failures_trigger_alert() {
        let pool = test_pool().await;
        ensure_core_alert_trigger(&pool).await;

        let namespace = create_namespace(
            &pool,
            CacheNamespacePolicy {
                max_staging_generations: 3,
                ..CacheNamespacePolicy::default()
            },
        )
        .await;
        for _ in 0..3 {
            let generation = create_generation(&pool, namespace.id).await;
            CacheGenerationRepository::fail(&pool, generation.id, "test: simulated ingest failure")
                .await
                .expect("fail generation");
        }

        let mut config = test_config();
        config.staging_expiry_seconds = 3600; // nothing left in staging state to expire
        config.staging_failure_alert_threshold = 3;

        let summary = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.staging_failure_alerts, 1);

        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM event WHERE trigger_ref = 'core.alert' ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .expect("fetch emitted alert payload");
        assert_eq!(payload["failure_type"], "cache_staging_repeated_failure");
        assert_eq!(payload["details"]["consecutive_failures"].as_i64(), Some(3));
        let persisted = CacheNamespaceRepository::find_by_id(&pool, namespace.id)
            .await
            .expect("load namespace")
            .expect("namespace remains live");
        assert_eq!(persisted.consecutive_refresh_failures, 3);
        assert!(
            CacheGenerationRepository::list_for_namespace(&pool, namespace.id, 10)
                .await
                .expect("list cleaned generations")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn namespace_delete_failure_preserves_committed_partition_progress() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, generation.id, "owned-entry").await;
        CacheNamespaceRepository::tombstone_with_reason(&pool, namespace.id, "owned test cleanup")
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE FUNCTION test_reject_namespace_delete() RETURNS TRIGGER LANGUAGE plpgsql AS $$
               BEGIN RAISE EXCEPTION 'owned namespace delete failure'; END $$;
             CREATE TRIGGER test_reject_namespace_delete BEFORE DELETE ON cache_namespace
               FOR EACH ROW EXECUTE FUNCTION test_reject_namespace_delete();",
        )
        .execute(&*pool)
        .await
        .unwrap();
        let summary = run_cache_retention_cycle(&ctx(&pool), &test_config())
            .await
            .unwrap();
        assert_eq!(summary.generations_deleted, 1);
        assert_eq!(summary.entries_deleted, 1);
        assert!(summary.bytes_reclaimed > 0);
        assert_eq!(summary.maintenance_failures, 1);
        assert!(summary.had_effect());
        assert!(CacheGenerationRepository::find_by_id(&*pool, generation.id)
            .await
            .unwrap()
            .is_none());
        assert!(CacheNamespaceRepository::find_by_id(&*pool, namespace.id)
            .await
            .unwrap()
            .is_some());
        sqlx::query("DROP TRIGGER test_reject_namespace_delete ON cache_namespace")
            .execute(&*pool)
            .await
            .unwrap();
        let next = run_cache_retention_cycle(&ctx(&pool), &test_config())
            .await
            .unwrap();
        assert_eq!(next.generations_deleted, 0);
        assert_eq!(next.namespaces_deleted, 1);
        assert_eq!(next.maintenance_failures, 0);
        pool.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn statistics_failure_preserves_cleanup_and_pending_work() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;
        seed_entry(&pool, generation.id, "owned-entry").await;
        CacheGenerationRepository::fail(&pool, generation.id, "owned test cleanup")
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE FUNCTION test_reject_statistics_ack() RETURNS TRIGGER LANGUAGE plpgsql AS $$
               BEGIN
                 IF NEW.completed_revision IS DISTINCT FROM OLD.completed_revision THEN
                   RAISE EXCEPTION 'owned statistics acknowledgement failure';
                 END IF;
                 RETURN NEW;
               END $$;
             CREATE TRIGGER test_reject_statistics_ack BEFORE UPDATE ON cache_entry_statistics_state
               FOR EACH ROW EXECUTE FUNCTION test_reject_statistics_ack();",
        )
        .execute(&*pool)
        .await
        .unwrap();
        let summary = run_cache_retention_cycle(&ctx(&pool), &test_config())
            .await
            .unwrap();
        assert_eq!(summary.generations_deleted, 1);
        assert_eq!(summary.entries_deleted, 1);
        assert_eq!(summary.statistics_failures, 1);
        assert_eq!(summary.maintenance_failures, 0);
        assert!(summary.statistics_pending);
        assert_eq!(summary.registered_partitions, 0);
        assert_eq!(summary.partitions_created_total, 1);
        assert_eq!(summary.partitions_dropped_total, 1);
        sqlx::query("DROP TRIGGER test_reject_statistics_ack ON cache_entry_statistics_state")
            .execute(&*pool)
            .await
            .unwrap();
        let next = run_cache_retention_cycle(&ctx(&pool), &test_config())
            .await
            .unwrap();
        assert!(next.statistics_refreshed);
        assert!(!next.statistics_pending);
        assert_eq!(next.statistics_failures, 0);
        pool.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn abandoned_failures_trigger_threshold_alert_in_same_cycle() {
        let pool = test_pool().await;
        ensure_core_alert_trigger(&pool).await;
        let namespace = create_namespace(
            &pool,
            CacheNamespacePolicy {
                max_staging_generations: 3,
                ..CacheNamespacePolicy::default()
            },
        )
        .await;
        for _ in 0..3 {
            let generation = create_generation(&pool, namespace.id).await;
            age_generation(&pool, generation.id).await;
        }

        let summary = run_cache_retention_cycle(&ctx(&pool), &test_config())
            .await
            .expect("cache retention cycle");

        assert_eq!(summary.staging_expired, 3);
        assert_eq!(summary.staging_failure_alerts, 1);
        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM event WHERE trigger_ref = 'core.alert' ORDER BY id DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .expect("fetch emitted alert payload");
        assert_eq!(payload["failure_type"], "cache_staging_repeated_failure");
        assert_eq!(payload["details"]["consecutive_failures"].as_i64(), Some(3));
    }
}
