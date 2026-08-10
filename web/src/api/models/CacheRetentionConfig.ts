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
   * Maximum `cache_entry` rows deleted per bounded batch call.
   */
  batch_size?: number;
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
   * Maximum entry-deletion batches performed for a single cleanup-candidate
   * generation within one supervisor cycle. Bounds how long a single
   * high-cardinality generation can dominate a cycle; entries are always
   * deleted in indexed bounded batches before the generation row itself.
   */
  max_batches_per_generation?: number;
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
};
