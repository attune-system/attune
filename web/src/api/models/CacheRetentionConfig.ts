/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Supervisor-owned cache generation/entry retention configuration.
 *
 * Persisted as the `cache_retention` JSON object on
 * `runtime_retention_config`, exposed through the retention API, and reloaded
 * at the start of every supervisor cycle. Cache cleanup runs as a distinct
 * step inside the existing retention cycle and reuses its advisory lock and
 * cadence rather than electing a second leader.
 */
export type CacheRetentionConfig = {
  /**
   * Suppress duplicate cache alerts sharing a correlation id for this long.
   */
  alert_cooldown_seconds?: number;
  /**
   * Maximum cache alerts emitted per supervisor cycle.
   */
  alert_limit_per_cycle?: number;
  /**
   * Server-side statement deadline for refresh partition creation, independent of cleanup.
   */
  ddl_creation_statement_timeout_milliseconds?: number;
  /**
   * Maximum wait for cache partition DDL locks.
   */
  ddl_lock_timeout_milliseconds?: number;
  /**
   * Server-side deadline for one atomic generation reclamation.
   */
  ddl_statement_timeout_milliseconds?: number;
  /**
   * Report cleanup candidates and metrics without deleting rows.
   */
  dry_run?: boolean;
  /**
   * Enable cache generation/entry cleanup as part of the retention cycle.
   */
  enabled?: boolean;
  /**
   * Extra grace beyond a namespace's own `freshness_target_seconds` before
   * a stale active generation is treated as alert-worthy.
   */
  freshness_alert_grace_seconds?: number;
  /**
   * Emit a `core.alert` when a namespace's active generation exceeds its
   * freshness target, or a namespace repeatedly fails to publish a
   * staging generation.
   */
  freshness_alerts_enabled?: boolean;
  /**
   * Total generation-reclamation budget per supervisor cycle.
   */
  max_cleanup_cycle_milliseconds?: number;
  /**
   * Maximum cleanup-candidate generations (failed, or retired past
   * `readable_until`) processed in a single supervisor cycle.
   */
  max_generations_per_cycle?: number;
  /**
   * Maximum namespaces inspected for staging expiry/freshness per cycle,
   * and maximum tombstoned-and-emptied namespaces deleted per cycle.
   */
  max_namespaces_per_cycle?: number;
  /**
   * Minimum time a retired generation remains readable after retirement.
   * Enforced defensively by the supervisor in addition to the generation's
   * own stored `readable_until`, so cleanup never races a traversal that
   * began while the generation was still active.
   */
  min_traversal_window_seconds?: number;
  /**
   * Unpublished staging or ready generations older than this many seconds
   * are treated as abandoned; the supervisor marks them `failed` so the
   * normal cleanup path reclaims them.
   */
  staging_expiry_seconds?: number;
  /**
   * Consecutive staging failures observed for the same namespace within
   * the freshness lookback before a repeated-failure alert is emitted.
   */
  staging_failure_alert_threshold?: number;
  /**
   * Minimum interval between successful parent/leaf cache statistics refreshes.
   */
  statistics_interval_seconds?: number;
  /**
   * Independent statement deadline for cache parent/leaf ANALYZE.
   */
  statistics_statement_timeout_milliseconds?: number;
};
