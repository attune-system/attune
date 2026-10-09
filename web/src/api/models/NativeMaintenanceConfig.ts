/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
/**
 * Bounded native partition and hourly-summary maintenance.
 */
export type NativeMaintenanceConfig = {
  default_repair_row_limit?: number;
  enabled?: boolean;
  lock_timeout_milliseconds?: number;
  max_partition_cycle_milliseconds?: number;
  max_partition_operations_per_cycle?: number;
  max_summary_buckets_per_cycle?: number;
  max_summary_cycle_milliseconds?: number;
  max_summary_invalidations_per_bucket?: number;
  operation_timeout_milliseconds?: number;
  partition_interval_seconds?: number;
  partition_lookahead_days?: number;
  summary_bootstrap_hours?: number;
  summary_interval_seconds?: number;
};
