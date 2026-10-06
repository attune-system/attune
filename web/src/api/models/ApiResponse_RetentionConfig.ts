/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheRetentionConfig } from "./CacheRetentionConfig";
import type { RetentionTargetsConfig } from "./RetentionTargetsConfig";
/**
 * Standard API response wrapper
 */
export type ApiResponse_RetentionConfig = {
  /**
   * Supervisor-owned runtime retention configuration.
   */
  data: {
    /**
     * Advisory lock key used to make accidental multi-supervisor deployments safe.
     */
    advisory_lock_key?: number;
    /**
     * Maximum rows to delete in each committed batch.
     */
    batch_size?: number;
    /**
     * Cache generation/entry retention and freshness maintenance. Persisted
     * with the runtime retention singleton and reloaded every cycle.
     */
    cache_retention?: CacheRetentionConfig;
    /**
     * How often the supervisor runs retention, in seconds.
     */
    check_interval_seconds?: number;
    /**
     * Report candidate rows without deleting them.
     */
    dry_run?: boolean;
    /**
     * Enable runtime row retention globally.
     */
    enabled?: boolean;
    /**
     * Maximum committed batches per target per cycle. Each target can delete
     * at most batch_size * max_batches_per_target rows per cycle.
     */
    max_batches_per_target?: number;
    /**
     * Per-target retention settings.
     */
    targets?: RetentionTargetsConfig;
  };
  /**
   * Optional message
   */
  message?: string | null;
};
