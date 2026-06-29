/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
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
     * Maximum rows to delete per target per cycle for regular tables.
     */
    batch_size?: number;
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
  /**
   * Optional message
   */
  message?: string | null;
};
