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
//!   generations' entries in bounded batches, deletes the emptied
//!   generation, and once a tombstoned namespace has no generations left,
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
        cache::MAX_CLEANUP_SELECTION, CacheEntryRepository, CacheGenerationRepository,
        CacheNamespaceRepository, FindById, MaintenanceRepository,
    },
    system_alert::{emit_core_alert, SystemAlert},
    Result,
};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Generations inspected per namespace when looking for abandoned staging
/// generations or computing a repeated-staging-failure streak. A namespace's
/// live generation count is already bounded by its own
/// `max_staging_generations`/`max_retained_generations` policy, so this is a
/// generous supervisor-side safety cap rather than a tunable knob.
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
    }
}

/// Runs one bounded cache retention/freshness step. Intended to be called
/// once per supervisor retention cycle, inside the same advisory lock as
/// runtime row retention.
pub async fn run_cache_retention_cycle(
    ctx: &CacheRetentionContext<'_>,
    config: &CacheRetentionConfig,
) -> Result<CacheRetentionCycleSummary> {
    if !config.enabled {
        info!("Cache retention is disabled in configuration; skipping cache cleanup step");
        return Ok(CacheRetentionCycleSummary::default());
    }

    let started = Instant::now();
    let result = async {
        let mut summary = CacheRetentionCycleSummary {
            dry_run: config.dry_run,
            ..Default::default()
        };
        scan_namespaces(ctx, config, &mut summary).await?;
        drain_cleanup_candidates(ctx, config, &mut summary).await?;
        Ok::<_, attune_common::Error>(summary)
    }
    .await;

    match result {
        Ok(mut summary) => {
            summary.maintenance_duration_ms =
                u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            emit_operational_metrics(&summary);
            Ok(summary)
        }
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
            Err(err)
        }
    }
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

    for namespace in &namespaces {
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
        if let Err(err) =
            observe_namespace_metrics(ctx.pool, namespace, &generations, summary).await
        {
            warn!(
                namespace_id = namespace.id,
                error = %err,
                "Failed to collect cache namespace operational metrics"
            );
        }

        for generation in &generations {
            if !matches!(
                generation.state,
                CacheGenerationState::Staging | CacheGenerationState::Ready
            ) || generation.created >= staging_cutoff
            {
                continue;
            }
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
            }
        }

        if !config.freshness_alerts_enabled {
            continue;
        }

        if freshness_alerts_emitted < alert_limit {
            match maybe_emit_freshness_alert(ctx, config, namespace).await {
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
            match maybe_emit_staging_failure_alert(ctx, config, namespace, &generations).await {
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

    for generation in generations {
        let records = u64::try_from(generation.record_count.max(0)).unwrap_or(u64::MAX);
        let bytes = u64::try_from(generation.size_bytes.max(0)).unwrap_or(u64::MAX);
        summary.records_observed = summary.records_observed.saturating_add(records);
        summary.storage_bytes_observed = summary.storage_bytes_observed.saturating_add(bytes);
        scope.records = scope.records.saturating_add(records);
        scope.storage_bytes = scope.storage_bytes.saturating_add(bytes);

        if generation.state == CacheGenerationState::Failed {
            summary.refresh_failures_observed = summary.refresh_failures_observed.saturating_add(1);
            scope.refresh_failures = scope.refresh_failures.saturating_add(1);
        }
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
    if active_age > freshness_target {
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

/// Emits a bounded, redacted alert when a namespace's most recent
/// generations, ordered newest-first, contain a run of consecutive `failed`
/// entries at or beyond `staging_failure_alert_threshold`. Returns `true`
/// only when an alert was actually emitted (not suppressed by cooldown).
async fn maybe_emit_staging_failure_alert(
    ctx: &CacheRetentionContext<'_>,
    config: &CacheRetentionConfig,
    namespace: &CacheNamespace,
    generations_newest_first: &[CacheGeneration],
) -> Result<bool> {
    let mut consecutive_failures: u32 = 0;
    for generation in generations_newest_first {
        if generation.state == CacheGenerationState::Failed {
            consecutive_failures += 1;
        } else {
            break;
        }
    }
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

/// Drains bounded cleanup-candidate generations: entries first, in indexed
/// bounded batches, then the emptied generation, then (bounded) the emptied
/// tombstoned namespace. Never deletes an entire high-cardinality generation
/// in one transaction.
async fn drain_cleanup_candidates(
    ctx: &CacheRetentionContext<'_>,
    config: &CacheRetentionConfig,
    summary: &mut CacheRetentionCycleSummary,
) -> Result<()> {
    let generation_limit = config
        .max_generations_per_cycle
        .clamp(1, MAX_CLEANUP_SELECTION);
    let candidates =
        CacheGenerationRepository::select_cleanup_candidates(ctx.pool, generation_limit).await?;
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

    let min_traversal_window = bounded_duration(config.min_traversal_window_seconds);
    let batch_size = config.batch_size.clamp(1, MAX_CLEANUP_SELECTION);
    let max_batches = config
        .max_batches_per_generation
        .clamp(1, MAX_CLEANUP_SELECTION);
    let max_namespace_deletes = config.max_namespaces_per_cycle.max(0);
    let mut namespaces_deleted = 0i64;

    for candidate in candidates {
        // Defensive re-check: a retired generation is only touched once both
        // its own stored `readable_until` (already filtered by
        // `select_cleanup_candidates`) *and* the configured minimum
        // traversal window have elapsed since retirement. Active generations
        // are never returned by `select_cleanup_candidates` at all.
        if candidate.state == CacheGenerationState::Retired {
            if let Some(retired_at) = candidate.retired {
                if Utc::now() - retired_at < min_traversal_window {
                    continue;
                }
            }
        }

        if let Err(err) =
            drain_generation_entries(ctx, candidate.id, batch_size, max_batches, summary).await
        {
            warn!(
                generation_id = candidate.id,
                error = %err,
                "Failed to drain cache generation entries"
            );
            continue;
        }

        match CacheGenerationRepository::delete_if_empty(ctx.pool, candidate.id).await {
            Ok(true) => {
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
                if namespaces_deleted < max_namespace_deletes {
                    match CacheNamespaceRepository::delete_tombstoned_if_empty(
                        ctx.pool,
                        candidate.namespace,
                    )
                    .await
                    {
                        Ok(true) => {
                            namespaces_deleted += 1;
                            summary.namespaces_deleted += 1;
                        }
                        Ok(false) => {}
                        Err(err) => warn!(
                            namespace_id = candidate.namespace,
                            error = %err,
                            "Failed to delete emptied tombstoned cache namespace"
                        ),
                    }
                }
            }
            Ok(false) => {}
            Err(err) => warn!(
                generation_id = candidate.id,
                error = %err,
                "Failed to delete emptied cache generation"
            ),
        }
    }

    Ok(())
}

/// Deletes entries from one cleanup-candidate generation in indexed bounded
/// batches, stopping once a batch removes nothing or `max_batches` is
/// reached, whichever happens first. A generation with remaining entries
/// beyond the per-cycle bound is simply picked up again next cycle.
async fn drain_generation_entries(
    ctx: &CacheRetentionContext<'_>,
    generation_id: Id,
    batch_size: i64,
    max_batches: i64,
    summary: &mut CacheRetentionCycleSummary,
) -> Result<()> {
    let mut batches = 0i64;
    while batches < max_batches {
        let deleted =
            CacheEntryRepository::delete_cleanup_batch(ctx.pool, generation_id, batch_size).await?;
        batches += 1;
        summary.entries_deleted += deleted;
        if deleted == 0 {
            break;
        }
    }
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
        status = "success",
        dry_run = summary.dry_run,
        maintenance_cycle_count = 1u64,
        maintenance_failure_count = 0u64,
        maintenance_duration_ms = summary.maintenance_duration_ms,
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
        db::Database,
        repositories::{
            cache::{
                CacheEntryInput, CacheNamespacePolicy, CacheOwnerScope, CreateCacheGenerationInput,
                CreateCacheGenerationResult, CreateCacheNamespaceInput, InsertCacheChunkResult,
            },
            CacheIngestRepository, Create,
        },
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

    /// Schema-per-test pool, mirroring `crates/common/tests/helpers.rs`
    /// (which is not reusable here since this crate has no library target).
    /// Applies every migration under `migrations/` into a freshly created,
    /// uniquely named schema so tests never collide with each other or with
    /// other crates' test runs against the same test database.
    ///
    /// The connection URL is read from `CACHE_RETENTION_TEST_DATABASE_URL`
    /// (falling back to the project's documented local test database) rather
    /// than through `Config::load_from_file` + `ATTUNE__DATABASE__URL`, since
    /// that layered environment override is not exercised by this narrow
    /// test helper.
    async fn test_pool() -> PgPool {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        let migrations_path = format!("{manifest_dir}/../../migrations");

        let database_url = std::env::var("CACHE_RETENTION_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://attune@localhost:5432/attune_test".to_string());
        let mut database_config: attune_common::config::DatabaseConfig =
            serde_json::from_value(json!({})).expect("default database config");
        database_config.url = database_url;
        database_config.max_connections = 5;

        let schema = format!("test_supervisor_cache_{}", unique_test_id());

        let base_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_config.url)
            .await
            .expect("connect base pool for schema setup");
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"))
            .execute(&base_pool)
            .await
            .expect("create per-test schema");

        let migration_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect({
                let schema = schema.clone();
                move |conn, _meta| {
                    let schema = schema.clone();
                    Box::pin(async move {
                        sqlx::query(&format!("SET search_path TO {schema}"))
                            .execute(&mut *conn)
                            .await?;
                        Ok(())
                    })
                }
            })
            .connect(&database_config.url)
            .await
            .expect("connect migration pool");

        let mut migration_files: Vec<_> = std::fs::read_dir(&migrations_path)
            .expect("read migrations directory")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("sql"))
            .collect();
        migration_files.sort_by_key(|entry| entry.path());

        const DEFAULT_SEARCH_PATH: &str = "SET search_path TO attune, public;";
        for migration_file in migration_files {
            let raw = std::fs::read_to_string(migration_file.path()).expect("read migration file");
            let rewritten = raw.replace(
                DEFAULT_SEARCH_PATH,
                &format!("SET search_path TO {schema}, public;"),
            );
            sqlx::query(&format!("SET search_path TO {schema}"))
                .execute(&migration_pool)
                .await
                .expect("set search_path before migration");
            if let Err(err) = sqlx::raw_sql(&rewritten).execute(&migration_pool).await {
                let message = format!("{err:?}");
                if !message.contains("already exists") && !message.contains("duplicate") {
                    panic!("migration {:?} failed: {err}", migration_file.path());
                }
            }
        }

        database_config.schema = Some(schema);
        let database = Database::new(&database_config)
            .await
            .expect("connect scoped test database");
        database.pool().clone()
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
            batch_size: 1000,
            max_batches_per_generation: 20,
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
            },
        )
        .await
        .expect("create test cache generation")
        {
            CreateCacheGenerationResult::Created(generation)
            | CreateCacheGenerationResult::Existing(generation) => generation,
        }
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
    async fn dry_run_reports_without_mutating() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;

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
    async fn bounded_batches_limit_entries_deleted_per_cycle() {
        let pool = test_pool().await;
        let namespace = create_namespace(&pool, CacheNamespacePolicy::default()).await;
        let generation = create_generation(&pool, namespace.id).await;
        seed_entries(&pool, generation.id, &["a", "b", "c", "d", "e"]).await;
        CacheGenerationRepository::fail(&pool, generation.id, "test: force cleanup eligible")
            .await
            .expect("fail generation for cleanup eligibility");

        let mut config = test_config();
        config.batch_size = 1;
        config.max_batches_per_generation = 2;

        let first_cycle = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("first cache retention cycle");
        assert_eq!(
            first_cycle.entries_deleted, 2,
            "must not delete more than batch_size * max_batches in one cycle"
        );
        assert_eq!(
            first_cycle.generations_deleted, 0,
            "generation still has entries left"
        );

        let second_cycle = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("second cache retention cycle");
        assert_eq!(second_cycle.entries_deleted, 2);
        assert_eq!(second_cycle.generations_deleted, 0);

        let third_cycle = run_cache_retention_cycle(&ctx(&pool), &config)
            .await
            .expect("third cache retention cycle");
        assert_eq!(third_cycle.entries_deleted, 1);
        assert_eq!(
            third_cycle.generations_deleted, 1,
            "generation is deleted once fully drained"
        );
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
                freshness_target_seconds: 0,
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
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

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
    }
}
