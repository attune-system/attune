/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { CacheRetentionConfig } from "./CacheRetentionConfig";
import type { RetentionTargetsConfig } from "./RetentionTargetsConfig";
/**
 * Supervisor-owned runtime retention configuration.
 */
export type RetentionConfig = {
  /**
   * Advisory lock key used to make accidental multi-supervisor deployments safe.
   */
  advisory_lock_key?: number;
  /**
   * Maximum rows to delete per target per cycle for regular tables.
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
   * Report candidates without deleting rows/chunks.
   */
  dry_run?: boolean;
  /**
   * Enable runtime row retention globally.
   */
  enabled?: boolean;
  /**
   * Per-target retention settings.
   */
  targets?: RetentionTargetsConfig;
};
