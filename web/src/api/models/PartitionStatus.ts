/* generated using openapi-typescript-codegen -- do not edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
import type { ManagedTable } from "./ManagedTable";
/**
 * Catalog-verified status. DEFAULT counts stop at repair_cap + 1; they are
 * lower bounds when default_count_exact is false, never full backlog scans.
 */
export type PartitionStatus = {
  default_count_exact: boolean;
  default_rows_at_least: number;
  future_partitions: number;
  missing_future_partitions: number;
  oldest_default_day?: string | null;
  parent: ManagedTable;
  registered_partitions: number;
};
